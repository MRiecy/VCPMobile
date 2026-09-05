use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::vcp_modules::chat::ast_diff::{
    diff_ast_streaming, prime_stream_code_highlighter, render_stream_snapshot, AstMutation,
};
use crate::vcp_modules::content_parser::BlockType;
use crate::vcp_modules::pre_renderer::code_highlighter::IncrementalCodeHighlighter;
use crate::vcp_modules::pre_renderer::markdown_ast::MarkdownNode;
use crate::vcp_modules::stream_block_parser::{
    speculative_thought_tail, StreamBlock, StreamBlockParser,
};

/// 推测渲染的 tail 字节上限：超过此阈值跳过 AST 解析，降级为纯文本尾部。
///
/// 取值依据（perf profile 基准，见 benches/ast_tail_bench.rs，约等于发布版热路径速度）：
/// - 代码围栏 tail 走 `code_fence_tail_nodes` 快速路径，每帧成本 O(Δ)（增量高亮补丁），
///   已豁免本上限。
/// - 协议块（Tool/ToolResult/ToolCallSummary）tail 封印为 plaintext 代码节点走同一增量
///   路径，且工具结果必定闭合到达、不存在长期未闭合 tail，已豁免本上限。
/// - 思维链 tail 走 Thought 外壳 + 纯文本增量投影（Plain 模式），根本不做 md AST 解析，
///   与本上限无关。
/// - RawHtml 容器 tail 走 html5ever 树权威 PatchRawHtml 路径（冻结段 + 活跃子树），
///   IPC/前端均 O(增量)，已豁免本上限。
/// - 因此本上限如今只兜底仍是 O(tail)/帧 全量重解析的通用 Markdown tail：
///   正常流中段落会随空行沉淀为 stable 块，tail 难以涨大；64KB 纯属「模型输出永不换行
///   的巨型段落」病理防呆，避免最坏帧在低端机上失控。
const MAX_SPECULATIVE_TAIL_AST_BYTES: usize = 65536;

/// 单进程内单调递增的 Aurora 流身份。一个 `AuroraBuffer` 对应一条独立序列域；
/// 暖接续会创建新 buffer，因此不能继续沿用上一条流的 frameSeq 比较基线。
static NEXT_AURORA_STREAM_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Serialize, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TailFrame {
    pub stream_id: u64,
    pub epoch: u64,
    pub revision: u64,
    pub frame_seq: u64,
    #[serde(default, skip_serializing_if = "is_false")]
    pub reset: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<Vec<MarkdownNode>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mutations: Vec<AstMutation>,
}

#[derive(Debug, Serialize, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuroraUpdateKind {
    Delta,
    Snapshot,
}

#[derive(Debug, Serialize, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TailRenderMode {
    Ast,
    Plain,
}

#[derive(Debug, Serialize, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum TailBlockType {
    Markdown,
    HtmlPreview,
    Thought,
}

#[derive(Debug, Serialize, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StableAppend {
    pub base_count: usize,
    pub blocks: Vec<StreamBlock>,
}

#[derive(Debug, Serialize, Clone, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum TailTextOp {
    Append {
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(rename = "previousHash")]
        previous_hash: Option<String>,
        content: String,
        hash: String,
        mode: TailRenderMode,
        #[serde(rename = "blockType")]
        block_type: TailBlockType,
        #[serde(rename = "thoughtTheme", skip_serializing_if = "Option::is_none")]
        thought_theme: Option<String>,
    },
    Replace {
        content: String,
        hash: String,
        mode: TailRenderMode,
        #[serde(rename = "blockType")]
        block_type: TailBlockType,
        #[serde(rename = "thoughtTheme", skip_serializing_if = "Option::is_none")]
        thought_theme: Option<String>,
    },
    Clear,
}

/// Aurora 语义沉淀更新，由 Rust 流式管道推送到前端
/// 采用稀疏序列化：只在字段有变化时才包含在 JSON 中，减少 IPC payload
#[derive(Debug, Serialize, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuroraUpdate {
    pub kind: AuroraUpdateKind,
    /// 整条 Aurora 更新所属的流身份；非流式单次响应没有该字段。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_id: Option<u64>,
    /// Snapshot 中完整覆盖的已沉淀语义块。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stable_blocks: Option<Vec<StreamBlock>>,
    /// Delta 中从 `base_count` 开始追加的新稳定块。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stable_append: Option<StableAppend>,
    /// Snapshot 中完整覆盖的推测 tail；AST 节点统一由 `tail_frame.snapshot` 承载。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tail_block: Option<StreamBlock>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tail_mode: Option<TailRenderMode>,
    /// Delta 中对 tail 显示投影的追加、替换或清空操作；Thought 起始标记不进入该正文。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tail_op: Option<TailTextOp>,
    /// 流式 AST 单帧补丁。每个 frame 是独立发送批次，前端不得累计全历史 mutations。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tail_frame: Option<TailFrame>,
    /// 全量内容（仅终结事件时发送，正常流式中省略）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// 🆕 推送周期中新增的、尚未推送给前端的纯文本片段
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chunk: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuroraRecoverySnapshot {
    pub stable_blocks: Vec<StreamBlock>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tail_block: Option<StreamBlock>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tail_mode: Option<TailRenderMode>,
    pub tail_snapshot: Vec<MarkdownNode>,
}

pub struct AuroraDeliveryCommit {
    pushed_len: usize,
    pushed_stable_count: usize,
    pushed_tail_len: usize,
    pushed_tail_epoch: u64,
    pushed_tail_hash: Option<String>,
    pushed_tail_mode: Option<TailRenderMode>,
    pushed_tail_block_type: Option<TailBlockType>,
    pushed_tail_source_start: Option<usize>,
    consumes_tail_frame: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Clone)]
struct TailFingerprint {
    state: Sha256,
    content_len: usize,
    source_start: usize,
}

impl TailFingerprint {
    fn from_content(content: &str, source_start: usize) -> Self {
        let mut state = Sha256::new();
        state.update(content.as_bytes());
        Self {
            state,
            content_len: content.len(),
            source_start,
        }
    }

    fn with_suffix(&self, suffix: &str) -> Self {
        let mut next = self.clone();
        next.state.update(suffix.as_bytes());
        next.content_len = next.content_len.saturating_add(suffix.len());
        next
    }

    fn wire_hash(&self) -> String {
        crate::vcp_modules::infra::utils::finalize_sha256_hex(self.state.clone())
    }
}

struct TailProjection {
    fingerprint: TailFingerprint,
    mode: TailRenderMode,
    block_type: TailBlockType,
    thought_theme: Option<String>,
}

fn classify_tail_block(nodes: &[MarkdownNode]) -> TailBlockType {
    match nodes {
        [MarkdownNode::CodeBlock {
            lang: Some(lang), ..
        }] if lang.eq_ignore_ascii_case("html") => TailBlockType::HtmlPreview,
        _ => TailBlockType::Markdown,
    }
}

/// 未闭合代码围栏 tail 的快速节点构造。
///
/// stream 层只在结束围栏缺席时把 tail 标记为 CodeFence，因此 tail 必然是
/// 「开围栏行 + 未闭合内容」：切首行取 info string 作 lang，其余原文作 code，
/// 与 pulldown 路径语义一致（内容字面保留、未闭合延伸至 EOF）。
/// hash 保持 None：tail 逐帧增长，diff 的 hash 门控必然 miss，计算只是浪费 O(n)。
fn code_fence_tail_nodes(tail: &str) -> Vec<MarkdownNode> {
    let (opener, code) = match tail.find('\n') {
        Some(nl) => (&tail[..nl], &tail[nl + 1..]),
        None => (tail, ""),
    };
    let opener = opener
        .trim_end_matches('\r')
        .trim_start_matches([' ', '\t']);
    let backticks = opener.bytes().take_while(|&b| b == b'`').count();
    if backticks < 3 {
        // 防御：stream 层已判定 CodeFence，理论上不可达；回退完整管线保持正确性
        return crate::vcp_modules::pre_renderer::parse_markdown_to_ast_streaming(tail);
    }
    // 与 pulldown 的 info string 语义严格对齐：去首尾空白，且恒为 Some（空 info 即 Some("")）
    let info = opener[backticks..].trim();
    // 与 pulldown 的行尾规范化对齐：CRLF / 孤 CR 统一为 LF
    let code = if code.contains('\r') {
        code.replace("\r\n", "\n").replace('\r', "\n")
    } else {
        code.to_string()
    };
    vec![MarkdownNode::code_block(Some(info.to_string()), code)]
}

/// 序列化 rcdom 子树为 HTML 字符串。
fn serialize_html_handle(handle: &markup5ever_rcdom::Handle) -> String {
    use html5ever::serialize::{serialize, SerializeOpts, TraversalScope};
    use markup5ever_rcdom::SerializableHandle;

    let serializable: SerializableHandle = handle.clone().into();
    let mut bytes = Vec::new();
    let opts = SerializeOpts {
        // 默认 ChildrenOnly 只会得到 innerHTML；IncludeNode 才包含节点自身标签
        traversal_scope: TraversalScope::IncludeNode,
        ..SerializeOpts::default()
    };
    serialize(&mut bytes, &serializable, opts).expect("html5ever serialize into Vec is infallible");
    String::from_utf8(bytes).unwrap_or_default()
}

/// HtmlContainer tail 的一帧 patch 切片。
struct HtmlTailPatch {
    /// 新闭合定型的内层子节点序列化 HTML（按序拼接）；种子帧恒为空串。
    frozen_html: String,
    /// 种子帧：最外层元素整棵序列化（含已冻结子节点）；稳态帧：当前活跃的
    /// 最内层最后一个子节点（或空串 = 最外层尚无子节点）。
    live_html: String,
    /// 切分后已冻结的内层子节点总数（前端校验/种子帧基线用）。
    frozen_total: usize,
    /// 种子帧：前端应整体重建容器内容并以 frozen_total 建立冻结基线。
    seed: bool,
}

/// HtmlContainer tail 的「树权威」切分：用 html5ever（与浏览器同套 HTML5 解析算法）
/// 做 fragment 解析。持续存在的 HtmlContainer tail 恒为「单个未闭合最外层元素」
/// （最外层一旦闭合，StreamBlockParser 就会把它沉淀为 stable 块），因此冻结域 =
/// 最外层元素的子节点列表：开放元素链（含 adoption agency / foster parenting 的
/// 全部作用域）必然挂在最后一个子节点上，除它之外全部闭合定型。
///
/// `state` 为此前已下发的冻结数（None = 种子帧）。返回 None 表示根结构异常
/// （多根节点等病理形态，如 table foster parenting），调用方应回退种子帧全量下发。
///
/// 代价说明：每帧对 tail 全量 parse 一次（O(tail)，native 速度）；IPC 与前端 parse
/// 均为 O(增量)。tail 只追加 => 冻结总数单调不减。
fn html_tail_patch_parts(tail: &str, state: Option<usize>) -> Option<HtmlTailPatch> {
    use html5ever::driver::{parse_fragment, ParseOpts};
    use html5ever::tendril::TendrilSink;
    use markup5ever_rcdom::{NodeData, RcDom};

    let dom = parse_fragment(
        RcDom::default(),
        ParseOpts::default(),
        // QualName / ns! / local_name! 经 html5ever 的 markup5ever 重导出，
        // 版本随 html5ever 联动，无需单独 pin markup5ever。
        html5ever::QualName::new(None, html5ever::ns!(html), html5ever::local_name!("div")),
        Vec::new(),
        false,
    )
    .one(tail);

    // parse_fragment 的 rcdom 结构：document → <html> 包裹元素 → fragment 根级子节点
    let root = dom.document.children.borrow().first().cloned()?;
    let root_children = root.children.borrow();
    if root_children.len() != 1 {
        return None;
    }
    let outer = root_children[0].clone();
    if !matches!(outer.data, NodeData::Element { .. }) {
        return None;
    }
    drop(root_children);

    let children = outer.children.borrow();
    let total_frozen = children.len().saturating_sub(1);
    match state {
        Some(already) if already <= total_frozen => {
            let frozen_html = children[already..total_frozen]
                .iter()
                .map(serialize_html_handle)
                .collect();
            let live_html = children
                .last()
                .map(serialize_html_handle)
                .unwrap_or_default();
            Some(HtmlTailPatch {
                frozen_html,
                live_html,
                frozen_total: total_frozen,
                seed: false,
            })
        }
        // 种子帧（含 already > total_frozen 的防御性重播种，理论上不可达）
        _ => Some(HtmlTailPatch {
            frozen_html: String::new(),
            live_html: serialize_html_handle(&outer),
            frozen_total: total_frozen,
            seed: true,
        }),
    }
}

/// Aurora 语义沉淀缓冲区
/// 职责：用轻量块解析器识别已闭合/未闭合块，前端增量接收
pub struct AuroraBuffer {
    pub stream_id: u64,
    pub full_text: String,
    pub stable_blocks: Vec<StreamBlock>,
    pub tail_content: String,
    tail_projection: Option<TailProjection>,
    /// 🆕 上一帧的 tail AST 缓存，用于做增量 Diff 对比
    pub prev_tail_ast: Vec<MarkdownNode>,
    /// 🆕 待发送的增量 AST 突变指令暂存池，防抖丢帧时防止中间差异丢失
    pub pending_mutations: Vec<AstMutation>,
    code_highlighter: IncrementalCodeHighlighter,
    pub tail_epoch: u64,
    pub tail_revision: u64,
    pub tail_reset_pending: bool,
    pub tail_frame_seq: u64,
    /// 🆕 记录已被消费并发送的 full_text 长度，用于计算增量 chunk
    pub pushed_len: usize,
    pushed_stable_count: usize,
    pushed_tail_len: usize,
    pushed_tail_epoch: u64,
    pushed_tail_hash: Option<String>,
    pushed_tail_mode: Option<TailRenderMode>,
    pushed_tail_block_type: Option<TailBlockType>,
    pushed_tail_source_start: Option<usize>,
    /// HtmlContainer tail 的冻结前沿：已下发过的「闭合根级子节点」数量。
    /// None = 下一帧需作为种子帧（全量冻结段 + 活跃子树）下发。
    html_tail_frozen_count: Option<usize>,
    parser: StreamBlockParser,
    is_finishing: bool,
}

impl AuroraBuffer {
    pub fn new() -> Self {
        Self::with_stream_id(NEXT_AURORA_STREAM_ID.fetch_add(1, Ordering::Relaxed))
    }

    fn with_stream_id(stream_id: u64) -> Self {
        Self {
            stream_id,
            full_text: String::new(),
            stable_blocks: Vec::new(),
            tail_content: String::new(),
            tail_projection: None,
            prev_tail_ast: Vec::new(),
            pending_mutations: Vec::new(),
            code_highlighter: IncrementalCodeHighlighter::default(),
            tail_epoch: 0,
            tail_revision: 0,
            tail_reset_pending: false,
            tail_frame_seq: 0,
            pushed_len: 0,
            pushed_stable_count: 0,
            pushed_tail_len: 0,
            pushed_tail_epoch: 0,
            pushed_tail_hash: None,
            pushed_tail_mode: None,
            pushed_tail_block_type: None,
            pushed_tail_source_start: None,
            html_tail_frozen_count: None,
            parser: StreamBlockParser::new(),
            is_finishing: false,
        }
    }

    /// 将新的文本块追加到全文
    pub fn append_chunk(&mut self, chunk: &str) {
        self.full_text.push_str(chunk);
    }

    /// 🆕 提取自上次推送以来累积消费的新增字符
    #[allow(dead_code)]
    pub fn take_chunk(&mut self) -> Option<String> {
        let current_len = self.full_text.len();
        if current_len > self.pushed_len {
            let chunk = self.full_text[self.pushed_len..current_len].to_string();
            self.pushed_len = current_len;
            Some(chunk)
        } else {
            None
        }
    }

    fn current_tail_metadata(&self) -> (Option<String>, Option<TailRenderMode>) {
        self.tail_projection
            .as_ref()
            .map_or((None, None), |projection| {
                (
                    Some(projection.fingerprint.wire_hash()),
                    Some(projection.mode),
                )
            })
    }

    fn current_tail_wire_block(&self) -> Option<StreamBlock> {
        let projection = self.tail_projection.as_ref()?;
        let content = self.tail_content.clone();
        let hash = projection.fingerprint.wire_hash();
        Some(match projection.block_type {
            TailBlockType::Markdown => StreamBlock::markdown(content, None, hash),
            TailBlockType::HtmlPreview => StreamBlock::HtmlPreview {
                content,
                highlighted_content: None,
                hash,
            },
            TailBlockType::Thought => StreamBlock::thought(
                projection
                    .thought_theme
                    .clone()
                    .unwrap_or_else(|| "思维链".to_string()),
                content,
                false,
                None,
                hash,
            ),
        })
    }

    fn current_tail_source_start(&self) -> Option<usize> {
        self.tail_projection
            .as_ref()
            .map(|projection| projection.fingerprint.source_start)
    }

    fn next_tail_fingerprint(&self, new_tail: &str, source_start: usize) -> TailFingerprint {
        let Some(projection) = self.tail_projection.as_ref() else {
            return TailFingerprint::from_content(new_tail, source_start);
        };
        let previous = &projection.fingerprint;
        if previous.source_start != source_start
            || previous.content_len != self.tail_content.len()
            || previous.content_len > new_tail.len()
        {
            return TailFingerprint::from_content(new_tail, source_start);
        }

        debug_assert_eq!(
            new_tail.get(..previous.content_len),
            Some(self.tail_content.as_str()),
            "同一 StreamBlockParser tail 起点必须保持严格追加"
        );
        match new_tail.get(previous.content_len..) {
            Some(suffix) => previous.with_suffix(suffix),
            None => TailFingerprint::from_content(new_tail, source_start),
        }
    }

    fn peek_tail_frame(&self, force_snapshot: bool) -> Option<TailFrame> {
        let reset = force_snapshot || self.tail_reset_pending;
        let snapshot = reset.then(|| render_stream_snapshot(&self.prev_tail_ast, "t"));
        let mutations = self.pending_mutations.clone();

        if !reset && snapshot.is_none() && mutations.is_empty() {
            return None;
        }

        Some(TailFrame {
            stream_id: self.stream_id,
            epoch: self.tail_epoch,
            revision: self.tail_revision,
            frame_seq: self.tail_frame_seq.saturating_add(1),
            reset,
            snapshot,
            mutations: if reset { Vec::new() } else { mutations },
        })
    }

    #[allow(dead_code)]
    pub fn take_tail_frame(&mut self) -> Option<TailFrame> {
        let frame = self.peek_tail_frame(false)?;
        self.tail_reset_pending = false;
        self.pending_mutations.clear();
        self.tail_frame_seq = frame.frame_seq;
        Some(frame)
    }

    pub fn prepare_delta_update(&self) -> Option<(AuroraUpdate, AuroraDeliveryCommit)> {
        let chunk = (self.full_text.len() > self.pushed_len)
            .then(|| self.full_text[self.pushed_len..].to_string());
        let stable_append =
            (self.stable_blocks.len() > self.pushed_stable_count).then(|| StableAppend {
                base_count: self.pushed_stable_count,
                blocks: self.stable_blocks[self.pushed_stable_count..].to_vec(),
            });
        let (current_tail_hash, current_tail_mode) = self.current_tail_metadata();
        let current_tail_block_type = self
            .tail_projection
            .as_ref()
            .map(|projection| projection.block_type);
        let current_thought_theme = self
            .tail_projection
            .as_ref()
            .and_then(|projection| projection.thought_theme.clone());
        let current_tail_source_start = self.current_tail_source_start();
        let tail_state_changed = self.tail_content.len() != self.pushed_tail_len
            || self.tail_epoch != self.pushed_tail_epoch
            || current_tail_hash != self.pushed_tail_hash
            || current_tail_mode != self.pushed_tail_mode
            || current_tail_block_type != self.pushed_tail_block_type;
        let tail_op = if !tail_state_changed {
            None
        } else if self.tail_projection.is_none() {
            Some(TailTextOp::Clear)
        } else {
            let hash = current_tail_hash.clone().unwrap_or_default();
            let mode = current_tail_mode.unwrap_or(TailRenderMode::Ast);
            let block_type = current_tail_block_type.unwrap_or(TailBlockType::Markdown);
            let can_append = self.tail_epoch == self.pushed_tail_epoch
                && self.tail_content.len() > self.pushed_tail_len
                && current_tail_mode == self.pushed_tail_mode
                && current_tail_block_type == self.pushed_tail_block_type
                && current_tail_source_start == self.pushed_tail_source_start;
            if can_append {
                if let Some(suffix) = self.tail_content.get(self.pushed_tail_len..) {
                    Some(TailTextOp::Append {
                        previous_hash: self.pushed_tail_hash.clone(),
                        content: suffix.to_string(),
                        hash,
                        mode,
                        block_type,
                        thought_theme: current_thought_theme.clone(),
                    })
                } else {
                    Some(TailTextOp::Replace {
                        content: self.tail_content.clone(),
                        hash,
                        mode,
                        block_type,
                        thought_theme: current_thought_theme.clone(),
                    })
                }
            } else {
                Some(TailTextOp::Replace {
                    content: self.tail_content.clone(),
                    hash,
                    mode,
                    block_type,
                    thought_theme: current_thought_theme,
                })
            }
        };
        let tail_frame = self.peek_tail_frame(false);

        if chunk.is_none() && stable_append.is_none() && tail_op.is_none() && tail_frame.is_none() {
            return None;
        }

        let update = AuroraUpdate {
            kind: AuroraUpdateKind::Delta,
            stream_id: Some(self.stream_id),
            stable_blocks: None,
            stable_append,
            tail_block: None,
            tail_mode: None,
            tail_op,
            tail_frame,
            content: None,
            chunk,
        };
        let commit = AuroraDeliveryCommit {
            pushed_len: self.full_text.len(),
            pushed_stable_count: self.stable_blocks.len(),
            pushed_tail_len: self.tail_content.len(),
            pushed_tail_epoch: self.tail_epoch,
            pushed_tail_hash: current_tail_hash,
            pushed_tail_mode: current_tail_mode,
            pushed_tail_block_type: current_tail_block_type,
            pushed_tail_source_start: current_tail_source_start,
            consumes_tail_frame: update.tail_frame.is_some(),
        };
        Some((update, commit))
    }

    pub fn prepare_snapshot_update(&self) -> (AuroraUpdate, AuroraDeliveryCommit) {
        let tail_block = self.current_tail_wire_block();
        let (current_tail_hash, current_tail_mode) = self.current_tail_metadata();
        let current_tail_block_type = self
            .tail_projection
            .as_ref()
            .map(|projection| projection.block_type);
        let current_tail_source_start = self.current_tail_source_start();
        let update = AuroraUpdate {
            kind: AuroraUpdateKind::Snapshot,
            stream_id: Some(self.stream_id),
            stable_blocks: Some(self.stable_blocks.clone()),
            stable_append: None,
            tail_block,
            tail_mode: current_tail_mode,
            tail_op: None,
            tail_frame: self.peek_tail_frame(true),
            content: Some(self.full_text.clone()),
            chunk: None,
        };
        let commit = AuroraDeliveryCommit {
            pushed_len: self.full_text.len(),
            pushed_stable_count: self.stable_blocks.len(),
            pushed_tail_len: self.tail_content.len(),
            pushed_tail_epoch: self.tail_epoch,
            pushed_tail_hash: current_tail_hash,
            pushed_tail_mode: current_tail_mode,
            pushed_tail_block_type: current_tail_block_type,
            pushed_tail_source_start: current_tail_source_start,
            consumes_tail_frame: true,
        };
        (update, commit)
    }

    pub fn commit_delivery(&mut self, commit: AuroraDeliveryCommit) {
        self.pushed_len = commit.pushed_len;
        self.pushed_stable_count = commit.pushed_stable_count;
        self.pushed_tail_len = commit.pushed_tail_len;
        self.pushed_tail_epoch = commit.pushed_tail_epoch;
        self.pushed_tail_hash = commit.pushed_tail_hash;
        self.pushed_tail_mode = commit.pushed_tail_mode;
        self.pushed_tail_block_type = commit.pushed_tail_block_type;
        self.pushed_tail_source_start = commit.pushed_tail_source_start;
        if commit.consumes_tail_frame {
            self.tail_reset_pending = false;
            self.pending_mutations.clear();
            self.tail_frame_seq = self.tail_frame_seq.saturating_add(1);
        }
    }

    /// 暖接续的权威数据基线：一次性完整覆盖 content/stable/tail 与 reset AST。
    #[allow(dead_code)]
    pub fn take_recovery_baseline(&mut self) -> AuroraUpdate {
        let (update, commit) = self.prepare_snapshot_update();
        self.commit_delivery(commit);
        update
    }

    pub fn compile_recovery_snapshot(content: String) -> AuroraRecoverySnapshot {
        // 无状态重建不创建新的 live 序列域，避免一次 UI 自愈消耗 Aurora streamId。
        let mut buffer = Self::with_stream_id(0);
        buffer.append_chunk(&content);
        let _ = buffer.process_queue();
        let tail_block = buffer.current_tail_wire_block();
        let (_, tail_mode) = buffer.current_tail_metadata();
        AuroraRecoverySnapshot {
            stable_blocks: buffer.stable_blocks,
            tail_block,
            tail_mode,
            tail_snapshot: render_stream_snapshot(&buffer.prev_tail_ast, "t"),
        }
    }

    /// 运行块解析器，识别已闭合块和未闭合尾部
    /// 返回 (stable_changed, tail_changed)
    pub fn process_queue(&mut self) -> (bool, bool) {
        if self.is_finishing {
            return (false, false);
        }

        let prev_stable_count = self.stable_blocks.len();

        // 1. 增量解析全文，产出本次新增的已闭合块 + 尾部纯文本
        let (new_blocks, raw_new_tail, raw_tail_type) = self.parser.process(&self.full_text);
        let raw_new_tail_start = self.parser.tail_start();
        let speculative_thought = speculative_thought_tail(&raw_new_tail);
        let (new_tail, new_tail_start, next_thought_theme) = match speculative_thought {
            Some(projection) => (
                projection.content.to_string(),
                raw_new_tail_start.saturating_add(projection.content_offset),
                Some(projection.theme),
            ),
            None => (raw_new_tail, raw_new_tail_start, None),
        };
        let previous_tail_block_type = self
            .tail_projection
            .as_ref()
            .map(|projection| projection.block_type);
        let previous_thought_theme = self
            .tail_projection
            .as_ref()
            .and_then(|projection| projection.thought_theme.as_deref());
        let was_thought_tail = previous_tail_block_type == Some(TailBlockType::Thought);
        // O(1) tail 等价判定：full_text 严格只增不减（next_tail_fingerprint 内有
        // debug_assert 守护该不变量），同一 tail 起点 + 同一长度即同一内容，
        // 无需 O(tail) memcmp；theme 状态变化单独检查。
        let tail_text_changed = match self.tail_projection.as_ref() {
            Some(projection) => {
                projection.fingerprint.source_start != new_tail_start
                    || projection.fingerprint.content_len != new_tail.len()
            }
            None => !new_tail.is_empty(),
        };
        let tail_changed = tail_text_changed
            || was_thought_tail != next_thought_theme.is_some()
            || previous_thought_theme != next_thought_theme.as_deref();
        let next_fingerprint = (!new_tail.is_empty() || next_thought_theme.is_some())
            .then(|| self.next_tail_fingerprint(&new_tail, new_tail_start));

        if !new_blocks.is_empty() {
            self.stable_blocks.extend(new_blocks);
            self.tail_epoch = self.tail_epoch.saturating_add(1);
            self.tail_revision = 0;
            self.tail_reset_pending = true;
            self.pending_mutations.clear();
            self.prev_tail_ast.clear();
            self.code_highlighter.clear();
            self.html_tail_frozen_count = None;
        }

        self.tail_content = new_tail;

        // 2. 推测渲染 (Speculative Rendering)：普通 tail 使用 Markdown AST；
        //    已识别的未闭合思维链保留 Thought 外壳，但正文走纯文本增量投影（Plain
        //    模式），不做 md 解析——未闭合思维链会把 processed_len 钉在起始标记上、
        //    tail 可涨至全流级别，md AST 在此意味着每帧 O(全流) 解析。
        //    当普通 tail 显示正文超过 MAX_SPECULATIVE_TAIL_AST_BYTES 时同样跳过 AST
        //    解析降级纯文本，避免在流式热路径上产生性能悬崖
        if !self.tail_content.is_empty() || next_thought_theme.is_some() {
            if raw_tail_type == Some(BlockType::HtmlContainer) && next_thought_theme.is_none() {
                // === HtmlContainer 树权威 patch 路径（全程增量，不受投机上限约束）===
                // 后端 html5ever 规范级解析切出「新闭合根子节点 / 活跃子树」；前端对
                // 冻结段只 parse 一次并永久挂载，morphdom 只收敛活跃子树。
                let block_type = TailBlockType::Markdown; // 与原 classify_tail_block([raw_html]) 输出一致
                if previous_tail_block_type.is_some()
                    && previous_tail_block_type != Some(block_type)
                    && !self.tail_reset_pending
                {
                    self.tail_epoch = self.tail_epoch.saturating_add(1);
                    self.tail_revision = 0;
                    self.tail_reset_pending = true;
                    self.pending_mutations.clear();
                    self.code_highlighter.clear();
                    self.html_tail_frozen_count = None;
                }

                if self.tail_reset_pending {
                    // reset 帧的 snapshot 由 prev_tail_ast 构造：仅此刻构造全量节点
                    let mut node = MarkdownNode::raw_html(self.tail_content.clone());
                    node.compute_hashes_recursively();
                    self.prev_tail_ast = vec![node];
                    self.html_tail_frozen_count = None; // reset 后的下一帧按种子帧下发
                    self.tail_revision = self.tail_revision.saturating_add(1);
                } else if tail_changed {
                    // 根结构非「单个元素」时（起始标签未写完整、foster parenting 等）
                    // patch 切分不适用：回退旧的整块 Replace，并清空 patch 状态，
                    // 结构恢复后的下一帧会按种子帧重新建立基线。
                    let mutation = match html_tail_patch_parts(
                        &self.tail_content,
                        self.html_tail_frozen_count,
                    ) {
                        Some(patch) => {
                            let seed = patch.seed || self.html_tail_frozen_count.is_none();
                            let mutation = AstMutation::PatchRawHtml {
                                id: "t0".to_string(),
                                frozen_html: patch.frozen_html,
                                live_html: patch.live_html,
                                frozen_total: patch.frozen_total,
                                seed,
                            };
                            self.html_tail_frozen_count = Some(patch.frozen_total);
                            mutation
                        }
                        None => {
                            let mut node = MarkdownNode::raw_html(self.tail_content.clone());
                            node.compute_hashes_recursively();
                            self.html_tail_frozen_count = None;
                            AstMutation::Replace {
                                id: "t0".to_string(),
                                node,
                            }
                        }
                    };
                    self.prev_tail_ast.clear();

                    // 与暂存池中上一条未发送的稳态 patch 合并（冻结段有序拼接，活跃区取最新）；
                    // 种子帧与 Replace 帧语义不同（live_html 为整棵外层），绝不参与合并。
                    let mergeable =
                        matches!(&mutation, AstMutation::PatchRawHtml { seed: false, .. });
                    match (mergeable, self.pending_mutations.last_mut()) {
                        (
                            true,
                            Some(AstMutation::PatchRawHtml {
                                frozen_html: pending_frozen,
                                live_html: pending_live,
                                frozen_total: pending_total,
                                seed: false,
                                ..
                            }),
                        ) => {
                            if let AstMutation::PatchRawHtml {
                                frozen_html,
                                live_html,
                                frozen_total,
                                ..
                            } = mutation
                            {
                                pending_frozen.push_str(&frozen_html);
                                *pending_live = live_html;
                                *pending_total = frozen_total;
                            }
                        }
                        _ => self.pending_mutations.push(mutation),
                    }
                    self.tail_revision = self.tail_revision.saturating_add(1);
                }

                self.tail_projection = Some(TailProjection {
                    fingerprint: next_fingerprint.unwrap_or_else(|| {
                        TailFingerprint::from_content(&self.tail_content, new_tail_start)
                    }),
                    mode: TailRenderMode::Ast,
                    block_type,
                    thought_theme: None,
                });
            } else {
                let exceeds_speculative_cap =
                    self.tail_content.len() > MAX_SPECULATIVE_TAIL_AST_BYTES;
                // 代码围栏 / HtmlDoc / 协议块 tail 均为增量路径（O(chunk)/帧），不再受投机
                // 上限约束；上限如今只兜底仍是 O(tail)/帧 的通用 Markdown tail。
                let incremental_tail = matches!(
                    raw_tail_type,
                    Some(
                        BlockType::CodeFence
                            | BlockType::HtmlDoc
                            | BlockType::Tool
                            | BlockType::ToolResult
                            | BlockType::ToolCallSummary
                    )
                );
                let nodes = if next_thought_theme.is_some() {
                    // 思维链 tail：Thought 外壳 + 纯文本增量（TailRenderMode::Plain）。
                    // 未闭合思维链会把 processed_len 钉在起始标记上、tail 可涨至全流
                    // 级别；走 md AST 意味着每帧 O(全流) 解析，且越涨越慢。改为纯文本
                    // 投影后每帧 O(Δ)，闭合时仍由 stream 层沉淀为正式 Thought 卡片。
                    // （与桌面端「封印」语义对齐，但保留自有 Thought 外壳。）
                    None
                } else if exceeds_speculative_cap && !incremental_tail {
                    None
                } else if raw_tail_type == Some(BlockType::CodeFence) {
                    // 代码围栏 tail 快速路径：stream 层已保证 tail = 开围栏行 + 未闭合内容，
                    // 跳过整条 Markdown 预处理管线（4 道全文本预处理 + pulldown），直接构造节点
                    Some(code_fence_tail_nodes(&self.tail_content))
                } else if raw_tail_type == Some(BlockType::HtmlDoc) {
                    // HtmlDoc = 未写围栏的完整 HTML 文档，与 ```html 围栏同族：
                    // 流式期按 html 源码走增量高亮（与围栏代码块共用下游 PatchCode 管线），
                    // 闭合后由 stream 层沉淀为 HtmlPreview 卡片。
                    Some(vec![MarkdownNode::code_block(
                        Some("html".to_string()),
                        self.tail_content.clone(),
                    )])
                } else if matches!(
                    raw_tail_type,
                    Some(BlockType::Tool | BlockType::ToolResult | BlockType::ToolCallSummary)
                ) {
                    // 未闭合协议块 tail：与桌面端 VCPChat 的「封印」语义对齐——转义原文包在
                    // 等宽 pre 里逐字显示（此处借 plaintext 代码节点走增量高亮的纯文本路径）。
                    // 协议文本不是 Markdown：逐帧解析既浪费，又可能被 * / $$ 等字符意外格式化。
                    // Diary/Style 不在此列（日记正文本身是 Markdown；思维链已在上方路由进
                    // Thought 外壳纯文本投影；HtmlDoc 已路由进 html 源码增量路径）。
                    Some(vec![MarkdownNode::code_block(
                        Some("plaintext".to_string()),
                        self.tail_content.clone(),
                    )])
                } else {
                    Some(
                        crate::vcp_modules::pre_renderer::parse_markdown_to_ast_streaming(
                            &self.tail_content,
                        ),
                    )
                };
                let (mode, block_type) = if let Some(new_nodes) = nodes {
                    let block_type = next_thought_theme.as_ref().map_or_else(
                        || classify_tail_block(&new_nodes),
                        |_| TailBlockType::Thought,
                    );
                    if previous_tail_block_type.is_some()
                        && previous_tail_block_type != Some(block_type)
                        && !self.tail_reset_pending
                    {
                        self.tail_epoch = self.tail_epoch.saturating_add(1);
                        self.tail_revision = 0;
                        self.tail_reset_pending = true;
                        self.pending_mutations.clear();
                        self.code_highlighter.clear();
                    }
                    // reset 帧直接从最新 prev_tail_ast 按需构造 snapshot，其间无需保留第二份树。
                    if !self.tail_reset_pending {
                        let mutations = diff_ast_streaming(
                            &self.prev_tail_ast,
                            &new_nodes,
                            "t",
                            &mut self.code_highlighter,
                        );
                        if !mutations.is_empty() {
                            self.pending_mutations.extend(mutations);
                        }
                    } else {
                        prime_stream_code_highlighter(&new_nodes, "t", &mut self.code_highlighter);
                    }
                    self.tail_revision = self.tail_revision.saturating_add(1);
                    self.prev_tail_ast = new_nodes;
                    (TailRenderMode::Ast, block_type)
                } else {
                    // 超长 tail（> MAX_SPECULATIVE_TAIL_AST_BYTES）降级为纯文本尾部。
                    // 不再逐帧产出 AST 帧，由 tail text op 走前端纯文本路径渲染（绝不留白）。
                    // 仅在「首次从 AST 模式跨入纯文本模式」时触发一次 epoch reset 清空旧 AST 沙箱，
                    // 之后保持安静，避免逐帧 epoch 自增与空转 reset 帧。
                    let was_ast_mode = !self.prev_tail_ast.is_empty();
                    self.prev_tail_ast.clear();
                    self.pending_mutations.clear();
                    self.code_highlighter.clear();
                    if was_ast_mode && !self.tail_reset_pending {
                        self.tail_epoch = self.tail_epoch.saturating_add(1);
                        self.tail_revision = 0;
                        self.tail_reset_pending = true;
                    }
                    (
                        TailRenderMode::Plain,
                        next_thought_theme
                            .as_ref()
                            .map_or(TailBlockType::Markdown, |_| TailBlockType::Thought),
                    )
                };

                self.tail_projection = Some(TailProjection {
                    fingerprint: next_fingerprint.unwrap_or_else(|| {
                        TailFingerprint::from_content(&self.tail_content, new_tail_start)
                    }),
                    mode,
                    block_type,
                    thought_theme: next_thought_theme,
                });
            }
        } else {
            self.tail_projection = None;
            if !self.prev_tail_ast.is_empty() || !self.tail_content.is_empty() {
                self.tail_epoch = self.tail_epoch.saturating_add(1);
                self.tail_revision = 0;
                self.tail_reset_pending = true;
                self.pending_mutations.clear();
            }
            self.prev_tail_ast.clear();
            self.code_highlighter.clear();
            self.html_tail_frozen_count = None;
        }

        let stable_changed = self.stable_blocks.len() != prev_stable_count;

        (stable_changed, tail_changed)
    }

    /// 结束流：强制完成剩余内容
    pub fn finalize(&mut self) {
        if self.is_finishing {
            return;
        }
        self.is_finishing = true;
        let final_new_blocks = self.parser.finalize(&self.full_text);

        self.stable_blocks.extend(final_new_blocks);
        self.tail_content.clear();
        self.tail_projection = None;
        self.prev_tail_ast.clear();
        self.pending_mutations.clear();
        self.code_highlighter.clear();
        self.tail_epoch = self.tail_epoch.saturating_add(1);
        self.tail_revision = 0;
        self.tail_reset_pending = true;
    }
}

#[tauri::command]
pub async fn rebuild_aurora_snapshot(content: String) -> Result<AuroraRecoverySnapshot, String> {
    Ok(AuroraBuffer::compile_recovery_snapshot(content))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_tail_patch_parts_splits_frozen_and_live() {
        // 种子帧：live 为整棵最外层元素，frozen 为空
        let seed = html_tail_patch_parts("<div class=\"card\"><section>one</section><p>B", None)
            .expect("single-root tail");
        assert!(seed.seed);
        assert_eq!(seed.frozen_total, 1);
        assert!(seed.frozen_html.is_empty());
        assert!(
            seed.live_html.contains("class=\"card\""),
            "seed.live={}",
            seed.live_html
        );
        assert!(
            seed.live_html.contains("one"),
            "seed.live={}",
            seed.live_html
        );

        // 稳态帧：在种子基础上继续闭合 <p>
        let steady = html_tail_patch_parts(
            "<div class=\"card\"><section>one</section><p>B</p><span>C",
            Some(1),
        )
        .expect("steady");
        assert!(!steady.seed);
        assert_eq!(steady.frozen_total, 2);
        assert!(steady.frozen_html.contains('B') && !steady.frozen_html.contains("one"));
        assert!(steady.live_html.contains('C'));
        assert!(
            !steady.live_html.contains("class=\"card\""),
            "稳态 live 不含外层"
        );

        // 多根节点（病理/起始标签未完整）→ None，调用方回退 Replace
        assert!(html_tail_patch_parts("hello<div>x", None).is_none());
    }

    /// 构造一个超过 MAX_SPECULATIVE_TAIL_AST_BYTES 的通用 Markdown tail（无空行的巨型
    /// 单段落，永不沉淀），验证 #1c 降级行为：Snapshot 仍可按需构造纯文本 tail，
    /// 且不再逐帧自增 epoch。注意：代码围栏/协议块/HtmlContainer 已豁免该上限，
    /// 本用例必须走通用 Markdown 路径（起始行不能匹配任何特种块标记）。
    #[test]
    fn test_oversized_tail_falls_back_to_plaintext_not_blank() {
        let mut buffer = AuroraBuffer::new();
        // 无空行的超长单段落，确保整段留在通用 markdown tail；体量远超 64KB 上限
        let big = "X".repeat(MAX_SPECULATIVE_TAIL_AST_BYTES + 20_000);
        buffer.append_chunk(&big);
        buffer.process_queue();

        assert_eq!(
            buffer.tail_projection.as_ref().map(|state| state.mode),
            Some(TailRenderMode::Plain)
        );
        let tb = buffer
            .current_tail_wire_block()
            .expect("Snapshot tail 不应为空（绝不留白）");
        match tb {
            StreamBlock::Markdown { content, nodes, .. } => {
                assert!(!content.is_empty(), "降级后必须保留纯文本 content");
                assert!(nodes.is_none(), "超长 tail 应跳过 AST 解析，nodes 为 None");
            }
            other => panic!("expected markdown tail block, got {:?}", other),
        }
        // 降级后 AST 基线已清空
        assert!(buffer.prev_tail_ast.is_empty());

        // 继续追加一个 chunk：epoch 不应再逐帧自增（已处于纯文本模式，应保持安静）
        let epoch_before = buffer.tail_epoch;
        buffer.append_chunk("YYYYY");
        buffer.process_queue();
        assert_eq!(
            buffer.tail_epoch, epoch_before,
            "纯文本模式下不应逐帧自增 epoch（避免空转 reset 帧）"
        );
    }

    /// 小于上限的普通 tail 仍走 AST 路径，并只保留唯一 canonical AST。
    #[test]
    fn test_normal_tail_uses_ast() {
        let mut buffer = AuroraBuffer::new();
        buffer.append_chunk("正常一段流式文本，尚未闭合");
        let (stable_changed, tail_changed) = buffer.process_queue();
        assert!(!stable_changed);
        assert!(tail_changed);
        assert_eq!(
            buffer.tail_projection.as_ref().map(|state| state.mode),
            Some(TailRenderMode::Ast)
        );
        assert!(!buffer.prev_tail_ast.is_empty());
        assert!(buffer.prev_tail_ast[0].get_hash().is_some());
        match buffer
            .current_tail_wire_block()
            .expect("Snapshot tail 应可按需构造")
        {
            StreamBlock::Markdown { nodes, .. } => assert!(nodes.is_none()),
            other => panic!("expected markdown tail block, got {other:?}"),
        }
        let _ = buffer.take_tail_frame();

        assert_eq!(buffer.process_queue(), (false, false));
        assert!(buffer.take_tail_frame().is_none());

        buffer.append_chunk("，继续");
        assert_eq!(buffer.process_queue(), (false, true));
        let frame = buffer.take_tail_frame().expect("append tail frame");
        assert!(frame
            .mutations
            .iter()
            .any(|mutation| matches!(mutation, AstMutation::AppendText { .. })));
    }

    #[test]
    fn unclosed_think_tail_projects_empty_inner_then_appends_plain_text() {
        let mut buffer = AuroraBuffer::new();
        buffer.append_chunk("<think>");
        assert_eq!(buffer.process_queue(), (false, true));

        assert!(matches!(
            buffer.current_tail_wire_block(),
            Some(StreamBlock::Thought {
                ref theme,
                ref content,
                is_complete: false,
                ..
            }) if theme == "思维链" && content.is_empty()
        ));
        let (initial, initial_commit) =
            buffer.prepare_delta_update().expect("thought opener delta");
        assert!(matches!(
            initial.tail_op,
            Some(TailTextOp::Replace {
                ref content,
                block_type: TailBlockType::Thought,
                thought_theme: Some(ref theme),
                ..
            }) if content.is_empty() && theme == "思维链"
        ));
        buffer.commit_delivery(initial_commit);

        buffer.append_chunk("**分析中**");
        assert_eq!(buffer.process_queue(), (false, true));
        let (append, _) = buffer.prepare_delta_update().expect("thought body delta");
        // Plain 投影：Append 携带原文（** 不作 md 解析），正文不进 AST 沙箱
        assert!(matches!(
            append.tail_op,
            Some(TailTextOp::Append {
                ref content,
                mode: TailRenderMode::Plain,
                block_type: TailBlockType::Thought,
                thought_theme: Some(ref theme),
                ..
            }) if content == "**分析中**" && theme == "思维链"
        ));
        assert!(buffer.prev_tail_ast.is_empty());
    }

    #[test]
    fn completing_a_partial_think_opener_replaces_and_resets_the_tail() {
        let mut buffer = AuroraBuffer::new();
        buffer.append_chunk("<thi");
        assert_eq!(buffer.process_queue(), (false, true));
        assert!(matches!(
            buffer.current_tail_wire_block(),
            Some(StreamBlock::Markdown { .. })
        ));
        let (_, markdown_commit) = buffer.prepare_delta_update().expect("partial opener delta");
        buffer.commit_delivery(markdown_commit);

        buffer.append_chunk("nk>");
        assert_eq!(buffer.process_queue(), (false, true));
        let (thought_update, _) = buffer
            .prepare_delta_update()
            .expect("completed opener delta");
        assert!(matches!(
            thought_update.tail_op,
            Some(TailTextOp::Replace {
                ref content,
                block_type: TailBlockType::Thought,
                ..
            }) if content.is_empty()
        ));
        assert!(matches!(
            thought_update.tail_frame,
            Some(TailFrame {
                reset: true,
                snapshot: Some(ref nodes),
                ..
            }) if nodes.is_empty()
        ));
    }

    #[test]
    fn vcp_thought_recovery_keeps_theme_and_strips_only_the_opener() {
        let raw = "[--- VCP元思考链: \"规划\" ---]\n- 第一步";
        let snapshot = AuroraBuffer::compile_recovery_snapshot(raw.to_string());

        assert!(matches!(
            snapshot.tail_block,
            Some(StreamBlock::Thought {
                ref theme,
                ref content,
                is_complete: false,
                ..
            }) if theme == "规划" && content == "\n- 第一步"
        ));
        // Plain 投影：无 AST 快照，mode 为 Plain
        assert!(snapshot.tail_snapshot.is_empty());
        assert_eq!(snapshot.tail_mode, Some(TailRenderMode::Plain));
    }

    #[test]
    fn strict_thought_close_moves_one_complete_block_out_of_the_tail() {
        let mut buffer = AuroraBuffer::new();
        buffer.append_chunk("<thinking>分析");
        assert_eq!(buffer.process_queue(), (false, true));
        let (_, commit) = buffer.prepare_delta_update().expect("open thought delta");
        buffer.commit_delivery(commit);

        buffer.append_chunk("完成</thinking>\n\n最终回答");
        assert_eq!(buffer.process_queue(), (true, true));
        assert!(matches!(
            buffer.stable_blocks.as_slice(),
            [StreamBlock::Thought {
                content,
                is_complete: true,
                ..
            }] if content == "分析完成"
        ));
        assert!(matches!(
            buffer.current_tail_wire_block(),
            Some(StreamBlock::Markdown { ref content, .. }) if content.contains("最终回答")
        ));

        let (handoff, _) = buffer
            .prepare_delta_update()
            .expect("closed thought handoff");
        assert!(matches!(
            handoff.stable_append,
            Some(StableAppend { ref blocks, .. })
                if matches!(blocks.as_slice(), [StreamBlock::Thought { is_complete: true, .. }])
        ));
        assert!(matches!(
            handoff.tail_op,
            Some(TailTextOp::Replace {
                block_type: TailBlockType::Markdown,
                ..
            })
        ));
    }

    #[test]
    fn finalizing_an_unclosed_thought_restores_the_raw_markdown_tail() {
        let raw = "<think>未完成的 **分析**";
        let mut buffer = AuroraBuffer::new();
        buffer.append_chunk(raw);
        assert_eq!(buffer.process_queue(), (false, true));
        assert!(matches!(
            buffer.current_tail_wire_block(),
            Some(StreamBlock::Thought {
                is_complete: false,
                ..
            })
        ));

        buffer.finalize();
        assert!(matches!(
            buffer.stable_blocks.as_slice(),
            [StreamBlock::Markdown { content, .. }] if content == raw
        ));
        assert!(buffer.current_tail_wire_block().is_none());
    }

    #[test]
    fn thought_tail_is_always_plain_text_regardless_of_size() {
        // 小体量思维链：同样走纯文本投影，不做 md AST（** 保持原文）
        let mut small = AuroraBuffer::new();
        small.append_chunk("<think>**加粗不应被解析**");
        assert_eq!(small.process_queue(), (false, true));
        assert_eq!(
            small.tail_projection.as_ref().map(|p| p.mode),
            Some(TailRenderMode::Plain)
        );
        assert!(small.prev_tail_ast.is_empty());
        assert!(matches!(
            small.current_tail_wire_block(),
            Some(StreamBlock::Thought {
                ref content,
                is_complete: false,
                ..
            }) if content == "**加粗不应被解析**"
        ));

        // 超限思维链：不触发 64KB 降级（它本就恒为 Plain），且不受上限约束继续增量
        let mut buffer = AuroraBuffer::new();
        let body = "长".repeat(MAX_SPECULATIVE_TAIL_AST_BYTES + 1);
        buffer.append_chunk(&format!("<think>{body}"));
        assert_eq!(buffer.process_queue(), (false, true));

        assert_eq!(
            buffer
                .tail_projection
                .as_ref()
                .map(|projection| projection.mode),
            Some(TailRenderMode::Plain)
        );
        assert!(matches!(
            buffer.current_tail_wire_block(),
            Some(StreamBlock::Thought {
                ref content,
                is_complete: false,
                ..
            }) if content == &body
        ));

        // 继续追加仍走 O(Δ) Append
        let (_, commit) = buffer
            .prepare_delta_update()
            .expect("initial thought delta");
        buffer.commit_delivery(commit);
        buffer.append_chunk("尾注");
        assert_eq!(buffer.process_queue(), (false, true));
        let (append, _) = buffer.prepare_delta_update().expect("append delta");
        assert!(matches!(
            append.tail_op,
            Some(TailTextOp::Append {
                ref content,
                block_type: TailBlockType::Thought,
                ..
            }) if content == "尾注"
        ));
    }

    #[test]
    fn unclosed_html_fence_is_speculated_and_finalized_as_code_block() {
        let mut buffer = AuroraBuffer::new();
        buffer.append_chunk(
            "```html\n<!DOCTYPE html>\n<html>\n<head>\n<style>body { color: red; }</style>\n</head>",
        );
        assert_eq!(buffer.process_queue(), (false, true));

        assert!(matches!(
            buffer.prev_tail_ast.as_slice(),
            [MarkdownNode::CodeBlock { lang, code, .. }]
                if lang.as_deref() == Some("html")
                    && code.contains("<!DOCTYPE html>")
                    && code.contains("<style>")
        ));
        assert!(matches!(
            buffer.current_tail_wire_block(),
            Some(StreamBlock::HtmlPreview {
                highlighted_content: None,
                ..
            })
        ));

        buffer.finalize();
        assert!(matches!(
            buffer.stable_blocks.as_slice(),
            [StreamBlock::Markdown { nodes: Some(nodes), .. }]
                if matches!(
                    nodes.as_slice(),
                    [MarkdownNode::CodeBlock { lang, code, .. }]
                        if lang.as_deref() == Some("html")
                            && code.contains("<!DOCTYPE html>")
                            && code.contains("<style>")
                )
        ));
    }

    #[test]
    fn recognizing_an_html_tail_resets_the_ast_for_the_new_vue_shell() {
        let mut buffer = AuroraBuffer::new();
        buffer.append_chunk("```ht");
        assert_eq!(buffer.process_queue(), (false, true));
        assert!(matches!(
            buffer.current_tail_wire_block(),
            Some(StreamBlock::Markdown { .. })
        ));
        let (_, first_commit) = buffer.prepare_delta_update().expect("partial fence delta");
        buffer.commit_delivery(first_commit);

        buffer.append_chunk("ml\n<main>streaming");
        assert_eq!(buffer.process_queue(), (false, true));
        let (html_update, _) = buffer.prepare_delta_update().expect("HTML tail delta");
        assert!(matches!(
            html_update.tail_op,
            Some(TailTextOp::Replace {
                block_type: TailBlockType::HtmlPreview,
                ..
            })
        ));
        assert!(matches!(
            html_update.tail_frame,
            Some(TailFrame {
                reset: true,
                snapshot: Some(ref nodes),
                ..
            }) if matches!(
                nodes.as_slice(),
                [MarkdownNode::CodeBlock { lang: Some(lang), .. }]
                    if lang.eq_ignore_ascii_case("html")
            )
        ));
    }

    #[test]
    fn large_html_fence_streams_highlight_patches_then_hands_off_to_markdown() {
        let mut buffer = AuroraBuffer::new();
        let completed_lines = "<div class=\"row\">value</div>\n".repeat(180);
        let initial = format!("```html\n{completed_lines}<span>tail");
        assert!(initial.len() > 4096);

        buffer.append_chunk(&initial);
        assert_eq!(buffer.process_queue(), (false, true));
        let (initial_update, initial_commit) = buffer
            .prepare_delta_update()
            .expect("initial highlighted delta");
        assert!(matches!(
            initial_update.tail_op,
            Some(TailTextOp::Replace {
                block_type: TailBlockType::HtmlPreview,
                ..
            })
        ));
        assert_eq!(
            serde_json::to_value(&initial_update).expect("serialize HTML tail")["tailOp"]
                ["blockType"],
            "html-preview"
        );
        let initial_frame = initial_update
            .tail_frame
            .as_ref()
            .expect("initial highlighted frame");
        assert!(matches!(
            initial_frame.mutations.as_slice(),
            [AstMutation::Add {
                node: MarkdownNode::CodeBlock {
                    highlighted_html: Some(html),
                    ..
                },
                ..
            }] if html.contains("data-vcp-stream-code")
        ));
        buffer.commit_delivery(initial_commit);
        assert!(matches!(
            buffer.prev_tail_ast.as_slice(),
            [MarkdownNode::CodeBlock {
                highlighted_html: None,
                ..
            }]
        ));

        buffer.append_chunk("中文说明</span>");
        assert_eq!(buffer.process_queue(), (false, true));
        let active_frame = buffer.take_tail_frame().expect("active line patch");
        assert!(matches!(
            active_frame.mutations.as_slice(),
            [AstMutation::PatchCode {
                completed_html,
                active_html,
                ..
            }] if completed_html.is_empty() && active_html.contains("中文说明")
        ));

        buffer.append_chunk("\n<p>next");
        assert_eq!(buffer.process_queue(), (false, true));
        let newline_frame = buffer.take_tail_frame().expect("completed line patch");
        assert!(matches!(
            newline_frame.mutations.as_slice(),
            [AstMutation::PatchCode {
                completed_html,
                active_html,
                ..
            }] if completed_html.contains("中文说明") && active_html.contains("next")
        ));

        buffer.append_chunk("\n```\n围栏后的解释");
        assert_eq!(buffer.process_queue(), (true, true));
        assert!(matches!(
            buffer.stable_blocks.as_slice(),
            [StreamBlock::HtmlPreview {
                content,
                highlighted_content: Some(html),
                ..
            }] if content.contains("中文说明") && html.contains("vcp-html-block")
        ));
        let handoff = buffer.take_tail_frame().expect("markdown handoff snapshot");
        assert!(handoff.reset);
        assert!(matches!(
            handoff.snapshot.as_deref(),
            Some([MarkdownNode::Paragraph { .. }])
        ));
    }

    #[test]
    fn raw_html_tail_streams_patch_raw_html_with_freeze_frontier() {
        let mut buffer = AuroraBuffer::new();
        buffer.append_chunk("<div class=\"card\">\n<section>one");
        assert_eq!(buffer.process_queue(), (false, true));

        assert_eq!(
            buffer.tail_projection.as_ref().map(|state| state.mode),
            Some(TailRenderMode::Ast)
        );
        // patch 路径不保留 AST（快照需要时由 reset 分支重建）
        assert!(buffer.prev_tail_ast.is_empty());

        // 第一帧：种子帧（外层起始标签已完整），整棵外层下发
        let frame = buffer.take_tail_frame().expect("seed patch frame");
        assert!(!frame.reset);
        match frame.mutations.as_slice() {
            [AstMutation::PatchRawHtml {
                id,
                frozen_html,
                live_html,
                frozen_total,
                seed,
            }] => {
                assert_eq!(id, "t0");
                assert!(seed, "首帧应为种子帧");
                assert!(frozen_html.is_empty());
                assert!(
                    live_html.contains("class=\"card\""),
                    "seed live={live_html}"
                );
                assert!(live_html.contains("one"));
                assert_eq!(*frozen_total, 1); // [text"\n", section] → 冻结 1（text）
            }
            other => panic!("expected single patch_raw_html seed, got {other:?}"),
        }

        // 无变化 → 无新帧
        assert_eq!(buffer.process_queue(), (false, false));
        assert!(buffer.take_tail_frame().is_none());

        // section 闭合 + 新段落活跃：冻结段推进
        buffer.append_chunk("</section>\n<p>two");
        assert_eq!(buffer.process_queue(), (false, true));
        let frame = buffer.take_tail_frame().expect("advance frame");
        match frame.mutations.as_slice() {
            [AstMutation::PatchRawHtml {
                frozen_html,
                live_html,
                frozen_total,
                seed,
                ..
            }] => {
                assert!(!seed);
                assert!(frozen_html.contains("one"), "frozen={frozen_html}");
                assert!(live_html.contains("two"), "live={live_html}");
                assert!(!live_html.contains("class=\"card\""), "稳态 live 不含外层");
                assert_eq!(*frozen_total, 3); // text/section/text 冻结，p 活跃
            }
            other => panic!("expected patch_raw_html, got {other:?}"),
        }
    }

    /// 持续存在的 HtmlContainer tail 不再受 64KB 投机上限约束：超限后仍走
    /// PatchRawHtml 增量帧，不降级为纯文本。
    #[test]
    fn raw_html_exceeding_speculative_limit_stays_incremental() {
        // 外层 div 永不闭合，内容远超 64KB
        let big = format!(
            "<div class=\"big\">{}",
            "X".repeat(MAX_SPECULATIVE_TAIL_AST_BYTES + 20_000)
        );
        let mut buffer = AuroraBuffer::new();
        buffer.append_chunk(&big);
        assert_eq!(buffer.process_queue(), (false, true));

        assert_eq!(
            buffer.tail_projection.as_ref().map(|state| state.mode),
            Some(TailRenderMode::Ast),
            "HtmlContainer tail 超限后必须保持增量 AST 模式"
        );
        let frame = buffer.take_tail_frame().expect("patch frame over limit");
        assert!(matches!(
            frame.mutations.as_slice(),
            [AstMutation::PatchRawHtml { seed: true, .. }]
        ));

        // 继续追加：稳态 patch，冻结数不增（外层唯一子节点=文本，仍活跃）
        buffer.append_chunk("YYYYY");
        assert_eq!(buffer.process_queue(), (false, true));
        let frame = buffer
            .take_tail_frame()
            .expect("steady patch frame over limit");
        match frame.mutations.as_slice() {
            [AstMutation::PatchRawHtml {
                seed, live_html, ..
            }] => {
                assert!(!seed);
                assert!(live_html.contains("YYYYY"), "live 应携带最新追加文本");
            }
            other => panic!("expected steady patch_raw_html, got {other:?}"),
        }
    }

    #[test]
    fn tail_frame_sequence_is_namespaced_by_buffer_stream_id() {
        let mut first = AuroraBuffer::new();
        first.append_chunk("a");
        first.process_queue();
        let first_frame = first.take_tail_frame().expect("first tail frame");
        assert_eq!(first_frame.stream_id, first.stream_id);
        assert_eq!(first_frame.frame_seq, 1);
        let wire = serde_json::to_value(&first_frame).expect("serialize tail frame");
        assert_eq!(wire["streamId"], first.stream_id);

        first.append_chunk("b");
        first.process_queue();
        let second_frame = first.take_tail_frame().expect("second tail frame");
        assert_eq!(second_frame.stream_id, first_frame.stream_id);
        assert_eq!(second_frame.frame_seq, 2);

        let second = AuroraBuffer::new();
        assert_ne!(second.stream_id, first.stream_id);
    }

    #[test]
    fn recovery_baseline_covers_full_content_without_replaying_it_as_a_chunk() {
        let mut buffer = AuroraBuffer::new();
        buffer.append_chunk("authoritative");
        buffer.process_queue();

        let baseline = buffer.take_recovery_baseline();
        assert_eq!(baseline.kind, AuroraUpdateKind::Snapshot);
        assert_eq!(baseline.stream_id, Some(buffer.stream_id));
        assert_eq!(baseline.content.as_deref(), Some("authoritative"));
        assert!(baseline.stable_blocks.as_ref().is_some_and(Vec::is_empty));
        match baseline
            .tail_block
            .as_ref()
            .expect("authoritative tail block")
        {
            StreamBlock::Markdown { content, nodes, .. } => {
                assert_eq!(content, "authoritative");
                assert!(
                    nodes.is_none(),
                    "Snapshot AST 只应由 tailFrame.snapshot 承载"
                );
            }
            other => panic!("expected markdown recovery tail, got {other:?}"),
        }
        assert_eq!(baseline.tail_mode, Some(TailRenderMode::Ast));
        let baseline_frame = baseline.tail_frame.as_ref().expect("reset baseline frame");
        assert!(baseline_frame.reset);
        assert_eq!(baseline_frame.frame_seq, 1);
        assert!(baseline_frame
            .snapshot
            .as_ref()
            .is_some_and(|nodes| !nodes.is_empty()));
        assert!(baseline.chunk.is_none());
        let wire = serde_json::to_value(&baseline).expect("serialize recovery baseline");
        assert_eq!(wire["kind"], "snapshot");
        assert!(wire.get("tail").is_none());
        assert!(wire.get("tailSnapshot").is_none());
        assert!(buffer.take_chunk().is_none());

        buffer.append_chunk("!");
        buffer.process_queue();
        let next = buffer.take_tail_frame().expect("post-baseline tail frame");
        assert_eq!(next.stream_id, buffer.stream_id);
        assert_eq!(next.frame_seq, 2);
        assert_eq!(buffer.take_chunk().as_deref(), Some("!"));
    }

    #[test]
    fn reset_snapshot_uses_the_latest_canonical_ast_without_a_mirror() {
        let mut buffer = AuroraBuffer::new();
        buffer.append_chunk("stable\n\ntail");
        assert_eq!(buffer.process_queue(), (true, true));
        assert!(buffer.tail_reset_pending);

        buffer.append_chunk(" grows");
        assert_eq!(buffer.process_queue(), (false, true));
        let canonical = buffer.prev_tail_ast.clone();
        let frame = buffer.take_tail_frame().expect("pending reset frame");

        assert!(frame.reset);
        assert_eq!(frame.snapshot.as_deref(), Some(canonical.as_slice()));
        assert!(frame.mutations.is_empty());
    }

    #[test]
    fn tail_fingerprint_matches_one_shot_sha256_across_utf8_appends() {
        let mut buffer = AuroraBuffer::new();
        for chunk in ["你", "好", "🙂", "，stream"] {
            buffer.append_chunk(chunk);
            buffer.process_queue();
            let (hash, _) = buffer.current_tail_metadata();
            let expected =
                crate::vcp_modules::infra::utils::calculate_sha256(buffer.tail_content.as_bytes());
            assert_eq!(hash.as_deref(), Some(expected.as_str()));
        }
    }

    #[test]
    fn tail_fingerprint_rebuilds_after_stable_prefix_precipitates() {
        let mut buffer = AuroraBuffer::new();
        buffer.append_chunk("stable\n\n尾");
        assert_eq!(buffer.process_queue(), (true, true));
        let (first_hash, _) = buffer.current_tail_metadata();
        assert_eq!(
            first_hash,
            Some(crate::vcp_modules::infra::utils::calculate_sha256(
                "尾".as_bytes()
            ))
        );

        buffer.append_chunk("🙂");
        assert_eq!(buffer.process_queue(), (false, true));
        let live_tail = buffer
            .current_tail_wire_block()
            .expect("live tail snapshot block");
        let recovery = AuroraBuffer::compile_recovery_snapshot(buffer.full_text.clone())
            .tail_block
            .expect("recovery tail snapshot block");
        assert_eq!(
            serde_json::to_value(live_tail).expect("serialize live tail")["hash"],
            serde_json::to_value(recovery).expect("serialize recovery tail")["hash"]
        );
    }

    #[test]
    fn normal_delivery_contains_only_tail_delta_and_ast_mutations() {
        let mut buffer = AuroraBuffer::new();
        buffer.append_chunk("hello");
        buffer.process_queue();

        let (first, first_commit) = buffer.prepare_delta_update().expect("first delta update");
        assert_eq!(first.kind, AuroraUpdateKind::Delta);
        assert!(first.content.is_none());
        assert!(first.stable_blocks.is_none());
        assert!(first.tail_block.is_none());
        assert!(matches!(
            first.tail_op,
            Some(TailTextOp::Replace { ref content, .. }) if content == "hello"
        ));
        assert!(first.tail_frame.is_some());
        let wire = serde_json::to_value(&first).expect("serialize first delta");
        assert!(wire.get("tailBlock").is_none());
        assert!(wire.get("stableBlocks").is_none());
        assert!(wire.get("content").is_none());

        // prepare 本身不消费任何增量；只有发送成功后的 commit 才推进 wire cursor。
        assert_eq!(buffer.pushed_len, 0);
        assert!(!buffer.pending_mutations.is_empty());
        let retry_wire = serde_json::to_value(
            &buffer
                .prepare_delta_update()
                .expect("retry delta remains pending")
                .0,
        )
        .expect("serialize retry delta");
        assert_eq!(wire, retry_wire);
        buffer.commit_delivery(first_commit);
        assert_eq!(buffer.pushed_len, 5);
        assert!(buffer.pending_mutations.is_empty());

        buffer.append_chunk("!");
        buffer.process_queue();
        let (second, _) = buffer.prepare_delta_update().expect("second delta update");
        assert_eq!(second.chunk.as_deref(), Some("!"));
        assert!(matches!(
            second.tail_op,
            Some(TailTextOp::Append { ref content, .. }) if content == "!"
        ));
        let second_wire = serde_json::to_value(&second).expect("serialize append delta");
        assert_eq!(
            second_wire["tailOp"]["previousHash"], wire["tailOp"]["hash"],
            "Rust append previous hash must use the frontend camelCase contract"
        );
    }

    #[test]
    fn stable_delivery_appends_only_newly_precipitated_blocks() {
        let mut buffer = AuroraBuffer::new();
        buffer.append_chunk("first\n\nsecond");
        let (stable_changed, _) = buffer.process_queue();
        assert!(stable_changed);
        let (first, first_commit) = buffer.prepare_delta_update().expect("first stable delta");
        let first_append = first.stable_append.expect("first stable append");
        assert_eq!(first_append.base_count, 0);
        assert_eq!(first_append.blocks.len(), 1);
        buffer.commit_delivery(first_commit);

        buffer.append_chunk("\n\nthird");
        let (stable_changed, _) = buffer.process_queue();
        assert!(stable_changed);
        let (second, _) = buffer.prepare_delta_update().expect("second stable delta");
        let second_append = second.stable_append.expect("second stable append");
        assert_eq!(second_append.base_count, 1);
        assert_eq!(second_append.blocks.len(), 1);
        assert!(second.stable_blocks.is_none());
    }

    #[test]
    fn recovery_compiler_returns_one_canonical_tail_snapshot() {
        let snapshot = AuroraBuffer::compile_recovery_snapshot("hello".to_string());
        assert!(snapshot.stable_blocks.is_empty());
        assert_eq!(snapshot.tail_mode, Some(TailRenderMode::Ast));
        assert!(!snapshot.tail_snapshot.is_empty());
        match snapshot.tail_block.expect("tail block") {
            StreamBlock::Markdown { content, nodes, .. } => {
                assert_eq!(content, "hello");
                assert!(nodes.is_none());
            }
            other => panic!("expected markdown recovery tail, got {other:?}"),
        }
    }

    #[test]
    fn code_fence_tail_nodes_matches_pulldown_semantics() {
        // 快速路径与 pulldown 全管线逐字节一致（lang trim / 空 info / CRLF / 4+ 反引号 / 未换行 opener）
        let cases = [
            "```html\n<div>hi</div>\n",
            "```rust\nfn main() {}\n}",
            "```\nplain\n",
            "````markdown\n```nested```\n",
            "```html",
            "```html\n",
            "```js\r\nlet a = 1;\r\n",
        ];
        for tail in cases {
            let fast = code_fence_tail_nodes(tail);
            let reference = crate::vcp_modules::pre_renderer::parse_markdown_to_ast_streaming(tail);
            let (
                [MarkdownNode::CodeBlock {
                    lang: fast_lang,
                    code: fast_code,
                    ..
                }],
                [MarkdownNode::CodeBlock {
                    lang: ref_lang,
                    code: ref_code,
                    ..
                }],
            ) = (fast.as_slice(), reference.as_slice())
            else {
                panic!("both paths must yield exactly one CodeBlock for {tail:?}: {reference:?}");
            };
            assert_eq!(fast_code, ref_code, "code mismatch for {tail:?}");
            assert_eq!(fast_lang, ref_lang, "lang mismatch for {tail:?}");
            assert_eq!(
                fast[0].get_hash(),
                None,
                "tail 节点不计算必然 miss 的 hash: {tail:?}"
            );
        }
    }

    #[test]
    fn code_fence_tail_fast_path_drives_incremental_patch_flow() {
        // 快速路径产出的节点必须无缝接入既有增量高亮管线：首帧 Add，严格追加帧 PatchCode
        let mut buffer = AuroraBuffer::new();
        buffer.append_chunk("```html\n<div>hi</div>\n");
        buffer.process_queue();
        assert_eq!(
            buffer.tail_projection.as_ref().map(|p| p.block_type),
            Some(TailBlockType::HtmlPreview)
        );
        let first = buffer.take_tail_frame().expect("first frame");
        assert!(
            matches!(first.mutations.as_slice(), [AstMutation::Add { .. }]),
            "first frame must Add the code node, got {:?}",
            first.mutations
        );

        buffer.append_chunk("<p>more</p>\n");
        buffer.process_queue();
        let second = buffer.take_tail_frame().expect("second frame");
        assert!(
            matches!(second.mutations.as_slice(), [AstMutation::PatchCode { .. }]),
            "strict append must produce PatchCode, got {:?}",
            second.mutations
        );
    }

    #[test]
    fn html_doc_tail_streams_as_html_code_incrementally() {
        // HtmlDoc（未写围栏的完整 HTML 文档）与 ```html 围栏同族：
        // 流式期投影为 html 源码 CodeBlock，走同一增量高亮管线。
        let mut buffer = AuroraBuffer::new();
        buffer.append_chunk("<!DOCTYPE html>\n<html>\n<body>\n<p>hi");
        buffer.process_queue();
        assert_eq!(
            buffer.tail_projection.as_ref().map(|p| p.block_type),
            Some(TailBlockType::HtmlPreview)
        );
        match &buffer.prev_tail_ast[0] {
            MarkdownNode::CodeBlock { lang, code, .. } => {
                assert_eq!(lang.as_deref(), Some("html"));
                assert!(code.contains("<!DOCTYPE html>") && code.contains("<p>hi"));
            }
            other => panic!("expected html CodeBlock projection, got {other:?}"),
        }
        let first = buffer.take_tail_frame().expect("first frame");
        assert!(
            matches!(first.mutations.as_slice(), [AstMutation::Add { .. }]),
            "first frame must Add the html code node, got {:?}",
            first.mutations
        );

        buffer.append_chunk("</p>\n<p>more");
        buffer.process_queue();
        let second = buffer.take_tail_frame().expect("second frame");
        assert!(
            matches!(second.mutations.as_slice(), [AstMutation::PatchCode { .. }]),
            "strict append must produce PatchCode, got {:?}",
            second.mutations
        );

        // 闭合 </html>：沉淀为 HtmlPreview stable 块，tail 清空
        buffer.append_chunk("</p>\n</body>\n</html>");
        let (stable_changed, _) = buffer.process_queue();
        assert!(
            stable_changed,
            "html doc close must precipitate a stable block"
        );
        assert_eq!(buffer.stable_blocks.len(), 1);
        assert!(buffer.tail_content.is_empty());
    }

    /// HtmlDoc tail 与代码围栏同族，不受 64KB 投机上限约束：超限后仍走增量高亮，
    /// 不降级为纯文本。
    #[test]
    fn html_doc_exceeding_speculative_limit_stays_incremental() {
        let big = format!(
            "<!DOCTYPE html>\n<html>\n<body>\n{}",
            "X".repeat(MAX_SPECULATIVE_TAIL_AST_BYTES + 20_000)
        );
        let mut buffer = AuroraBuffer::new();
        buffer.append_chunk(&big);
        assert_eq!(buffer.process_queue(), (false, true));

        assert_eq!(
            buffer.tail_projection.as_ref().map(|state| state.mode),
            Some(TailRenderMode::Ast),
            "HtmlDoc tail 超限后必须保持增量 AST 模式"
        );
        assert_eq!(
            buffer.tail_projection.as_ref().map(|p| p.block_type),
            Some(TailBlockType::HtmlPreview)
        );
        assert!(buffer.take_tail_frame().is_some());
    }

    #[test]
    fn unclosed_protocol_tail_projects_as_plaintext_code_node() {
        // 与桌面端 VCPChat「封印」语义对齐：未闭合协议块 tail 原样投影为 plaintext
        // 代码节点，不经过 Markdown 管线（协议文本里的 * / $$ 等字符不得被格式化）
        let mut buffer = AuroraBuffer::new();
        buffer.append_chunk(
            "<<<[TOOL_REQUEST]>>>\ntool_name:「始」Diary「末」\ncontent:「始」*粗体* $$x$$ 保持字面",
        );
        buffer.process_queue();

        assert_eq!(buffer.prev_tail_ast.len(), 1);
        match &buffer.prev_tail_ast[0] {
            MarkdownNode::CodeBlock { lang, code, .. } => {
                assert_eq!(lang.as_deref(), Some("plaintext"));
                assert_eq!(
                    code,
                    "<<<[TOOL_REQUEST]>>>\ntool_name:「始」Diary「末」\ncontent:「始」*粗体* $$x$$ 保持字面"
                );
            }
            other => panic!("expected plaintext CodeBlock projection, got {other:?}"),
        }
        let first = buffer.take_tail_frame().expect("first frame");
        assert!(
            matches!(first.mutations.as_slice(), [AstMutation::Add { .. }]),
            "first frame must Add the plaintext node, got {:?}",
            first.mutations
        );

        // 增量补丁流仍然工作：追加内容产出 PatchCode 而非整树重建
        buffer.append_chunk("\n更多内容");
        buffer.process_queue();
        let second = buffer.take_tail_frame().expect("second frame");
        assert!(
            matches!(second.mutations.as_slice(), [AstMutation::PatchCode { .. }]),
            "protocol tail must flow through incremental code path, got {:?}",
            second.mutations
        );
    }
}

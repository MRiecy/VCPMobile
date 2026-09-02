use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::vcp_modules::chat::ast_diff::{
    diff_ast_streaming, prime_stream_code_highlighter, render_stream_snapshot, AstMutation,
};
use crate::vcp_modules::pre_renderer::code_highlighter::IncrementalCodeHighlighter;
use crate::vcp_modules::pre_renderer::markdown_ast::MarkdownNode;
use crate::vcp_modules::stream_block_parser::{StreamBlock, StreamBlockParser};

/// 推测渲染的 tail 字节上限：超过此阈值跳过 AST 解析，降级为纯文本尾部。
///
/// 取值依据（perf profile 基准，见 ast_bench.rs，约等于发布版热路径速度）：
/// - 解析本身极廉价：40KB tail 的 parse+hash+diff+serialize 仅约 0.55ms，远非瓶颈。
/// - CodeBlock 严格追加已改为完成行/活跃行补丁；RawHtml 等不可拆节点仍会整块 Replace。
///   因此保留 65536 上限覆盖绝大多数真实 HTML/代码产物，同时约束通用 AST Snapshot，
///   并由 vcp_client 固定 30Hz 合并发送，避免按上游 token 粒度重复解析与推送。
///   仅在 tail 超过 64KB 这种极端体量时才降级为纯文本，避免单帧 JSON 过大拖垮 webview。
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
    },
    Replace {
        content: String,
        hash: String,
        mode: TailRenderMode,
        #[serde(rename = "blockType")]
        block_type: TailBlockType,
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
    /// Delta 中对 tail 原文的追加、替换或清空操作。
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
}

fn classify_tail_block(nodes: &[MarkdownNode]) -> TailBlockType {
    match nodes {
        [MarkdownNode::CodeBlock {
            lang: Some(lang), ..
        }] if lang.eq_ignore_ascii_case("html") => TailBlockType::HtmlPreview,
        _ => TailBlockType::Markdown,
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
        let current_tail_source_start = self.current_tail_source_start();
        let tail_state_changed = self.tail_content.len() != self.pushed_tail_len
            || self.tail_epoch != self.pushed_tail_epoch
            || current_tail_hash != self.pushed_tail_hash
            || current_tail_mode != self.pushed_tail_mode
            || current_tail_block_type != self.pushed_tail_block_type;
        let tail_op = if !tail_state_changed {
            None
        } else if self.tail_content.is_empty() {
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
                    })
                } else {
                    Some(TailTextOp::Replace {
                        content: self.tail_content.clone(),
                        hash,
                        mode,
                        block_type,
                    })
                }
            } else {
                Some(TailTextOp::Replace {
                    content: self.tail_content.clone(),
                    hash,
                    mode,
                    block_type,
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
        let (new_blocks, new_tail) = self.parser.process(&self.full_text);
        let new_tail_start = self.parser.tail_start();
        let tail_changed = self.tail_content != new_tail;
        let previous_tail_block_type = self
            .tail_projection
            .as_ref()
            .map(|projection| projection.block_type);
        let next_fingerprint =
            (!new_tail.is_empty()).then(|| self.next_tail_fingerprint(&new_tail, new_tail_start));

        if !new_blocks.is_empty() {
            self.stable_blocks.extend(new_blocks);
            self.tail_epoch = self.tail_epoch.saturating_add(1);
            self.tail_revision = 0;
            self.tail_reset_pending = true;
            self.pending_mutations.clear();
            self.prev_tail_ast.clear();
            self.code_highlighter.clear();
        }

        self.tail_content = new_tail;

        // 2. 推测渲染 (Speculative Rendering)：将 tail 视为一个临时 Markdown 块
        //    当 tail 超过 MAX_SPECULATIVE_TAIL_AST_BYTES 时跳过 AST 解析，
        //    避免在流式热路径上产生性能悬崖
        if !self.tail_content.is_empty() {
            let nodes = if self.tail_content.len() > MAX_SPECULATIVE_TAIL_AST_BYTES {
                None
            } else {
                Some(
                    crate::vcp_modules::pre_renderer::parse_markdown_to_ast_streaming(
                        &self.tail_content,
                    ),
                )
            };
            let (mode, block_type) = if let Some(new_nodes) = nodes {
                let block_type = classify_tail_block(&new_nodes);
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
                (TailRenderMode::Plain, TailBlockType::Markdown)
            };

            self.tail_projection = Some(TailProjection {
                fingerprint: next_fingerprint.unwrap_or_else(|| {
                    TailFingerprint::from_content(&self.tail_content, new_tail_start)
                }),
                mode,
                block_type,
            });
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

    /// 构造一个超过 MAX_SPECULATIVE_TAIL_AST_BYTES 的、非 HTML 起始的纯文本代码块 tail，
    /// 验证 #1c 降级行为：Snapshot 仍可按需构造纯文本 tail，且不再逐帧自增 epoch。
    #[test]
    fn test_oversized_tail_falls_back_to_plaintext_not_blank() {
        let mut buffer = AuroraBuffer::new();
        // 未闭合代码围栏，确保整段留在 tail；体量远超 64KB 上限
        let big = format!(
            "```text\n{}",
            "X".repeat(MAX_SPECULATIVE_TAIL_AST_BYTES + 20_000)
        );
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
    fn raw_html_hashes_the_single_canonical_diff_baseline() {
        let mut buffer = AuroraBuffer::new();
        buffer.append_chunk("<div>one");
        assert_eq!(buffer.process_queue(), (false, true));

        assert_eq!(
            buffer.tail_projection.as_ref().map(|state| state.mode),
            Some(TailRenderMode::Ast)
        );
        assert!(buffer.prev_tail_ast[0].get_hash().is_some());
        let _ = buffer.take_tail_frame();

        assert_eq!(buffer.process_queue(), (false, false));
        assert!(buffer.take_tail_frame().is_none());

        buffer.append_chunk("two");
        assert_eq!(buffer.process_queue(), (false, true));
        let frame = buffer.take_tail_frame().expect("raw replace frame");
        assert!(frame
            .mutations
            .iter()
            .any(|mutation| matches!(mutation, AstMutation::Replace { .. })));
    }

    #[test]
    fn raw_html_obeys_the_speculative_ast_limit() {
        let raw_html_at_limit = format!(
            "<div>{}",
            "X".repeat(MAX_SPECULATIVE_TAIL_AST_BYTES - "<div>".len())
        );
        let mut at_limit = AuroraBuffer::new();
        at_limit.append_chunk(&raw_html_at_limit);
        assert_eq!(at_limit.process_queue(), (false, true));
        assert_eq!(
            at_limit.tail_projection.as_ref().map(|state| state.mode),
            Some(TailRenderMode::Ast)
        );
        match at_limit
            .current_tail_wire_block()
            .expect("raw tail at limit")
        {
            StreamBlock::Markdown { content, nodes, .. } => {
                assert_eq!(content, raw_html_at_limit);
                assert!(nodes.is_none());
            }
            other => panic!("expected rich raw html at limit, got {other:?}"),
        }

        let raw_html_over_limit = format!("{raw_html_at_limit}X");
        let mut over_limit = AuroraBuffer::new();
        over_limit.append_chunk(&raw_html_over_limit);
        assert_eq!(over_limit.process_queue(), (false, true));
        assert_eq!(
            over_limit.tail_projection.as_ref().map(|state| state.mode),
            Some(TailRenderMode::Plain)
        );
        match over_limit
            .current_tail_wire_block()
            .expect("raw tail over limit")
        {
            StreamBlock::Markdown { content, nodes, .. } => {
                assert_eq!(content, raw_html_over_limit);
                assert!(nodes.is_none());
            }
            other => panic!("expected plaintext raw html over limit, got {other:?}"),
        }
        assert!(over_limit.prev_tail_ast.is_empty());
        assert!(over_limit.take_tail_frame().is_none());
        let (initial_plain, initial_commit) = over_limit
            .prepare_delta_update()
            .expect("initial plaintext replace");
        assert!(matches!(
            initial_plain.tail_op,
            Some(TailTextOp::Replace {
                mode: TailRenderMode::Plain,
                ..
            })
        ));
        over_limit.commit_delivery(initial_commit);

        over_limit.append_chunk("Y");
        assert_eq!(over_limit.process_queue(), (false, true));
        assert!(over_limit.take_tail_frame().is_none());
        let (plain_append, _) = over_limit
            .prepare_delta_update()
            .expect("plaintext append delta");
        assert!(plain_append.tail_frame.is_none());
        assert!(matches!(
            plain_append.tail_op,
            Some(TailTextOp::Append {
                ref content,
                mode: TailRenderMode::Plain,
                ..
            }) if content == "Y"
        ));
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
}

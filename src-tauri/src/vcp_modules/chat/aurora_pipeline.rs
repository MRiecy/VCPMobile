use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::vcp_modules::chat::ast_diff::{diff_ast, AstMutation};
use crate::vcp_modules::pre_renderer::markdown_ast::MarkdownNode;
use crate::vcp_modules::stream_block_parser::{StreamBlock, StreamBlockParser};

/// 推测渲染的 tail 字节上限：超过此阈值跳过 AST 解析，降级为纯文本尾部。
///
/// 取值依据（perf profile 基准，见 ast_bench.rs，约等于发布版热路径速度）：
/// - 解析本身极廉价：40KB tail 的 parse+hash+diff+serialize 仅约 0.55ms，远非瓶颈。
/// - 真正的成本是 IPC 载荷：CodeBlock/RawHtml 走整节点 Replace，每帧重发整块，
///   40KB 块在一次流式中累计推送可达 ~18.5MB。
///   因此上限从 8192 提升到 65536（覆盖绝大多数真实 HTML/代码产物），
///   并配合 vcp_client 的自适应降帧（30→10→5Hz）把每秒 IPC 载荷压到可接受范围。
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

/// Aurora 语义沉淀更新，由 Rust 流式管道推送到前端
/// 采用稀疏序列化：只在字段有变化时才包含在 JSON 中，减少 IPC payload
#[derive(Debug, Serialize, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuroraUpdate {
    /// 整条 Aurora 更新所属的流身份；非流式单次响应没有该字段。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_id: Option<u64>,
    /// 流式增量块：已确认闭合的语义块（仅 stable_changed 时发送）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stable_blocks: Option<Vec<StreamBlock>>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub stable_changed: bool,
    /// 推测块：当前正在增长的尾部，按 Markdown 预渲染（仅 tail_changed 时发送）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tail_block: Option<StreamBlock>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tail: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub tail_changed: bool,
    /// 流式 AST 单帧补丁。每个 frame 是独立发送批次，前端不得累计全历史 mutations。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tail_frame: Option<TailFrame>,
    /// reset/recovery 使用的完整 tail AST 快照，保留为非 frame 恢复兜底字段
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tail_snapshot: Option<Vec<MarkdownNode>>,
    /// 全量内容（仅终结事件时发送，正常流式中省略）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// 🆕 推送周期中新增的、尚未推送给前端的纯文本片段
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chunk: Option<String>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// Aurora 语义沉淀缓冲区
/// 职责：用轻量块解析器识别已闭合/未闭合块，前端增量接收
pub struct AuroraBuffer {
    pub stream_id: u64,
    pub full_text: String,
    pub stable_blocks: Vec<StreamBlock>,
    pub tail_content: String,
    pub tail_block: Option<StreamBlock>,
    /// 🆕 上一帧的 tail AST 缓存，用于做增量 Diff 对比
    pub prev_tail_ast: Vec<MarkdownNode>,
    /// 🆕 待发送的增量 AST 突变指令暂存池，防抖丢帧时防止中间差异丢失
    pub pending_mutations: Vec<AstMutation>,
    pub tail_epoch: u64,
    pub tail_revision: u64,
    pub tail_reset_pending: bool,
    pub tail_snapshot_pending: Option<Vec<MarkdownNode>>,
    pub tail_frame_seq: u64,
    /// 🆕 记录已被消费并发送的 full_text 长度，用于计算增量 chunk
    pub pushed_len: usize,
    parser: StreamBlockParser,
    is_finishing: bool,
}

impl AuroraBuffer {
    pub fn new() -> Self {
        Self {
            stream_id: NEXT_AURORA_STREAM_ID.fetch_add(1, Ordering::Relaxed),
            full_text: String::new(),
            stable_blocks: Vec::new(),
            tail_content: String::new(),
            tail_block: None,
            prev_tail_ast: Vec::new(),
            pending_mutations: Vec::new(),
            tail_epoch: 0,
            tail_revision: 0,
            tail_reset_pending: false,
            tail_snapshot_pending: None,
            tail_frame_seq: 0,
            pushed_len: 0,
            parser: StreamBlockParser::new(),
            is_finishing: false,
        }
    }

    /// 将新的文本块追加到全文
    pub fn append_chunk(&mut self, chunk: &str) {
        self.full_text.push_str(chunk);
    }

    /// 🆕 提取自上次推送以来累积消费的新增字符
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

    pub fn take_tail_frame(&mut self) -> Option<TailFrame> {
        let reset = self.tail_reset_pending;
        self.tail_reset_pending = false;
        let snapshot = self.tail_snapshot_pending.take();
        let mutations = std::mem::take(&mut self.pending_mutations);

        if !reset && snapshot.is_none() && mutations.is_empty() {
            return None;
        }

        self.tail_frame_seq = self.tail_frame_seq.saturating_add(1);
        Some(TailFrame {
            stream_id: self.stream_id,
            epoch: self.tail_epoch,
            revision: self.tail_revision,
            frame_seq: self.tail_frame_seq,
            reset,
            snapshot,
            mutations: if reset { Vec::new() } else { mutations },
        })
    }

    /// 暖接续的权威数据基线：完整覆盖 content/stable/tail，但不携带旧 buffer 的 AST frame。
    /// 后续首个真实 frame 继续使用本 buffer 的 streamId，前端据此接管新的序列域。
    pub fn take_recovery_baseline(&mut self) -> AuroraUpdate {
        self.pushed_len = self.full_text.len();
        let _ = self.take_tail_frame();
        AuroraUpdate {
            stream_id: Some(self.stream_id),
            stable_blocks: Some(self.stable_blocks.clone()),
            stable_changed: true,
            tail_block: self.tail_block.clone(),
            tail: Some(self.tail_content.clone()),
            tail_changed: true,
            tail_frame: None,
            tail_snapshot: None,
            content: Some(self.full_text.clone()),
            chunk: None,
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
        let tail_changed = self.tail_content != new_tail;

        if !new_blocks.is_empty() {
            self.stable_blocks.extend(new_blocks);
            self.tail_epoch = self.tail_epoch.saturating_add(1);
            self.tail_revision = 0;
            self.tail_reset_pending = true;
            self.pending_mutations.clear();
            self.prev_tail_ast.clear();
            self.tail_snapshot_pending = None;
        }

        self.tail_content = new_tail;

        // 2. 推测渲染 (Speculative Rendering)：将 tail 视为一个临时 Markdown 块
        //    当 tail 超过 MAX_SPECULATIVE_TAIL_AST_BYTES 时跳过 AST 解析，
        //    避免在流式热路径上产生性能悬崖
        if !self.tail_content.is_empty() {
            let (nodes, nodes_need_hashing) =
                if crate::vcp_modules::content_parser::is_html_tag_block(&self.tail_content) {
                    // 如果是以 HTML 容器/样式标签开头，直接将其作为 RawHtml 块，防止 pulldown_cmark 将内部 CSS 规则或内联样式解析为缩进代码块
                    (
                        Some(vec![
                            crate::vcp_modules::pre_renderer::MarkdownNode::raw_html(
                                self.tail_content.clone(),
                            ),
                        ]),
                        true,
                    )
                } else if self.tail_content.len() <= MAX_SPECULATIVE_TAIL_AST_BYTES {
                    (
                        Some(
                            crate::vcp_modules::pre_renderer::parse_markdown_to_ast_streaming(
                                &self.tail_content,
                            ),
                        ),
                        false,
                    )
                } else {
                    (None, false)
                };
            let hash = crate::vcp_modules::sync_hash::HashAggregator::compute_content_hash(
                &self.tail_content,
            );

            // 🆕 如果解析出了 AST，对其计算 Diff，生成增量渲染指令集
            if let Some(mut new_nodes) = nodes.clone() {
                // parser 返回的树已经递归 hash；只有绕过 parser 手工构造的 RawHtml clone
                // 需要补一次。只处理 diff 基线副本，保持 tailBlock 的既有 wire 形态不变。
                if nodes_need_hashing {
                    for node in &mut new_nodes {
                        node.compute_hashes_recursively();
                    }
                }
                // reset 帧会被 take_tail_frame 强制清空 mutations 并改发 snapshot，
                // 故此时跳过 diff_ast（其结果必被丢弃），直接记录 snapshot，省去一次全量 diff。
                if self.tail_reset_pending {
                    self.tail_snapshot_pending = Some(new_nodes.clone());
                } else {
                    let mutations = diff_ast(&self.prev_tail_ast, &new_nodes, "t");
                    if !mutations.is_empty() {
                        self.pending_mutations.extend(mutations);
                    }
                }
                self.tail_revision = self.tail_revision.saturating_add(1);
                self.prev_tail_ast = new_nodes;
            } else {
                // 超长 tail（> MAX_SPECULATIVE_TAIL_AST_BYTES 且非 HTML 容器）：降级为纯文本尾部。
                // 不再逐帧产出 AST 帧，改由 tail_block.content 走前端纯文本路径渲染（绝不留白）。
                // 仅在「首次从 AST 模式跨入纯文本模式」时触发一次 epoch reset 清空旧 AST 沙箱，
                // 之后保持安静，避免逐帧 epoch 自增与空转 reset 帧。
                let was_ast_mode = !self.prev_tail_ast.is_empty();
                self.prev_tail_ast.clear();
                self.pending_mutations.clear();
                if was_ast_mode && !self.tail_reset_pending {
                    self.tail_epoch = self.tail_epoch.saturating_add(1);
                    self.tail_revision = 0;
                    self.tail_reset_pending = true;
                    self.tail_snapshot_pending = Some(Vec::new());
                }
            }

            self.tail_block = Some(StreamBlock::markdown(
                self.tail_content.clone(),
                nodes,
                hash,
            ));
        } else {
            self.tail_block = None;
            if !self.prev_tail_ast.is_empty() || !self.tail_content.is_empty() {
                self.tail_epoch = self.tail_epoch.saturating_add(1);
                self.tail_revision = 0;
                self.tail_reset_pending = true;
                self.pending_mutations.clear();
                self.tail_snapshot_pending = Some(Vec::new());
            }
            self.prev_tail_ast.clear();
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
        self.tail_block = None;
        self.prev_tail_ast.clear();
        self.pending_mutations.clear();
        self.tail_epoch = self.tail_epoch.saturating_add(1);
        self.tail_revision = 0;
        self.tail_reset_pending = true;
        self.tail_snapshot_pending = Some(Vec::new());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一个超过 MAX_SPECULATIVE_TAIL_AST_BYTES 的、非 HTML 起始的纯文本代码块 tail，
    /// 验证 #1c 降级行为：tail_block 仍带纯文本 content（绝不留白），且不再逐帧自增 epoch。
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

        // 关键：tail_block 必须存在且携带纯文本 content，nodes 为 None（前端据此走纯文本路径）
        let tb = buffer
            .tail_block
            .as_ref()
            .expect("tail_block 不应为空（绝不留白）");
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

    /// 小于上限的普通代码块仍走 AST 路径：tail_block.nodes 应为 Some。
    #[test]
    fn test_normal_tail_uses_ast() {
        let mut buffer = AuroraBuffer::new();
        buffer.append_chunk("正常一段流式文本，尚未闭合");
        let (stable_changed, tail_changed) = buffer.process_queue();
        assert!(!stable_changed);
        assert!(tail_changed);
        {
            let tb = buffer.tail_block.as_ref().expect("tail_block 应存在");
            if let StreamBlock::Markdown { nodes, .. } = tb {
                let nodes = nodes.as_deref().expect("小体量 tail 应解析出 AST 节点");
                assert!(nodes[0].get_hash().is_some());
                assert_eq!(nodes, buffer.prev_tail_ast.as_slice());
            } else {
                panic!("expected markdown tail block");
            }
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
    fn raw_html_keeps_wire_shape_but_hashes_the_diff_baseline_once() {
        let mut buffer = AuroraBuffer::new();
        buffer.append_chunk("<div>one");
        assert_eq!(buffer.process_queue(), (false, true));

        let tail_nodes = match buffer.tail_block.as_ref().expect("raw tail block") {
            StreamBlock::Markdown {
                nodes: Some(nodes), ..
            } => nodes,
            other => panic!("expected raw markdown tail block, got {other:?}"),
        };
        assert!(tail_nodes[0].get_hash().is_none());
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
        assert_eq!(baseline.stream_id, Some(buffer.stream_id));
        assert_eq!(baseline.content.as_deref(), Some("authoritative"));
        assert_eq!(baseline.tail.as_deref(), Some("authoritative"));
        assert!(baseline.stable_changed);
        assert!(baseline.tail_changed);
        assert!(baseline.tail_block.is_some());
        assert!(baseline.tail_frame.is_none());
        assert!(baseline.chunk.is_none());
        assert!(buffer.take_chunk().is_none());

        buffer.append_chunk("!");
        buffer.process_queue();
        let next = buffer.take_tail_frame().expect("post-baseline tail frame");
        assert_eq!(next.stream_id, buffer.stream_id);
        assert_eq!(next.frame_seq, 2);
        assert_eq!(buffer.take_chunk().as_deref(), Some("!"));
    }
}

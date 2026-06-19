use serde::{Deserialize, Serialize};

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

#[derive(Debug, Serialize, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TailFrame {
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
            epoch: self.tail_epoch,
            revision: self.tail_revision,
            frame_seq: self.tail_frame_seq,
            reset,
            snapshot,
            mutations: if reset { Vec::new() } else { mutations },
        })
    }

    /// 运行块解析器，识别已闭合块和未闭合尾部
    /// 返回 (stable_changed, tail_changed)
    pub fn process_queue(&mut self) -> (bool, bool) {
        if self.is_finishing {
            return (false, false);
        }

        let prev_stable_count = self.stable_blocks.len();
        let prev_tail = self.tail_content.clone();

        // 1. 增量解析全文，产出本次新增的已闭合块 + 尾部纯文本
        let (new_blocks, new_tail) = self.parser.process(&self.full_text);

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
            let nodes = if crate::vcp_modules::content_parser::is_html_tag_block(&self.tail_content)
            {
                // 如果是以 HTML 容器/样式标签开头，直接将其作为 RawHtml 块，防止 pulldown_cmark 将内部 CSS 规则或内联样式解析为缩进代码块
                Some(vec![
                    crate::vcp_modules::pre_renderer::MarkdownNode::raw_html(
                        self.tail_content.clone(),
                    ),
                ])
            } else if self.tail_content.len() <= MAX_SPECULATIVE_TAIL_AST_BYTES {
                Some(
                    crate::vcp_modules::pre_renderer::parse_markdown_to_ast_streaming(
                        &self.tail_content,
                    ),
                )
            } else {
                None
            };
            let hash = crate::vcp_modules::sync_hash::HashAggregator::compute_content_hash(
                &self.tail_content,
            );

            // 🆕 如果解析出了 AST，对其计算 Diff，生成增量渲染指令集
            if let Some(mut new_nodes) = nodes.clone() {
                for node in &mut new_nodes {
                    node.compute_hashes_recursively();
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
        let tail_changed = self.tail_content != prev_tail;

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
        buffer.process_queue();
        let tb = buffer.tail_block.as_ref().expect("tail_block 应存在");
        if let StreamBlock::Markdown { nodes, .. } = tb {
            assert!(nodes.is_some(), "小体量 tail 应解析出 AST 节点");
        } else {
            panic!("expected markdown tail block");
        }
    }
}

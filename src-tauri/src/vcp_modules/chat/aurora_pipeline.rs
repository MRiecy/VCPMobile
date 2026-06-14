use serde::{Deserialize, Serialize};

use crate::vcp_modules::chat::ast_diff::{diff_ast, AstMutation};
use crate::vcp_modules::pre_renderer::markdown_ast::MarkdownNode;
use crate::vcp_modules::stream_block_parser::{StreamBlock, StreamBlockParser};

/// 推测渲染的 tail 字节上限：超过此阈值跳过 AST 解析，防止流式热路径性能悬崖
const MAX_SPECULATIVE_TAIL_AST_BYTES: usize = 8192;

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
            parser: StreamBlockParser::new(),
            is_finishing: false,
        }
    }

    /// 将新的文本块追加到全文
    pub fn append_chunk(&mut self, chunk: &str) {
        self.full_text.push_str(chunk);
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
                let mutations = diff_ast(&self.prev_tail_ast, &new_nodes, "t");
                if !mutations.is_empty() {
                    self.pending_mutations.extend(mutations);
                }
                self.tail_revision = self.tail_revision.saturating_add(1);
                if self.tail_reset_pending {
                    self.tail_snapshot_pending = Some(new_nodes.clone());
                }
                self.prev_tail_ast = new_nodes;
            } else {
                self.prev_tail_ast.clear();
                if !self.tail_reset_pending {
                    self.tail_epoch = self.tail_epoch.saturating_add(1);
                    self.tail_revision = 0;
                    self.tail_reset_pending = true;
                }
                self.pending_mutations.clear();
                self.tail_snapshot_pending = None;
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

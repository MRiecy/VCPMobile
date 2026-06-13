use serde::{Deserialize, Serialize};

use crate::vcp_modules::stream_block_parser::{StreamBlock, StreamBlockParser};
use crate::vcp_modules::chat::ast_diff::{diff_ast, AstMutation};
use crate::vcp_modules::pre_renderer::markdown_ast::MarkdownNode;

/// 推测渲染的 tail 字节上限：超过此阈值跳过 AST 解析，防止流式热路径性能悬崖
const MAX_SPECULATIVE_TAIL_AST_BYTES: usize = 8192;

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
    /// 🆕 流式 AST 突变指令集（仅在启用 AST Diff 模式且有变动时包含）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tail_mutations: Option<Vec<AstMutation>>,
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
            parser: StreamBlockParser::new(),
            is_finishing: false,
        }
    }

    /// 将新的文本块追加到全文
    pub fn append_chunk(&mut self, chunk: &str) {
        self.full_text.push_str(chunk);
    }

    /// 运行块解析器，识别已闭合块和未闭合尾部
    /// 返回 (stable_changed, tail_changed, tail_mutations)
    pub fn process_queue(&mut self) -> (bool, bool, Option<Vec<AstMutation>>) {
        if self.is_finishing {
            return (false, false, None);
        }

        let prev_stable_count = self.stable_blocks.len();
        let prev_tail = self.tail_content.clone();

        // 1. 增量解析全文，产出本次新增的已闭合块 + 尾部纯文本
        let (new_blocks, new_tail) = self.parser.process(&self.full_text);

        if !new_blocks.is_empty() {
            self.stable_blocks.extend(new_blocks);
        }

        self.tail_content = new_tail;

        let mut tail_mutations = None;

        // 2. 推测渲染 (Speculative Rendering)：将 tail 视为一个临时 Markdown 块
        //    当 tail 超过 MAX_SPECULATIVE_TAIL_AST_BYTES 时跳过 AST 解析，
        //    避免在流式热路径上产生性能悬崖
        if !self.tail_content.is_empty() {
            let nodes = if crate::vcp_modules::content_parser::is_html_tag_block(&self.tail_content) {
                // 如果是以 HTML 容器/样式标签开头，直接将其作为 RawHtml 块，防止 pulldown_cmark 将内部 CSS 规则或内联样式解析为缩进代码块
                Some(vec![crate::vcp_modules::pre_renderer::MarkdownNode::raw_html(self.tail_content.clone())])
            } else if self.tail_content.len() <= MAX_SPECULATIVE_TAIL_AST_BYTES {
                Some(crate::vcp_modules::pre_renderer::parse_markdown_to_ast(
                    &self.tail_content,
                ))
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
                    tail_mutations = Some(mutations);
                }
                self.prev_tail_ast = new_nodes;
            } else {
                self.prev_tail_ast.clear();
            }

            self.tail_block = Some(StreamBlock::markdown(
                self.tail_content.clone(),
                nodes,
                hash,
            ));
        } else {
            self.tail_block = None;
            self.prev_tail_ast.clear();
        }

        let stable_changed = self.stable_blocks.len() != prev_stable_count;
        let tail_changed = self.tail_content != prev_tail;

        (stable_changed, tail_changed, tail_mutations)
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
    }
}

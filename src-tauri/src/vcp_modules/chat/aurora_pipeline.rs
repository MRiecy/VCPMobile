use serde::{Deserialize, Serialize};

use crate::vcp_modules::stream_block_parser::{StreamBlock, StreamBlockParser};

/// Aurora 语义沉淀更新，由 Rust 流式管道推送到前端
#[derive(Debug, Serialize, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuroraUpdate {
    /// 流式增量块：已确认闭合的语义块
    pub stable_blocks: Vec<StreamBlock>,
    /// 推测块：当前正在增长的尾部，按 Markdown 预渲染
    pub tail_block: Option<StreamBlock>,
    pub tail: String,
    pub content: String,
}

/// Aurora 语义沉淀缓冲区
/// 职责：用轻量块解析器识别已闭合/未闭合块，前端增量接收
pub struct AuroraBuffer {
    pub full_text: String,
    pub stable_blocks: Vec<StreamBlock>,
    pub tail_content: String,
    pub tail_block: Option<StreamBlock>,
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
            parser: StreamBlockParser::new(),
            is_finishing: false,
        }
    }

    /// 将新的文本块追加到全文
    pub fn append_chunk(&mut self, chunk: &str) {
        self.full_text.push_str(chunk);
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
        }

        self.tail_content = new_tail;

        // 2. 推测渲染 (Speculative Rendering)：将 tail 视为一个临时 Markdown 块
        if !self.tail_content.is_empty() {
            let nodes = crate::vcp_modules::pre_renderer::parse_markdown_to_ast(&self.tail_content);
            let hash = crate::vcp_modules::sync_hash::HashAggregator::compute_content_hash(
                &self.tail_content,
            );
            self.tail_block = Some(StreamBlock::markdown(
                self.tail_content.clone(),
                Some(nodes),
                hash,
            ));
        } else {
            self.tail_block = None;
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
    }

    /// 简单的 HTML 标签补全，防止流式输出截断导致 DOM 渲染异常
    pub fn balance_html_tags(html: &str) -> String {
        let tags = ["div", "pre", "code", "p", "span", "blockquote"];
        let mut balanced = html.to_string();
        for tag in tags {
            let open_count = html.matches(&format!("<{tag}>")).count()
                + html.matches(&format!("<{tag} ")).count();
            let close_count = html.matches(&format!("</{tag}>")).count();
            if open_count > close_count {
                balanced.push_str(&format!("</{tag}>").repeat(open_count - close_count));
            }
        }
        balanced
    }
}

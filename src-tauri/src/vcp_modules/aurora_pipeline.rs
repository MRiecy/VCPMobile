use serde::{Deserialize, Serialize};

use crate::vcp_modules::stream_block_parser::{StreamBlock, StreamBlockParser};

/// Aurora 语义沉淀更新，由 Rust 流式管道推送到前端
#[derive(Debug, Serialize, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuroraUpdate {
    pub stable: String,
    /// 流式增量块：已确认闭合的语义块，前端增量渲染
    pub stable_blocks: Vec<StreamBlock>,
    pub tail: String,
    pub content: String,
}

/// Aurora 语义沉淀缓冲区
/// 职责：用轻量块解析器识别已闭合/未闭合块，前端增量接收
pub struct AuroraBuffer {
    pub full_text: String,
    pub stable_content: String,
    pub stable_blocks: Vec<StreamBlock>,
    pub tail_content: String,
    parser: StreamBlockParser,
    is_finishing: bool,
}

impl AuroraBuffer {
    pub fn new() -> Self {
        Self {
            full_text: String::new(),
            stable_content: String::new(),
            stable_blocks: Vec::new(),
            tail_content: String::new(),
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
        let prev_stable_len = self.stable_content.len();
        let prev_tail = self.tail_content.clone();

        // 增量解析全文，产出已闭合块 + 尾部纯文本
        let (new_blocks, new_tail) = self.parser.process(&self.full_text);

        self.stable_blocks = new_blocks;
        self.tail_content = new_tail;

        // 从块数组重建 stable_content HTML（用于 displayedContent 后备）
        let mut html = String::new();
        for block in &self.stable_blocks {
            html.push_str(&stream_block_to_html(block));
        }
        self.stable_content = html;

        let stable_changed = self.stable_blocks.len() != prev_stable_count
            || self.stable_content.len() != prev_stable_len;
        let tail_changed = self.tail_content != prev_tail;

        (stable_changed, tail_changed)
    }

    /// 结束流：强制完成剩余内容
    pub fn finalize(&mut self) {
        self.is_finishing = true;
        let final_blocks = self.parser.finalize(&self.full_text);
        self.stable_blocks = final_blocks;

        // 重建 stable_content
        let mut html = String::new();
        for block in &self.stable_blocks {
            html.push_str(&stream_block_to_html(block));
        }
        self.stable_content = html;
        self.tail_content.clear();
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

/// 将 StreamBlock 转为纯文本（用于 stable_content 字符串）
/// 注意：这里的 stable_content 仅作后备兼容，前端实际使用 stable_blocks 渲染
fn stream_block_to_html(block: &StreamBlock) -> String {
    match block {
        StreamBlock::Markdown { content, .. } => {
            format!("{}\n\n", content)
        }
        StreamBlock::Thought { content, .. } => {
            format!("[思考]\n{}\n\n", content)
        }
        StreamBlock::Tool { tool_name, content } => {
            format!("[工具: {}]\n{}\n\n", tool_name, content)
        }
        StreamBlock::ToolResult {
            tool_name,
            status,
            details,
            footer,
        } => {
            let mut s = format!("[工具结果: {} ({})]\n", tool_name, status);
            for d in details {
                s.push_str(&format!("  {}: {}\n", d.key, d.value));
            }
            if !footer.is_empty() {
                s.push_str(&format!("{}\n", footer));
            }
            s.push('\n');
            s
        }
        StreamBlock::Diary { maid, date, content, .. } => {
            format!("[日记: {} @ {}]\n{}\n\n", maid, date, content)
        }
        StreamBlock::HtmlPreview { content } => {
            format!("{}\n\n", content)
        }
        StreamBlock::RoleDivider { role, is_end } => {
            let action = if *is_end { "结束" } else { "起始" };
            format!("[角色分界: {} {}]\n\n", role, action)
        }
        StreamBlock::Style { content } => {
            format!("<style>{}</style>\n\n", content)
        }
        StreamBlock::ButtonClick { content } => {
            format!("[按钮: {}]\n\n", content)
        }
    }
}

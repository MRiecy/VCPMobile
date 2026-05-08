use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// Aurora 语义沉淀更新，由 Rust 流式管道推送到前端
#[derive(Debug, Serialize, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuroraUpdate {
    pub stable: String,
    pub tail: String,
    pub content: String,
}

/// Aurora 语义沉淀缓冲区
/// 职责：在 Rust 端将流式文本分为稳定区（stable）和尾部（tail），
/// 前端只需直接更新 Vue 状态，不再做 Markdown 语义分析。
pub struct AuroraBuffer {
    pub full_text: String,
    pub stable_content: String,
    pub tail_content: String,
    semantic_queue: VecDeque<char>,
    is_finishing: bool,
}

impl AuroraBuffer {
    pub fn new() -> Self {
        Self {
            full_text: String::new(),
            stable_content: String::new(),
            tail_content: String::new(),
            semantic_queue: VecDeque::new(),
            is_finishing: false,
        }
    }

    /// 将新的文本块推入语义队列
    pub fn append_chunk(&mut self, chunk: &str) {
        self.full_text.push_str(chunk);
        for ch in chunk.chars() {
            self.semantic_queue.push_back(ch);
        }
    }

    /// 消费语义队列，执行沉淀逻辑
    /// 返回 (stable_changed, tail_changed)
    pub fn process_queue(&mut self) -> (bool, bool) {
        let backlog = self.semantic_queue.len();
        if backlog == 0 {
            return (false, false);
        }

        let step = backlog.div_ceil(8);
        let mut tail_changed = false;

        for _ in 0..step {
            if let Some(ch) = self.semantic_queue.pop_front() {
                self.tail_content.push(ch);
                tail_changed = true;
            }
        }

        // 查找沉淀锚点（双换行）
        let mut stable_changed = false;
        if let Some(last_break) = self.tail_content.rfind("\n\n") {
            if !self.is_finishing {
                let potential_stable = &self.tail_content[..last_break + 2];

                // 严谨检测是否处于非稳定块内部
                let is_in_code = !potential_stable.matches("```").count().is_multiple_of(2);
                let is_in_think = potential_stable.matches("<think").count()
                    > potential_stable.matches("</think").count();
                let is_in_vcp_think = potential_stable.matches("[--- VCP元思考链").count()
                    > potential_stable.matches("[--- 元思考链结束 ---]").count();
                let is_in_tool = potential_stable.matches("<<<[TOOL_REQUEST]>>>").count()
                    > potential_stable.matches("<<<[END_TOOL_REQUEST]>>>").count();

                if !is_in_code && !is_in_think && !is_in_vcp_think && !is_in_tool {
                    self.stable_content.push_str(potential_stable);
                    self.tail_content = self.tail_content[last_break + 2..].to_string();
                    stable_changed = true;
                }
            }
        }

        (stable_changed, tail_changed)
    }

    /// 结束流：将剩余 tail 彻底沉淀到 stable
    pub fn finalize(&mut self) {
        self.is_finishing = true;
        if !self.tail_content.is_empty() {
            self.stable_content.push_str(&self.tail_content);
            self.tail_content.clear();
        }
        self.semantic_queue.clear();
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

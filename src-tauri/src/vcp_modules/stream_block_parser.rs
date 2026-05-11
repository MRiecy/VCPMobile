use serde::{Deserialize, Serialize};

use crate::vcp_modules::content_parser::{
    BlockType, ToolResultDetail, BUTTON_CLICK, DIARY_END, DIARY_START, GENERIC_CODE_FENCE_END,
    GENERIC_CODE_FENCE_START, HTML_DOC_END, HTML_DOC_START, HTML_FENCE_START,
    ROLE_DIVIDER, STYLE_TAG_END, STYLE_TAG_START, THINK_END, THINK_START, THOUGHT_END,
    THOUGHT_START, TOOL_END, TOOL_RESULT_END, TOOL_RESULT_START, TOOL_START,
    /* extraction helpers */
    CONTENT_REGEX, DATE_REGEX, KV_REGEX, MAID_REGEX, TOOL_NAME,
};
use crate::vcp_modules::pre_renderer::MarkdownNode;

/// 流式模式下轻量解析的块类型，前端增量渲染
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum StreamBlock {
    #[serde(rename = "markdown")]
    Markdown {
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        nodes: Option<Vec<MarkdownNode>>,
    },
    #[serde(rename = "thought")]
    Thought {
        theme: String,
        content: String,
        is_complete: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        nodes: Option<Vec<MarkdownNode>>,
    },
    #[serde(rename = "tool-use")]
    Tool {
        tool_name: String,
        content: String,
    },
    #[serde(rename = "tool-result")]
    ToolResult {
        tool_name: String,
        status: String,
        details: Vec<ToolResultDetail>,
        footer: String,
    },
    #[serde(rename = "diary")]
    Diary {
        maid: String,
        date: String,
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        nodes: Option<Vec<MarkdownNode>>,
    },
    #[serde(rename = "html-preview")]
    HtmlPreview {
        content: String,
    },
    #[serde(rename = "role-divider")]
    RoleDivider {
        role: String,
        is_end: bool,
    },
    #[serde(rename = "style")]
    Style {
        content: String,
    },
    #[serde(rename = "button-click")]
    ButtonClick {
        content: String,
    },
}

/// 流式块解析器
/// 增量扫描 full_text，识别已闭合的语义块和未闭合的尾部
pub struct StreamBlockParser {
    processed_len: usize,
}

impl StreamBlockParser {
    pub fn new() -> Self {
        Self { processed_len: 0 }
    }

    /// 处理累积的全文，返回 (已完成的块列表, 尾部纯文本)
    /// 已闭合的块从 tail 中移除加入 stable blocks，未闭合部分保留为 tail
    pub fn process(&mut self, full_text: &str) -> (Vec<StreamBlock>, String) {
        let mut blocks = Vec::new();
        let mut pos = self.processed_len.min(full_text.len());

        while pos < full_text.len() {
            let remaining = &full_text[pos..];

            // 1. 寻找最早出现的特种块起始标记
            if let Some((start, end, block_type)) = find_earliest_start_marker(remaining) {
                // 2. 标记之前的文本 → Markdown 段落
                if start > 0 {
                    let before = &remaining[..start];
                    let (md_blocks, md_tail) = split_markdown_paragraphs(before);
                    blocks.extend(md_blocks);
                    if !md_tail.is_empty() {
                        // 不完整的段落 + 后面的特种块 → 整体作为 tail
                        self.processed_len = pos;
                        let tail =
                            format!("{}{}", md_tail, &remaining[start..]);
                        return (blocks, tail);
                    }
                }

                // 3. 寻找对应结束标记
                let content_start = end;
                let search_area = &remaining[content_start..];

                if let Some((end_start, end_end)) =
                    find_end_marker(search_area, &block_type)
                {
                    let inner_content = &search_area[..end_start];
                    let block = build_stream_block(
                        &block_type,
                        inner_content,
                        remaining,
                        start,
                        end,
                    );
                    blocks.push(block);
                    pos += content_start + end_end;
                } else {
                    // 找不到结束标记 → 从该块起始开始全部作为 tail
                    self.processed_len = pos;
                    return (blocks, remaining.to_string());
                }
            } else {
                // 4. 无任何特种块标记 → 全部按段落分割
                let (md_blocks, md_tail) = split_markdown_paragraphs(remaining);
                blocks.extend(md_blocks);
                if md_tail.is_empty() {
                    self.processed_len = full_text.len();
                    return (blocks, String::new());
                } else {
                    self.processed_len = pos + remaining.len() - md_tail.len();
                    return (blocks, md_tail.to_string());
                }
            }
        }

        self.processed_len = full_text.len();
        (blocks, String::new())
    }

    /// 流结束：强制处理剩余 tail 为最后一个 Markdown 块
    pub fn finalize(&mut self, full_text: &str) -> Vec<StreamBlock> {
        let (mut blocks, tail) = self.process(full_text);
        let trimmed = tail.trim();
        if !trimmed.is_empty() {
            let nodes =
                crate::vcp_modules::pre_renderer::parse_markdown_to_ast(trimmed);
            blocks.push(StreamBlock::Markdown {
                content: trimmed.to_string(),
                nodes: Some(nodes),
            });
        }
        blocks
    }

    /// 重置解析器状态（用于新消息）
    #[allow(dead_code)]
    pub fn reset(&mut self) {
        self.processed_len = 0;
    }
}

// ── 内部辅助函数 ──────────────────────────────────────────────────────

/// 在文本中寻找最早出现的特种块起始标记
/// 返回 (start_offset, end_offset, BlockType)
fn find_earliest_start_marker(text: &str) -> Option<(usize, usize, BlockType)> {
    let checks: [(&regex::Regex, BlockType); 10] = [
        (&TOOL_START, BlockType::Tool),
        (&THOUGHT_START, BlockType::Thought),
        (&THINK_START, BlockType::Think),
        (&TOOL_RESULT_START, BlockType::ToolResult),
        (&DIARY_START, BlockType::Diary),
        (&HTML_FENCE_START, BlockType::HtmlFence),
        (&HTML_DOC_START, BlockType::HtmlDoc),
        (&ROLE_DIVIDER, BlockType::RoleDivider),
        (&STYLE_TAG_START, BlockType::Style),
        (&GENERIC_CODE_FENCE_START, BlockType::CodeFence),
    ];

    let mut earliest: Option<(usize, usize, BlockType)> = None;
    for (re, bt) in checks {
        if let Some(m) = re.find(text) {
            if earliest
                .as_ref()
                .is_none_or(|(s, _, _)| m.start() < *s)
            {
                earliest = Some((m.start(), m.end(), bt));
            }
        }
    }
    earliest
}

/// 寻找对应块的结束标记
/// 返回 (end_start_offset, end_end_offset) 在 search_area 内
fn find_end_marker(
    search_area: &str,
    block_type: &BlockType,
) -> Option<(usize, usize)> {
    let m = match block_type {
        BlockType::Tool => TOOL_END.find(search_area),
        BlockType::Thought => THOUGHT_END.find(search_area),
        BlockType::Think => THINK_END.find(search_area),
        BlockType::ToolResult => TOOL_RESULT_END.find(search_area),
        BlockType::Diary => DIARY_END.find(search_area),
        BlockType::HtmlFence | BlockType::CodeFence => {
            GENERIC_CODE_FENCE_END.find(search_area)
        }
        BlockType::HtmlDoc => HTML_DOC_END.find(search_area),
        BlockType::RoleDivider => {
            // RoleDivider 是单行标记，自闭合
            return Some((0, 0));
        }
        BlockType::Style => STYLE_TAG_END.find(search_area),
    };
    m.map(|m| (m.start(), m.end()))
}

/// 从匹配的标记构建 StreamBlock
fn build_stream_block(
    block_type: &BlockType,
    inner_content: &str,
    remaining: &str,
    start_idx: usize,
    end_idx: usize,
) -> StreamBlock {
    match block_type {
        BlockType::Tool => {
            let tool_name = extract_tool_name(inner_content);
            if is_daily_note_create(inner_content) {
                let (maid, date, content) = extract_diary_details(inner_content);
                let nodes =
                    crate::vcp_modules::pre_renderer::parse_markdown_to_ast(&content);
                StreamBlock::Diary {
                    maid,
                    date,
                    content,
                    nodes: Some(nodes),
                }
            } else {
                StreamBlock::Tool {
                    tool_name,
                    content: inner_content.to_string(),
                }
            }
        }
        BlockType::Thought => {
            let start_marker_text = &remaining[start_idx..end_idx];
            let theme = THOUGHT_START
                .captures(start_marker_text)
                .and_then(|c| c.get(1))
                .map(|m| m.as_str().trim().replace("\"", ""))
                .unwrap_or_else(|| "元思考链".to_string());
            let nodes =
                crate::vcp_modules::pre_renderer::parse_markdown_to_ast(inner_content);
            StreamBlock::Thought {
                theme,
                content: inner_content.to_string(),
                is_complete: true,
                nodes: Some(nodes),
            }
        }
        BlockType::Think => {
            let nodes =
                crate::vcp_modules::pre_renderer::parse_markdown_to_ast(inner_content);
            StreamBlock::Thought {
                theme: "思维链".to_string(),
                content: inner_content.to_string(),
                is_complete: true,
                nodes: Some(nodes),
            }
        }
        BlockType::ToolResult => {
            let (tool_name, status, details, footer) = parse_tool_result(inner_content);
            StreamBlock::ToolResult {
                tool_name,
                status,
                details,
                footer,
            }
        }
        BlockType::Diary => {
            let (maid, date, content) = extract_diary_details(inner_content);
            let nodes =
                crate::vcp_modules::pre_renderer::parse_markdown_to_ast(&content);
            StreamBlock::Diary {
                maid,
                date,
                content,
                nodes: Some(nodes),
            }
        }
        BlockType::HtmlFence | BlockType::HtmlDoc => {
            // HTML 块：提取纯 HTML 内容
            StreamBlock::HtmlPreview {
                content: inner_content.to_string(),
            }
        }
        BlockType::CodeFence => {
            // 代码围栏：拼回完整 Markdown 走标准 AST 渲染，复用现有 CodeBlock 样式
            let fence = &remaining[start_idx..end_idx];
            let full_text = format!("{}\n{}\n```", fence, inner_content);
            let nodes =
                crate::vcp_modules::pre_renderer::parse_markdown_to_ast(&full_text);
            StreamBlock::Markdown {
                content: full_text,
                nodes: Some(nodes),
            }
        }
        BlockType::RoleDivider => {
            let marker_text = &remaining[start_idx..end_idx];
            if let Some(caps) = ROLE_DIVIDER.captures(marker_text) {
                let is_end = caps.get(1).is_some();
                let role = caps
                    .get(2)
                    .map(|m| m.as_str().to_lowercase())
                    .unwrap_or_default();
                StreamBlock::RoleDivider { role, is_end }
            } else {
                StreamBlock::RoleDivider {
                    role: "unknown".to_string(),
                    is_end: false,
                }
            }
        }
        BlockType::Style => StreamBlock::Style {
            content: inner_content.to_string(),
        },
    }
}

/// 将纯文本按 \n\n 分割为 Markdown 段落块
/// 返回 (completed_blocks, tail_text)
fn split_markdown_paragraphs(text: &str) -> (Vec<StreamBlock>, String) {
    if text.is_empty() {
        return (Vec::new(), String::new());
    }

    if let Some(last_break) = text.rfind("\n\n") {
        let stable = &text[..last_break + 2];
        let tail = &text[last_break + 2..];

        let mut blocks = Vec::new();
        for para in stable.split("\n\n") {
            let trimmed = para.trim();
            if trimmed.is_empty() {
                continue;
            }
            // 对已闭合的 Markdown 段落进行 AST 预渲染
            let nodes =
                crate::vcp_modules::pre_renderer::parse_markdown_to_ast(trimmed);
            blocks.push(StreamBlock::Markdown {
                content: trimmed.to_string(),
                nodes: Some(nodes),
            });
        }
        // 检查 inline button clicks
        let blocks = extract_inline_buttons(blocks);
        (blocks, tail.to_string())
    } else {
        // 无段落分割 → 全部为 tail
        (Vec::new(), text.to_string())
    }
}

/// 从 Markdown 块中提取内联按钮点击
fn extract_inline_buttons(mut blocks: Vec<StreamBlock>) -> Vec<StreamBlock> {
    let mut result = Vec::new();

    for block in blocks.drain(..) {
        match block {
            StreamBlock::Markdown { content, nodes } => {
                let mut last_end = 0;
                let mut has_button = false;

                for cap in BUTTON_CLICK.captures_iter(&content) {
                    has_button = true;
                    let Some(m) = cap.get(0) else { continue };
                    let Some(btn_content) = cap.get(1) else { continue };

                    // 按钮前的文本作为 Markdown 块
                    if m.start() > last_end {
                        let before = content[last_end..m.start()].trim().to_string();
                        if !before.is_empty() {
                            let before_nodes =
                                crate::vcp_modules::pre_renderer::parse_markdown_to_ast(
                                    &before,
                                );
                            result.push(StreamBlock::Markdown {
                                content: before,
                                nodes: Some(before_nodes),
                            });
                        }
                    }

                    result.push(StreamBlock::ButtonClick {
                        content: btn_content.as_str().trim().to_string(),
                    });
                    last_end = m.end();
                }

                if has_button {
                    // 最后一个按钮后的文本
                    if last_end < content.len() {
                        let after = content[last_end..].trim().to_string();
                        if !after.is_empty() {
                            let after_nodes =
                                crate::vcp_modules::pre_renderer::parse_markdown_to_ast(
                                    &after,
                                );
                            result.push(StreamBlock::Markdown {
                                content: after,
                                nodes: Some(after_nodes),
                            });
                        }
                    }
                } else {
                    result.push(StreamBlock::Markdown { content, nodes });
                }
            }
            other => result.push(other),
        }
    }

    result
}

// ── 内容提取辅助函数（与 content_parser.rs 保持一致的逻辑）──

fn extract_tool_name(content: &str) -> String {
    if let Some(caps) = TOOL_NAME.captures(content) {
        if let Some(m) = caps.get(1).or_else(|| caps.get(2)) {
            let mut name = m.as_str().trim().to_string();
            name = name
                .replace("「始」", "")
                .replace("「末」", "")
                .replace("「始exp」", "")
                .replace("「末exp」", "");
            if name.ends_with(',') {
                name.pop();
            }
            return name.trim().to_string();
        }
    }
    "Processing...".to_string()
}

fn is_daily_note_create(content: &str) -> bool {
    content.contains("DailyNote") && content.contains("create")
}

fn extract_diary_details(content: &str) -> (String, String, String) {
    let maid = MAID_REGEX
        .captures(content)
        .and_then(|c| c.get(1).or_else(|| c.get(2)))
        .map(|m| m.as_str().trim().to_string())
        .unwrap_or_default();

    let date = DATE_REGEX
        .captures(content)
        .and_then(|c| c.get(1).or_else(|| c.get(2)))
        .map(|m| m.as_str().trim().to_string())
        .unwrap_or_default();

    let diary_content = CONTENT_REGEX
        .captures(content)
        .and_then(|c| c.get(1).or_else(|| c.get(2)))
        .map(|m| m.as_str().trim().to_string())
        .unwrap_or_else(|| "[日记内容解析失败]".to_string());

    (maid, date, diary_content)
}

fn parse_tool_result(content: &str) -> (String, String, Vec<ToolResultDetail>, String) {
    let mut tool_name = "Unknown Tool".to_string();
    let mut status = "Unknown Status".to_string();
    let mut details = Vec::new();
    let mut footer_lines = Vec::new();

    let mut current_key: Option<String> = None;
    let mut current_value_lines: Vec<String> = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(captures) = KV_REGEX.captures(trimmed) {
            if let Some(key) = current_key.take() {
                let val = current_value_lines.join("\n").trim().to_string();
                if key == "工具名称" {
                    tool_name = val;
                } else if key == "执行状态" {
                    status = val;
                } else {
                    details.push(ToolResultDetail { key, value: val });
                }
            }
            if let (Some(key_match), Some(val_match)) = (captures.get(1), captures.get(2)) {
                current_key = Some(key_match.as_str().trim().to_string());
                current_value_lines = vec![val_match.as_str().trim().to_string()];
            }
        } else if current_key.is_some() {
            current_value_lines.push(line.to_string());
        } else if !trimmed.is_empty() {
            footer_lines.push(line.to_string());
        }
    }

    if let Some(key) = current_key {
        let val = current_value_lines.join("\n").trim().to_string();
        if key == "工具名称" {
            tool_name = val;
        } else if key == "执行状态" {
            status = val;
        } else {
            details.push(ToolResultDetail { key, value: val });
        }
    }

    (tool_name, status, details, footer_lines.join("\n"))
}

use lazy_static::lazy_static;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::hash::{Hash, Hasher};

use crate::vcp_modules::pre_renderer::MarkdownNode;

#[derive(Debug, Clone, Serialize, Deserialize, Hash)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "markdown")]
    Markdown {
        #[serde(skip_serializing_if = "Option::is_none")]
        content: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        nodes: Option<Vec<MarkdownNode>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        hash: Option<u64>,
    },
    #[serde(rename = "tool-use")]
    ToolUse {
        tool_name: String,
        content: String,
        is_complete: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        hash: Option<u64>,
    },
    #[serde(rename = "tool-result")]
    ToolResult {
        tool_name: String,
        status: String,
        details: Vec<ToolResultDetail>,
        footer: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        hash: Option<u64>,
    },
    #[serde(rename = "diary")]
    Diary {
        maid: String,
        date: String,
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        nodes: Option<Vec<MarkdownNode>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        hash: Option<u64>,
    },
    #[serde(rename = "thought")]
    Thought {
        theme: String,
        content: String,
        is_complete: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        nodes: Option<Vec<MarkdownNode>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        hash: Option<u64>,
    },
    #[serde(rename = "button-click")]
    ButtonClick {
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        hash: Option<u64>,
    },
    #[serde(rename = "html-preview")]
    HtmlPreview {
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        highlighted_content: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        hash: Option<u64>,
    },
    #[serde(rename = "role-divider")]
    RoleDivider {
        role: String,
        is_end: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        hash: Option<u64>,
    },
    #[serde(rename = "style")]
    Style {
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        hash: Option<u64>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, Hash)]
pub struct ToolResultDetail {
    pub key: String,
    pub value: String,
}

impl ContentBlock {
    pub fn markdown(content: Option<String>, nodes: Option<Vec<MarkdownNode>>) -> Self {
        Self::Markdown {
            content,
            nodes,
            hash: None,
        }
    }

    pub fn tool_use(tool_name: String, content: String, is_complete: bool) -> Self {
        Self::ToolUse {
            tool_name,
            content,
            is_complete,
            hash: None,
        }
    }

    pub fn tool_result(
        tool_name: String,
        status: String,
        details: Vec<ToolResultDetail>,
        footer: String,
    ) -> Self {
        Self::ToolResult {
            tool_name,
            status,
            details,
            footer,
            hash: None,
        }
    }

    pub fn diary(
        maid: String,
        date: String,
        content: String,
        nodes: Option<Vec<MarkdownNode>>,
    ) -> Self {
        Self::Diary {
            maid,
            date,
            content,
            nodes,
            hash: None,
        }
    }

    pub fn thought(
        theme: String,
        content: String,
        is_complete: bool,
        nodes: Option<Vec<MarkdownNode>>,
    ) -> Self {
        Self::Thought {
            theme,
            content,
            is_complete,
            nodes,
            hash: None,
        }
    }

    #[allow(dead_code)]
    pub fn button_click(content: String) -> Self {
        Self::ButtonClick {
            content,
            hash: None,
        }
    }

    pub fn html_preview(content: String) -> Self {
        let highlighted_content = crate::vcp_modules::chat::pre_renderer::code_highlighter::highlight_code_block(&content, "html");
        Self::HtmlPreview {
            content,
            highlighted_content,
            hash: None,
        }
    }

    pub fn role_divider(role: String, is_end: bool) -> Self {
        Self::RoleDivider {
            role,
            is_end,
            hash: None,
        }
    }

    pub fn style(content: String) -> Self {
        Self::Style {
            content,
            hash: None,
        }
    }

    pub fn compute_hash(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.hash(&mut hasher);
        hasher.finish()
    }

    pub fn set_hash(&mut self, h: u64) {
        match self {
            ContentBlock::Markdown { hash, .. } => *hash = Some(h),
            ContentBlock::ToolUse { hash, .. } => *hash = Some(h),
            ContentBlock::ToolResult { hash, .. } => *hash = Some(h),
            ContentBlock::Diary { hash, .. } => *hash = Some(h),
            ContentBlock::Thought { hash, .. } => *hash = Some(h),
            ContentBlock::ButtonClick { hash, .. } => *hash = Some(h),
            ContentBlock::HtmlPreview { hash, .. } => *hash = Some(h),
            ContentBlock::RoleDivider { hash, .. } => *hash = Some(h),
            ContentBlock::Style { hash, .. } => *hash = Some(h),
        }
    }

    pub fn compute_hashes_recursively(&mut self) {
        let h = self.compute_hash();
        self.set_hash(h);
    }
}

#[derive(Debug, PartialEq)]
pub(crate) enum BlockType {
    Tool,
    Thought,
    Think,
    ToolResult,
    Diary,
    HtmlFence,
    HtmlDoc,
    HtmlContainer,
    Style,
    RoleDivider,
    CodeFence,
}

lazy_static! {
    // 核心修复：为所有 VCP 块的起始标记强制增加行首锚定符 `(?im)^[ \t]*`
    // 这将彻底消除因正文提及 `<<<[TOOL_REQUEST]>>>` 等内联代码而引发的 AST 错误截断
    pub(crate) static ref TOOL_START: Regex = Regex::new(r"(?im)^[ \t]*<<<\[TOOL_REQUEST\]>>>").unwrap();
    pub(crate) static ref TOOL_END: Regex = Regex::new(r"(?im)^[ \t]*<<<\[END_TOOL_REQUEST\]>>>").unwrap();
    pub(crate) static ref TOOL_NAME: Regex = Regex::new(r"<tool_name>([\s\S]*?)</tool_name>|tool_name:\s*「始(?:exp)?」([^「」]*)「末(?:exp)?」").unwrap();

    pub(crate) static ref THOUGHT_START: Regex = Regex::new(r"(?im)^[ \t]*\[--- VCP元思考链(?::\s*([^\]]*?))?\s*---\]").unwrap();
    pub(crate) static ref THOUGHT_END: Regex = Regex::new(r"(?im)^[ \t]*\[--- 元思考链结束 ---\]").unwrap();

    pub(crate) static ref THINK_START: Regex = Regex::new(r"(?i)<think(?:ing)?>").unwrap();
    pub(crate) static ref THINK_END: Regex = Regex::new(r"(?i)</think(?:ing)?>").unwrap();

    pub(crate) static ref TOOL_RESULT_START: Regex = Regex::new(r"(?im)^[ \t]*\[\[VCP调用结果信息汇总:").unwrap();
    pub(crate) static ref TOOL_RESULT_END: Regex = Regex::new(r"(?im)^[ \t]*VCP调用结果结束\]\]").unwrap();

    pub(crate) static ref DIARY_START: Regex = Regex::new(r"(?im)^[ \t]*<<<DailyNoteStart>>>").unwrap();
    pub(crate) static ref DIARY_END: Regex = Regex::new(r"(?im)^[ \t]*<<<DailyNoteEnd>>>").unwrap();

    pub(crate) static ref BUTTON_CLICK: Regex = Regex::new(r"\[\[点击按钮:(.*?)\]\]").unwrap();

    pub(crate) static ref MAID_REGEX: Regex = Regex::new(r"(?:maid|maidName):\s*「始(?:exp)?」([^「」]*)「末(?:exp)?」|Maid:\s*([^\n\r]*)").unwrap();
    pub(crate) static ref DATE_REGEX: Regex = Regex::new(r"Date:\s*「始(?:exp)?」([^「」]*)「末(?:exp)?」|Date:\s*([^\n\r]*)").unwrap();
    pub(crate) static ref CONTENT_REGEX: Regex = Regex::new(r"Content:\s*「始(?:exp)?」([\s\S]*?)「末(?:exp)?」|Content:\s*([\s\S]*)").unwrap();

    pub(crate) static ref KV_REGEX: Regex = Regex::new(r"^-\s*([^:]+):\s*(.*)").unwrap();

    pub(crate) static ref HTML_FENCE_START: Regex = Regex::new(r"(?im)^[ \t]*```html[ \t]*$").unwrap();
    pub(crate) static ref HTML_FENCE_END: Regex = Regex::new(r"(?im)^[ \t]*```[ \t]*$").unwrap();

    // 修复：强行增加行首锚定符 ^，防止正文中的内联 `<!DOCTYPE html>` 触发解析截断
    pub(crate) static ref HTML_DOC_START: Regex = Regex::new(r"(?im)^[ \t]*(?:<!doctype html>|<html[\s>])").unwrap();
    pub(crate) static ref HTML_DOC_END: Regex = Regex::new(r"(?i)</html>").unwrap();

    pub(crate) static ref HTML_CONTAINER_OPEN_RE: Regex =
        Regex::new(r"(?im)^[ \t]*<(div|section|article|header|footer|main|aside|figure|figcaption)\b[^>]*>").unwrap();

    pub(crate) static ref ROLE_DIVIDER: Regex = Regex::new(r"(?im)^[ \t]*<<<\[(END_)?ROLE_DIVIDE_(SYSTEM|ASSISTANT|USER)\]>>>").unwrap();
    pub(crate) static ref STYLE_TAG_START: Regex = Regex::new(r"(?i)<style\b[^>]*>").unwrap();
    pub(crate) static ref STYLE_TAG_END: Regex = Regex::new(r"(?i)</style>").unwrap();

    pub(crate) static ref GENERIC_CODE_FENCE_START: Regex = Regex::new(r"(?im)^[ \t]*```[a-zA-Z0-9-]*[ \t]*$").unwrap();
    pub(crate) static ref GENERIC_CODE_FENCE_END: Regex = Regex::new(r"(?im)^[ \t]*```[ \t]*$").unwrap();


    static ref LIST_REGEX: Regex = Regex::new(r"^[ \t]*([-*]|\d+\.)[ \t]+").unwrap();
    static ref HTML_TAG_REGEX: Regex = Regex::new(r"(?i)^[ \t]*</?[a-zA-Z][a-zA-Z0-9]*[\s>/]").unwrap();
    static ref CHINESE_PARA_REGEX: Regex = Regex::new(r"^[\u4e00-\u9fa5]").unwrap();
    static ref VCP_SPECIAL_MARKER_REGEX: Regex = Regex::new(r"(?i)^(<<<|\[\[VCP|\[---|<think|</think)").unwrap();
}

pub fn de_indent_misinterpreted_code_blocks(text: &str) -> String {
    let mut in_fence = false;
    let mut result = Vec::new();

    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            result.push(trimmed.to_string());
            continue;
        }

        if in_fence {
            result.push(line.to_string());
            continue;
        }

        let has_indentation = line.len() > trimmed.len();
        if has_indentation {
            if LIST_REGEX.is_match(line) {
                result.push(line.to_string());
            } else if HTML_TAG_REGEX.is_match(line)
                || CHINESE_PARA_REGEX.is_match(trimmed)
                || VCP_SPECIAL_MARKER_REGEX.is_match(trimmed)
                || trimmed.starts_with("<!--")
            {
                result.push(trimmed.to_string());
            } else {
                result.push(line.to_string());
            }
        } else {
            result.push(line.to_string());
        }
    }

    result.join("\n")
}

/// 核心解析函数：将原始 Markdown 文本解析为 AST 块数组
pub fn parse_content(raw_text: &str) -> Vec<ContentBlock> {
    let deindented_text = de_indent_misinterpreted_code_blocks(raw_text);
    let text = deindented_text.as_str();

    let mut blocks = Vec::new();
    let mut current_pos = 0;

    // 预编译主匹配正则（包含所有特种块起始标记，利用捕获组编号识别类型）
    // 1: TOOL, 2: THOUGHT, 3: THINK, 4: TOOL_RESULT, 5: DIARY, 6: HTML_FENCE, 7: HTML_DOC, 8: ROLE_DIVIDER, 9: STYLE, 10: CODE_FENCE, 11: HTML_CONTAINER
    lazy_static! {
        static ref MASTER_START: Regex = Regex::new(concat!(
            r"(?im)",
            r"(^[ \t]*<<<\[TOOL_REQUEST\]>>>)|",                       // 1
            r"(^[ \t]*\[--- VCP元思考链(?::\s*[^\]]*?)?\s*---\])|",    // 2
            r"(<think(?:ing)?>)|",                                     // 3
            r"(^[ \t]*\[\[VCP调用结果信息汇总:)|",                     // 4
            r"(^[ \t]*<<<DailyNoteStart>>>)|",                         // 5
            r"(^[ \t]*```html[ \t]*$)|",                               // 6
            r"(^[ \t]*(?:<!doctype html>|<html[\s>]))|",               // 7
            r"(^[ \t]*<<<\[(?:END_)?ROLE_DIVIDE_(?:SYSTEM|ASSISTANT|USER)\]>>>)|", // 8
            r"(<style\b[^>]*>)|",                                      // 9
            r"(^[ \t]*```[a-zA-Z0-9-]*[ \t]*$)|",                       // 10
            r"(^[ \t]*<(div|section|article|header|footer|main|aside|figure|figcaption)\b[^>]*>)" // 11
        )).unwrap();
    }

    while current_pos < text.len() {
        let remaining = &text[current_pos..];

        if let Some(caps) = MASTER_START.captures(remaining) {
            let m = caps.get(0).unwrap();
            let start_idx = m.start();
            let end_idx = m.end();

            // 1. 将起始标记之前的文本作为 Markdown 块推入
            if start_idx > 0 {
                let md_text = &remaining[..start_idx];
                blocks.extend(parse_inline_blocks(md_text));
            }

            // 识别匹配到的块类型
            let mut container_tag = String::new();
            let block_type = if caps.get(1).is_some() {
                BlockType::Tool
            } else if caps.get(2).is_some() {
                BlockType::Thought
            } else if caps.get(3).is_some() {
                BlockType::Think
            } else if caps.get(4).is_some() {
                BlockType::ToolResult
            } else if caps.get(5).is_some() {
                BlockType::Diary
            } else if caps.get(6).is_some() {
                BlockType::HtmlFence
            } else if caps.get(7).is_some() {
                BlockType::HtmlDoc
            } else if caps.get(8).is_some() {
                BlockType::RoleDivider
            } else if caps.get(9).is_some() {
                BlockType::Style
            } else if caps.get(10).is_some() {
                BlockType::CodeFence
            } else {
                container_tag = caps.get(11).unwrap().as_str().to_lowercase();
                BlockType::HtmlContainer
            };

            // 2. 寻找对应的结束标记
            let content_start = end_idx;
            let search_area = &remaining[content_start..];

            let (end_marker_start, end_marker_end, is_complete) = match block_type {
                BlockType::Tool => TOOL_END.find(search_area).map_or((None, None, false), |m| {
                    (Some(m.start()), Some(m.end()), true)
                }),
                BlockType::Thought => THOUGHT_END
                    .find(search_area)
                    .map_or((None, None, false), |m| {
                        (Some(m.start()), Some(m.end()), true)
                    }),
                BlockType::Think => THINK_END
                    .find(search_area)
                    .map_or((None, None, false), |m| {
                        (Some(m.start()), Some(m.end()), true)
                    }),
                BlockType::ToolResult => TOOL_RESULT_END
                    .find(search_area)
                    .map_or((None, None, false), |m| {
                        (Some(m.start()), Some(m.end()), true)
                    }),
                BlockType::Diary => DIARY_END
                    .find(search_area)
                    .map_or((None, None, false), |m| {
                        (Some(m.start()), Some(m.end()), true)
                    }),
                BlockType::HtmlFence => HTML_FENCE_END
                    .find(search_area)
                    .map_or((None, None, false), |m| {
                        (Some(m.start()), Some(m.end()), true)
                    }),
                BlockType::HtmlDoc => HTML_DOC_END
                    .find(search_area)
                    .map_or((None, None, false), |m| {
                        (Some(m.start()), Some(m.end()), true)
                    }),
                BlockType::HtmlContainer => crate::vcp_modules::chat::pre_renderer::markdown_parser::find_matching_close_tag(remaining, content_start, &container_tag)
                    .map_or((None, None, false), |(s, e)| {
                        (Some(s - content_start), Some(e - content_start), true)
                    }),
                BlockType::RoleDivider => (Some(0), Some(0), true),
                BlockType::Style => STYLE_TAG_END
                    .find(search_area)
                    .map_or((None, None, false), |m| {
                        (Some(m.start()), Some(m.end()), true)
                    }),
                BlockType::CodeFence => GENERIC_CODE_FENCE_END
                    .find(search_area)
                    .map_or((None, None, false), |m| {
                        (Some(m.start()), Some(m.end()), true)
                    }),
            };

            // 容错处理：未闭合的块（流式中断）降级为普通 Markdown
            if !is_complete
                && !matches!(
                    block_type,
                    BlockType::HtmlFence
                        | BlockType::HtmlDoc
                        | BlockType::HtmlContainer
                        | BlockType::CodeFence
                        | BlockType::RoleDivider
                )
            {
                let marker_text = &remaining[start_idx..end_idx];
                blocks.push(ContentBlock::markdown(
                    None,
                    Some(crate::vcp_modules::pre_renderer::parse_markdown_to_ast(
                        marker_text,
                    )),
                ));
                current_pos += end_idx;
                continue;
            }

            let inner_content = if let Some(end_start) = end_marker_start {
                &search_area[..end_start]
            } else {
                search_area
            };

            // 3. 解析具体的块内容
            let block = match block_type {
                BlockType::Tool => {
                    let tool_name = extract_tool_name(inner_content);
                    if is_daily_note_create(inner_content) {
                        let (maid, date, content) = extract_diary_details(inner_content);
                        let nodes =
                            crate::vcp_modules::pre_renderer::parse_markdown_to_ast(&content);
                        ContentBlock::diary(maid, date, content, Some(nodes))
                    } else {
                        ContentBlock::tool_use(tool_name, inner_content.to_string(), is_complete)
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
                    ContentBlock::thought(
                        theme,
                        inner_content.to_string(),
                        is_complete,
                        Some(nodes),
                    )
                }
                BlockType::Think => {
                    let nodes =
                        crate::vcp_modules::pre_renderer::parse_markdown_to_ast(inner_content);
                    ContentBlock::thought(
                        "思维链".to_string(),
                        inner_content.to_string(),
                        is_complete,
                        Some(nodes),
                    )
                }
                BlockType::ToolResult => {
                    let (tool_name, status, details, footer) = parse_tool_result(inner_content);
                    ContentBlock::tool_result(tool_name, status, details, footer)
                }
                BlockType::Diary => {
                    let (maid, date, content) = extract_diary_details(inner_content);
                    let nodes = crate::vcp_modules::pre_renderer::parse_markdown_to_ast(&content);
                    ContentBlock::diary(maid, date, content, Some(nodes))
                }
                BlockType::HtmlFence => ContentBlock::html_preview(inner_content.to_string()),
                BlockType::HtmlDoc => {
                    let mut full_html = String::new();
                    full_html.push_str(&remaining[start_idx..end_idx]);
                    full_html.push_str(inner_content);
                    if is_complete {
                        if let (Some(s), Some(e)) = (end_marker_start, end_marker_end) {  
                            full_html.push_str(&search_area[s..e]);
                        }
                    }
                    ContentBlock::html_preview(full_html)
                }
                BlockType::HtmlContainer => {
                    let open_tag = &remaining[start_idx..end_idx];
                    let deindented_inner = crate::vcp_modules::chat::pre_renderer::markdown_parser::trim_common_leading_indent(inner_content);
                    let mut nodes = vec![crate::vcp_modules::pre_renderer::MarkdownNode::raw_html(open_tag.to_string())];
                    nodes.extend(crate::vcp_modules::pre_renderer::parse_markdown_to_ast(&deindented_inner));
                    if is_complete {
                        if let (Some(s), Some(e)) = (end_marker_start, end_marker_end) {
                            let close_tag = &search_area[s..e];
                            nodes.push(crate::vcp_modules::pre_renderer::MarkdownNode::raw_html(close_tag.to_string()));
                        }
                    }
                    ContentBlock::markdown(None, Some(nodes))
                }
                BlockType::RoleDivider => {                    let marker_text = &remaining[start_idx..end_idx];
                    if let Some(caps) = ROLE_DIVIDER.captures(marker_text) {
                        let is_end = caps.get(1).is_some();
                        let role = caps
                            .get(2)
                            .map(|m| m.as_str().to_lowercase())
                            .unwrap_or_default();
                        ContentBlock::role_divider(role, is_end)
                    } else {
                        ContentBlock::markdown(
                            None,
                            Some(crate::vcp_modules::pre_renderer::parse_markdown_to_ast(
                                marker_text,
                            )),
                        )
                    }
                }
                BlockType::Style => ContentBlock::style(inner_content.to_string()),
                BlockType::CodeFence => {
                    let mut full_fence = String::new();
                    full_fence.push_str(&remaining[start_idx..end_idx]);
                    full_fence.push_str(inner_content);
                    if is_complete {
                        if let (Some(s), Some(e)) = (end_marker_start, end_marker_end) {
                            full_fence.push_str(&search_area[s..e]);
                        }
                    }
                    ContentBlock::markdown(
                        None,
                        Some(crate::vcp_modules::pre_renderer::parse_markdown_to_ast(
                            &full_fence,
                        )),
                    )
                }
            };

            blocks.push(block);

            // 4. 更新游标
            if let Some(end_end) = end_marker_end {
                current_pos += content_start + end_end;
            } else {
                break;
            }
        } else {
            // 没有找到任何特种块，剩余部分全部作为 Markdown 处理
            blocks.extend(parse_inline_blocks(remaining));
            break;
        }
    }

    // 计算全量块的稳定哈希指纹
    for block in &mut blocks {
        block.compute_hashes_recursively();
    }

    blocks
}

/// 解析内联块（如按钮点击）
fn parse_inline_blocks(text: &str) -> Vec<ContentBlock> {
    let mut blocks = Vec::new();
    let mut last_end = 0;

    for cap in BUTTON_CLICK.captures_iter(text) {
        let Some(m) = cap.get(0) else { continue };
        let Some(button_content) = cap.get(1) else {
            continue;
        };
        if m.start() > last_end {
            blocks.push(ContentBlock::markdown(
                None,
                Some(crate::vcp_modules::pre_renderer::parse_markdown_to_ast(
                    &text[last_end..m.start()],
                )),
            ));
        }
        blocks.push(ContentBlock::button_click(
            button_content.as_str().trim().to_string(),
        ));
        last_end = m.end();
    }

    if last_end < text.len() {
        blocks.push(ContentBlock::markdown(
            None,
            Some(crate::vcp_modules::pre_renderer::parse_markdown_to_ast(
                &text[last_end..],
            )),
        ));
    }

    blocks
}

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

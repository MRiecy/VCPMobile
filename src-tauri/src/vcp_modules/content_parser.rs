use lazy_static::lazy_static;
use regex::Regex;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "markdown")]
    Markdown { content: String },
    #[serde(rename = "tool-use")]
    ToolUse {
        tool_name: String,
        content: String,
        is_complete: bool,
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
    },
    #[serde(rename = "thought")]
    Thought {
        theme: String,
        content: String,
        is_complete: bool,
    },
    #[serde(rename = "button-click")]
    ButtonClick { content: String },
    #[serde(rename = "html-preview")]
    HtmlPreview { content: String },
    #[serde(rename = "role-divider")]
    RoleDivider { role: String, is_end: bool },
    #[serde(rename = "style")]
    Style { content: String },
    #[serde(rename = "math")]
    Math { content: String, display_mode: bool },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultDetail {
    pub key: String,
    pub value: String,
}

#[derive(Debug, PartialEq)]
enum BlockType {
    Tool,
    Thought,
    Think,
    ToolResult,
    Diary,
    HtmlFence,
    HtmlDoc,
    Style,
    RoleDivider,
    CodeFence,
    Math,
}

lazy_static! {
    // 核心修复：为所有 VCP 块的起始标记强制增加行首锚定符 `(?im)^[ \t]*`
    // 这将彻底消除因正文提及 `<<<[TOOL_REQUEST]>>>` 等内联代码而引发的 AST 错误截断
    static ref TOOL_START: Regex = Regex::new(r"(?im)^[ \t]*<<<\[TOOL_REQUEST\]>>>").unwrap();
    static ref TOOL_END: Regex = Regex::new(r"(?im)^[ \t]*<<<\[END_TOOL_REQUEST\]>>>").unwrap();
    static ref TOOL_NAME: Regex = Regex::new(r"<tool_name>([\s\S]*?)</tool_name>|tool_name:\s*「始(?:exp)?」([^「」]*)「末(?:exp)?」").unwrap();

    static ref THOUGHT_START: Regex = Regex::new(r"(?im)^[ \t]*\[--- VCP元思考链(?::\s*([^\]]*?))?\s*---\]").unwrap();
    static ref THOUGHT_END: Regex = Regex::new(r"(?im)^[ \t]*\[--- 元思考链结束 ---\]").unwrap();

    static ref THINK_START: Regex = Regex::new(r"(?i)<think(?:ing)?>").unwrap();
    static ref THINK_END: Regex = Regex::new(r"(?i)</think(?:ing)?>").unwrap();

    static ref TOOL_RESULT_START: Regex = Regex::new(r"(?im)^[ \t]*\[\[VCP调用结果信息汇总:").unwrap();
    static ref TOOL_RESULT_END: Regex = Regex::new(r"(?im)^[ \t]*VCP调用结果结束\]\]").unwrap();

    static ref DIARY_START: Regex = Regex::new(r"(?im)^[ \t]*<<<DailyNoteStart>>>").unwrap();
    static ref DIARY_END: Regex = Regex::new(r"(?im)^[ \t]*<<<DailyNoteEnd>>>").unwrap();

    static ref BUTTON_CLICK: Regex = Regex::new(r"\[\[点击按钮:(.*?)\]\]").unwrap();

    static ref MAID_REGEX: Regex = Regex::new(r"(?:maid|maidName):\s*「始(?:exp)?」([^「」]*)「末(?:exp)?」|Maid:\s*([^\n\r]*)").unwrap();
    static ref DATE_REGEX: Regex = Regex::new(r"Date:\s*「始(?:exp)?」([^「」]*)「末(?:exp)?」|Date:\s*([^\n\r]*)").unwrap();
    static ref CONTENT_REGEX: Regex = Regex::new(r"Content:\s*「始(?:exp)?」([\s\S]*?)「末(?:exp)?」|Content:\s*([\s\S]*)").unwrap();

    static ref KV_REGEX: Regex = Regex::new(r"^-\s*([^:]+):\s*(.*)").unwrap();

    static ref HTML_FENCE_START: Regex = Regex::new(r"(?im)^[ \t]*```html[ \t]*$").unwrap();
    static ref HTML_FENCE_END: Regex = Regex::new(r"(?im)^[ \t]*```[ \t]*$").unwrap();

    // 修复：强行增加行首锚定符 ^，防止正文中的内联 `<!DOCTYPE html>` 触发解析截断
    static ref HTML_DOC_START: Regex = Regex::new(r"(?im)^[ \t]*(?:<!doctype html>|<html[\s>])").unwrap();
    static ref HTML_DOC_END: Regex = Regex::new(r"(?i)</html>").unwrap();

    static ref ROLE_DIVIDER: Regex = Regex::new(r"(?im)^[ \t]*<<<\[(END_)?ROLE_DIVIDE_(SYSTEM|ASSISTANT|USER)\]>>>").unwrap();
    static ref STYLE_TAG_START: Regex = Regex::new(r"(?i)<style\b[^>]*>").unwrap();
    static ref STYLE_TAG_END: Regex = Regex::new(r"(?i)</style>").unwrap();

    static ref GENERIC_CODE_FENCE_START: Regex = Regex::new(r"(?im)^[ \t]*```[a-zA-Z0-9-]*[ \t]*$").unwrap();
    static ref GENERIC_CODE_FENCE_END: Regex = Regex::new(r"(?im)^[ \t]*```[ \t]*$").unwrap();

    static ref MATH_BLOCK_START: Regex = Regex::new(r"(?im)^[ \t]*(\$\$|\\\[|\\begin\{([a-z]+\*?)\})").unwrap();

    static ref LIST_REGEX: Regex = Regex::new(r"^[ \t]*([-*]|\d+\.)[ \t]+").unwrap();
    static ref HTML_TAG_REGEX: Regex = Regex::new(r"(?i)^[ \t]*</?(div|p|img|span|a|h[1-6]|ul|ol|li|table|tr|td|th|section|article|header|footer|nav|aside|main|figure|figcaption|blockquote|pre|code|style|script|button|form|input|textarea|select|label|iframe|video|audio|canvas|svg)[\s>/]").unwrap();
    static ref CHINESE_PARA_REGEX: Regex = Regex::new(r"^[\u4e00-\u9fa5]").unwrap();
    static ref VCP_SPECIAL_MARKER_REGEX: Regex = Regex::new(r"(?i)^(<<<|\[\[VCP|\[---|<think|</think)").unwrap();

    static ref HTML_RE_START: Regex = Regex::new(r"(?im)^[ \t]*(?:<!doctype html>|<html[\s>])").unwrap();
    static ref HTML_RE_END: Regex = Regex::new(r"(?i)</html>").unwrap();
    static ref HTML_RE_FENCE: Regex = Regex::new(r"(?m)^[ \t]*```").unwrap();
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

    while current_pos < text.len() {
        let remaining = &text[current_pos..];

        // 寻找最早出现的特种块起始标记
        let mut earliest_match: Option<(usize, usize, BlockType)> = None;

        let checks = [
            (TOOL_START.find(remaining), BlockType::Tool),
            (THOUGHT_START.find(remaining), BlockType::Thought),
            (THINK_START.find(remaining), BlockType::Think),
            (TOOL_RESULT_START.find(remaining), BlockType::ToolResult),
            (DIARY_START.find(remaining), BlockType::Diary),
            (HTML_FENCE_START.find(remaining), BlockType::HtmlFence),
            (HTML_DOC_START.find(remaining), BlockType::HtmlDoc),
            (ROLE_DIVIDER.find(remaining), BlockType::RoleDivider),
            (STYLE_TAG_START.find(remaining), BlockType::Style),
            (MATH_BLOCK_START.find(remaining), BlockType::Math),
            (
                GENERIC_CODE_FENCE_START.find(remaining),
                BlockType::CodeFence,
            ),
        ];

        for (m_opt, b_type) in checks {
            if let Some(m) = m_opt {
                if earliest_match
                    .as_ref()
                    .is_none_or(|(start, _, _)| m.start() < *start)
                {
                    earliest_match = Some((m.start(), m.end(), b_type));
                }
            }
        }

        match earliest_match {
            Some((start_idx, end_idx, block_type)) => {
                // 1. 将起始标记之前的文本作为 Markdown 块推入
                if start_idx > 0 {
                    let md_text = &remaining[..start_idx];
                    blocks.extend(parse_inline_blocks(md_text));
                }

                // 2. 寻找对应的结束标记
                let content_start = end_idx;
                let search_area = &remaining[content_start..];

                let (end_marker_start, end_marker_end, is_complete) = match block_type {
                    BlockType::Tool => {
                        TOOL_END.find(search_area).map_or((None, None, false), |m| {
                            (Some(m.start()), Some(m.end()), true)
                        })
                    }
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
                    BlockType::RoleDivider => (Some(0), Some(0), true), // RoleDivider is a single line marker
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
                    BlockType::Math => {
                        let start_marker = &remaining[start_idx..end_idx];
                        let trimmed = start_marker.trim();
                        if trimmed.starts_with("\\begin") {
                            MATH_BLOCK_START
                                .captures(start_marker)
                                .and_then(|c| c.get(2))
                                .and_then(|m| {
                                    let env_name = m.as_str();
                                    let end_str = format!("\\end{{{}}}", env_name);
                                    let end_len = end_str.len();
                                    search_area
                                        .find(&end_str)
                                        .map(|pos| (Some(pos), Some(pos + end_len), true))
                                })
                                .unwrap_or((None, None, false))
                        } else if trimmed.starts_with("$$") {
                            search_area
                                .find("$$")
                                .map_or((None, None, false), |pos| (Some(pos), Some(pos + 2), true))
                        } else {
                            search_area
                                .find("\\]")
                                .map_or((None, None, false), |pos| (Some(pos), Some(pos + 2), true))
                        }
                    }
                };

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
                            ContentBlock::Diary {
                                maid,
                                date,
                                content,
                            }
                        } else {
                            ContentBlock::ToolUse {
                                tool_name,
                                content: inner_content.to_string(),
                                is_complete,
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

                        ContentBlock::Thought {
                            theme,
                            content: inner_content.to_string(),
                            is_complete,
                        }
                    }
                    BlockType::Think => ContentBlock::Thought {
                        theme: "思维链".to_string(),
                        content: inner_content.to_string(),
                        is_complete,
                    },
                    BlockType::ToolResult => {
                        let (tool_name, status, details, footer) = parse_tool_result(inner_content);
                        ContentBlock::ToolResult {
                            tool_name,
                            status,
                            details,
                            footer,
                        }
                    }
                    BlockType::Diary => {
                        let (maid, date, content) = extract_diary_details(inner_content);
                        ContentBlock::Diary {
                            maid,
                            date,
                            content,
                        }
                    }
                    BlockType::HtmlFence => ContentBlock::HtmlPreview {
                        content: inner_content.to_string(),
                    },
                    BlockType::HtmlDoc => {
                        let mut full_html = String::new();
                        full_html.push_str(&remaining[start_idx..end_idx]);
                        full_html.push_str(inner_content);
                        if is_complete {
                            if let (Some(s), Some(e)) = (end_marker_start, end_marker_end) {
                                full_html.push_str(&search_area[s..e]);
                            }
                        }
                        ContentBlock::HtmlPreview { content: full_html }
                    }
                    BlockType::RoleDivider => {
                        let marker_text = &remaining[start_idx..end_idx];
                        if let Some(caps) = ROLE_DIVIDER.captures(marker_text) {
                            let is_end = caps.get(1).is_some();
                            let role = caps
                                .get(2)
                                .map(|m| m.as_str().to_lowercase())
                                .unwrap_or_default();
                            ContentBlock::RoleDivider { role, is_end }
                        } else {
                            ContentBlock::Markdown {
                                content: marker_text.to_string(),
                            }
                        }
                    }
                    BlockType::Style => ContentBlock::Style {
                        content: inner_content.to_string(),
                    },
                    BlockType::CodeFence => {
                        let mut full_fence = String::new();
                        full_fence.push_str(&remaining[start_idx..end_idx]);
                        full_fence.push_str(inner_content);
                        if is_complete {
                            if let (Some(s), Some(e)) = (end_marker_start, end_marker_end) {
                                full_fence.push_str(&search_area[s..e]);
                            }
                        }
                        ContentBlock::Markdown {
                            content: full_fence,
                        }
                    }
                    BlockType::Math => {
                        let start_marker = &remaining[start_idx..end_idx];
                        let trimmed = start_marker.trim();
                        if trimmed.starts_with("\\begin") {
                            let math_content = if let Some(end_end) = end_marker_end {
                                remaining[start_idx..content_start + end_end].to_string()
                            } else {
                                remaining[start_idx..].to_string()
                            };
                            ContentBlock::Math {
                                content: math_content,
                                display_mode: true,
                            }
                        } else {
                            ContentBlock::Math {
                                content: inner_content.trim().to_string(),
                                display_mode: true,
                            }
                        }
                    }
                };

                blocks.push(block);

                // 4. 更新游标
                if let Some(end_end) = end_marker_end {
                    current_pos += content_start + end_end;
                } else {
                    // 如果是不完整的块（流式传输中），直接结束解析
                    break;
                }
            }
            None => {
                // 没有找到任何特种块，剩余部分全部作为 Markdown 处理
                blocks.extend(parse_inline_blocks(remaining));
                break;
            }
        }
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
            blocks.push(ContentBlock::Markdown {
                content: text[last_end..m.start()].to_string(),
            });
        }
        blocks.push(ContentBlock::ButtonClick {
            content: button_content.as_str().trim().to_string(),
        });
        last_end = m.end();
    }

    if last_end < text.len() {
        blocks.push(ContentBlock::Markdown {
            content: text[last_end..].to_string(),
        });
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

/// 预处理：确保裸露的 HTML（包含 DOCTYPE 或完整的 html 标签）被 Markdown 代码块包裹
pub fn ensure_html_fenced(text: &str) -> String {
    let mut result = String::new();
    let mut last_pos = 0;

    // 寻找所有的 HTML 起始标记
    for m_start in HTML_RE_START.find_iter(text) {
        if m_start.start() < last_pos {
            continue;
        }

        // 检查在该起始标记之前，处于未闭合状态的 ``` 数量
        let prefix = &text[..m_start.start()];
        let fence_count = HTML_RE_FENCE.find_iter(prefix).count();

        // 如果 fence_count 是奇数，说明当前处于代码块内部，跳过
        if !fence_count.is_multiple_of(2) {
            continue;
        }

        // 寻找配对的结束标记
        if let Some(m_end) = HTML_RE_END.find(&text[m_start.start()..]) {
            let end_pos = m_start.start() + m_end.end();

            // 将之前的文本加入结果
            result.push_str(&text[last_pos..m_start.start()]);

            // 包裹 HTML
            result.push_str("\n```html\n");
            result.push_str(&text[m_start.start()..end_pos]);
            result.push_str("\n```\n");

            last_pos = end_pos;
        }
    }

    result.push_str(&text[last_pos..]);
    result
}

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
        #[serde(default, skip_serializing_if = "String::is_empty")]
        valet: String,
        date: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        file_name: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        folder: String,
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        nodes: Option<Vec<MarkdownNode>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        hash: Option<u64>,
    },
    #[serde(rename = "diary-update")]
    DiaryUpdate {
        maid: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        valet: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        folder: String,
        target: String,
        replace: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        target_nodes: Option<Vec<MarkdownNode>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        replace_nodes: Option<Vec<MarkdownNode>>,
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
    #[serde(rename = "tool-call-summary")]
    ToolCallSummary {
        items: Vec<ToolCallSummaryItem>,
        raw_content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        hash: Option<u64>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, Hash)]
pub struct ToolCallSummaryItem {
    pub tool_name: String,
    pub status: String,
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
        valet: String,
        date: String,
        file_name: String,
        folder: String,
        content: String,
        nodes: Option<Vec<MarkdownNode>>,
    ) -> Self {
        Self::Diary {
            maid,
            valet,
            date,
            file_name,
            folder,
            content,
            nodes,
            hash: None,
        }
    }

    pub fn diary_update(
        maid: String,
        valet: String,
        folder: String,
        target: String,
        replace: String,
        target_nodes: Option<Vec<MarkdownNode>>,
        replace_nodes: Option<Vec<MarkdownNode>>,
    ) -> Self {
        Self::DiaryUpdate {
            maid,
            valet,
            folder,
            target,
            replace,
            target_nodes,
            replace_nodes,
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
        // 在流结束后沉淀或全量重新编译时，做终态 classed 高亮预渲染（HTML 外壳），生成不含 style 的 DOM
        let highlighted_content =
            crate::vcp_modules::chat::pre_renderer::code_highlighter::highlight_code_block(
                &content,
                "html",
                crate::vcp_modules::chat::pre_renderer::code_highlighter::CodeBlockShell::Html,
            );
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

    pub fn tool_call_summary(items: Vec<ToolCallSummaryItem>, raw_content: String) -> Self {
        Self::ToolCallSummary {
            items,
            raw_content,
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
            ContentBlock::DiaryUpdate { hash, .. } => *hash = Some(h),
            ContentBlock::Thought { hash, .. } => *hash = Some(h),
            ContentBlock::ButtonClick { hash, .. } => *hash = Some(h),
            ContentBlock::HtmlPreview { hash, .. } => *hash = Some(h),
            ContentBlock::RoleDivider { hash, .. } => *hash = Some(h),
            ContentBlock::Style { hash, .. } => *hash = Some(h),
            ContentBlock::ToolCallSummary { hash, .. } => *hash = Some(h),
        }
    }

    pub fn compute_hashes_recursively(&mut self) {
        let h = self.compute_hash();
        self.set_hash(h);
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum BlockType {
    Tool,
    Thought,
    Think,
    ToolResult,
    Diary,
    HtmlDoc,
    HtmlContainer,
    Style,
    RoleDivider,
    CodeFence,
    ToolCallSummary,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ParsedDailyNote {
    Create {
        maid: String,
        valet: String,
        date: String,
        file_name: String,
        folder: String,
        content: String,
    },
    Update {
        maid: String,
        valet: String,
        folder: String,
        target: String,
        replace: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MarkedFieldMode {
    Normal,
    Escape,
    Exp,
}

lazy_static! {
    // 核心修复：为所有 VCP 块的起始标记强制增加行首锚定符 `(?im)^[ \t]*`
    // 这将彻底消除因正文提及 `<<<[TOOL_REQUEST]>>>` 等内联代码而引发的 AST 错误截断
    pub(crate) static ref TOOL_START: Regex = Regex::new(r"(?im)^[ \t]*<<<\[TOOL_REQUEST\]>>>").unwrap();
    pub(crate) static ref TOOL_END: Regex = Regex::new(r"(?im)^[ \t]*<<<\[END_TOOL_REQUEST\]>>>").unwrap();
    pub(crate) static ref TOOL_NAME_XML: Regex = Regex::new(r"(?is)<tool_name>(.*?)</tool_name>").unwrap();
    static ref MARKED_FIELD_START: Regex = Regex::new(r"(?i)[「{]始(?:escape|exp)?[」}]").unwrap();
    static ref MARKED_FIELD_END: Regex = Regex::new(r"(?i)[「{]末(?:escape|exp)?[」}]").unwrap();

    pub(crate) static ref THOUGHT_START: Regex = Regex::new(r"(?im)^[ \t]*\[--- VCP元思考链(?::\s*([^\]]*?))?\s*---\]").unwrap();
    pub(crate) static ref THOUGHT_END: Regex = Regex::new(r"(?im)^[ \t]*\[--- 元思考链结束 ---\]").unwrap();

    pub(crate) static ref THINK_START: Regex = Regex::new(r"(?i)<think(?:ing)?>").unwrap();
    pub(crate) static ref THINK_END: Regex = Regex::new(r"(?i)</think(?:ing)?>").unwrap();

    pub(crate) static ref TOOL_RESULT_START: Regex = Regex::new(r"(?im)^[ \t]*\[\[VCP调用结果信息汇总:").unwrap();
    pub(crate) static ref TOOL_RESULT_END: Regex = Regex::new(r"(?im)^[ \t]*VCP调用结果结束\]\]").unwrap();

    pub(crate) static ref DIARY_START: Regex = Regex::new(r"(?im)^[ \t]*<<<DailyNoteStart>>>").unwrap();
    pub(crate) static ref DIARY_END: Regex = Regex::new(r"(?im)^[ \t]*<<<DailyNoteEnd>>>").unwrap();

    pub(crate) static ref BUTTON_CLICK: Regex = Regex::new(r"\[\[点击按钮:(.*?)\]\]").unwrap();

    pub(crate) static ref KV_REGEX: Regex = Regex::new(r"^-\s*([^:]+):\s*(.*)").unwrap();

    // 修复：强行增加行首锚定符 ^，防止正文中的内联 `<!DOCTYPE html>` 触发解析截断
    pub(crate) static ref HTML_DOC_START: Regex = Regex::new(r"(?im)^[ \t]*(?:<!doctype html>|<html[\s>])").unwrap();
    pub(crate) static ref HTML_DOC_END: Regex = Regex::new(r"(?i)</html>").unwrap();

    pub(crate) static ref HTML_CONTAINER_OPEN_RE: Regex =
        Regex::new(r"(?im)^[ \t]*<(div|section|article|header|footer|main|aside|figure|figcaption)\b[^>]*>").unwrap();

    pub(crate) static ref ROLE_DIVIDER: Regex = Regex::new(r"(?im)^[ \t]*<<<\[(END_)?ROLE_DIVIDE_(SYSTEM|ASSISTANT|USER)\]>>>").unwrap();
    pub(crate) static ref STYLE_TAG_START: Regex = Regex::new(r"(?im)^[ \t]*<style\b[^>]*>?").unwrap();
    pub(crate) static ref STYLE_TAG_END: Regex = Regex::new(r"(?i)</style>").unwrap();
    pub(crate) static ref TOOL_CALL_SUMMARY_START: Regex = Regex::new(r"(?im)^[ \t]*\[本轮工具调用摘要:\]").unwrap();
    pub(crate) static ref TOOL_CALL_SUMMARY_END: Regex = Regex::new(r"(?im)^[ \t]*\[本轮工具调用摘要结束\]").unwrap();

    // 起始标记兼容 ≥3 反引号（CommonMark 允许任意长度围栏）；结束配对必须 ≥ 开围栏数，
    // 由 find_matching_fence_end 按起始标记动态计数，不再使用固定三反引号的结束正则。
    pub(crate) static ref GENERIC_CODE_FENCE_START: Regex = Regex::new(r"(?im)^[ \t]*`{3,}[a-zA-Z0-9-]*[ \t]*\r?$").unwrap();


    static ref LIST_REGEX: Regex = Regex::new(r"^[ \t]*([-*]|\d+\.)[ \t]+").unwrap();
    static ref HTML_TAG_REGEX: Regex = Regex::new(r"(?i)^[ \t]*</?[a-zA-Z][a-zA-Z0-9]*[\s>/]").unwrap();
}

/// 检测字符是否为自然语言的起始字符（CJK / 日文 / 韩文 / 标点）。
///
/// 覆盖以下 Unicode 区块：
///   U+2E80..U+9FFF  CJK Radicals → Unified Ideographs（大部分东亚文字）
///   U+AC00..U+D7AF  Hangul Syllables（韩文）
///   U+F900..U+FAFF  CJK Compatibility Ideographs
///   U+FE30..U+FE4F  CJK Compatibility Forms
///   U+FF01..U+FF60  Fullwidth Forms（全角标点+字母）
///   U+FFE0..U+FFE6  Fullwidth Signs
///   若干常用 Curly Quotes / Em-Dash / Ellipsis
#[inline]
fn is_natural_language_line_start(c: char) -> bool {
    ('\u{2E80}'..='\u{9FFF}').contains(&c)
        || ('\u{AC00}'..='\u{D7AF}').contains(&c)
        || ('\u{F900}'..='\u{FAFF}').contains(&c)
        || ('\u{FE30}'..='\u{FE4F}').contains(&c)
        || ('\u{FF00}'..='\u{FFEF}').contains(&c)
        || ('\u{FFE0}'..='\u{FFE6}').contains(&c)
        || ('\u{2000}'..='\u{206F}').contains(&c)
        || ('\u{25A0}'..='\u{25FF}').contains(&c)
        || c == '\u{00B7}'
}

#[inline]
fn is_vcp_marker(s: &str) -> bool {
    s.starts_with("<<<")
        || s.starts_with("[---")
        || (s.len() >= 5 && s.is_char_boundary(5) && s[..5].eq_ignore_ascii_case("[[vcp"))
        || (s.len() >= 6 && s.is_char_boundary(6) && s[..6].eq_ignore_ascii_case("<think"))
        || (s.len() >= 7 && s.is_char_boundary(7) && s[..7].eq_ignore_ascii_case("</think"))
}

pub fn de_indent_misinterpreted_code_blocks(text: &str) -> String {
    let mut result = String::with_capacity(text.len());

    // 预先检测所有代码围栏的行索引范围
    let lines: Vec<&str> = text.lines().collect();
    let num_lines = lines.len();
    let mut is_inside_fence = vec![false; num_lines];
    let mut temp_in_fence = false;

    for i in 0..num_lines {
        let trimmed = lines[i].trim_start();
        if trimmed.starts_with("```") {
            temp_in_fence = !temp_in_fence;
            is_inside_fence[i] = true; // 围栏行本身也算作围栏内
        } else if temp_in_fence {
            is_inside_fence[i] = true;
        }
    }

    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            result.push('\n');
        }

        // 如果是代码围栏内部的行，绝对不进行任何去缩进清洗，原样保留
        if is_inside_fence[i] {
            result.push_str(line);
            continue;
        }

        let trimmed = line.trim_start();

        let has_indentation = line.len() > trimmed.len();
        if has_indentation {
            if LIST_REGEX.is_match(line) {
                result.push_str(line);
            } else if (trimmed.starts_with('<') && HTML_TAG_REGEX.is_match(trimmed))
                || trimmed
                    .chars()
                    .next()
                    .is_some_and(is_natural_language_line_start)
                || is_vcp_marker(trimmed)
                || trimmed.starts_with("<!--")
            {
                result.push_str(trimmed);
            } else {
                result.push_str(line);
            }
        } else {
            result.push_str(line);
        }
    }

    result
}

pub(crate) fn find_matching_fence_end(
    search_area: &str,
    start_marker_text: &str,
) -> (Option<usize>, Option<usize>, bool) {
    let trimmed = start_marker_text.trim_start();
    let fence_char = match trimmed.chars().next() {
        Some(c) if c == '`' => c,
        _ => return (None, None, false),
    };
    let count = trimmed.chars().take_while(|&c| c == fence_char).count();
    if count < 3 {
        return (None, None, false);
    }

    // 手写行扫描，与原正则 `(?m)^[ \t]{0,3}\`{count,}[ \t]*\r?$` 语义等价：
    // 结束围栏 = 行首至多 3 个空格/制表符 + 不少于开围栏数的反引号 + 仅空白收尾。
    // 原实现每次调用都现编译一个新 Regex，而本函数处于流式热路径（未闭合代码块的每一帧
    // 都会调一次），正则编译开销远超扫描本身。
    let mut line_start = 0usize;
    for line in search_area.split_inclusive('\n') {
        let content = line.strip_suffix('\n').unwrap_or(line);
        let bytes = content.as_bytes();

        let mut i = 0;
        while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
            i += 1;
        }
        if i > 3 {
            line_start += line.len();
            continue;
        }

        let backticks = bytes[i..].iter().take_while(|&&b| b == b'`').count();
        if backticks >= count {
            let rest = &bytes[i + backticks..];
            let rest = match rest.last() {
                Some(b'\r') => &rest[..rest.len() - 1],
                _ => rest,
            };
            if rest.iter().all(|&b| b == b' ' || b == b'\t') {
                return (Some(line_start), Some(line_start + content.len()), true);
            }
        }

        line_start += line.len();
    }

    (None, None, false)
}

/// 核心解析函数：将原始 Markdown 文本解析为 AST 块数组
pub fn parse_content(raw_text: &str) -> Vec<ContentBlock> {
    let deindented_text = de_indent_misinterpreted_code_blocks(raw_text);
    let text = deindented_text.as_str();

    let mut blocks = Vec::new();
    let mut current_pos = 0;

    // 预编译主匹配正则（包含所有特种块起始标记，利用捕获组编号识别类型）
    // html 围栏不再是特例：与普通代码围栏共用组 9 命中，由 CodeFence 分支统一走 pulldown 识别
    // 1: TOOL, 2: THOUGHT, 3: THINK, 4: TOOL_RESULT, 5: DIARY, 6: HTML_DOC, 7: ROLE_DIVIDER, 8: STYLE, 9: CODE_FENCE, 10: HTML_CONTAINER, 12: TOOL_CALL_SUMMARY
    lazy_static! {
        static ref MASTER_START: Regex = Regex::new(concat!(
            r"(?im)",
            r"(^[ \t]*<<<\[TOOL_REQUEST\]>>>)|",                       // 1
            r"(^[ \t]*\[--- VCP元思考链(?::\s*[^\]]*?)?\s*---\])|",    // 2
            r"(<think(?:ing)?>)|",                                     // 3
            r"(^[ \t]*\[\[VCP调用结果信息汇总:)|",                     // 4
            r"(^[ \t]*<<<DailyNoteStart>>>)|",                         // 5
            r"(^[ \t]*(?:<!doctype html>|<html[\s>]))|",               // 6
            r"(^[ \t]*<<<\[(?:END_)?ROLE_DIVIDE_(?:SYSTEM|ASSISTANT|USER)\]>>>)|", // 7
            r"(^[ \t]*<style\b[^>]*>)|",                                      // 8
            r"(^[ \t]*`{3,}[a-zA-Z0-9-]*[ \t]*$)|",                    // 9
            r"(^[ \t]*<(div|section|article|header|footer|main|aside|figure|figcaption)\b[^>]*>)|", // 10
            r"(^[ \t]*\[本轮工具调用摘要:\])"                          // 12
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
                if md_text.contains("[[点击按钮:") {
                    blocks.extend(parse_inline_blocks(md_text));
                } else {
                    blocks.push(ContentBlock::markdown(
                        None,
                        Some(crate::vcp_modules::pre_renderer::parse_markdown_to_ast(
                            md_text,
                        )),
                    ));
                }
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
                BlockType::HtmlDoc
            } else if caps.get(7).is_some() {
                BlockType::RoleDivider
            } else if caps.get(8).is_some() {
                BlockType::Style
            } else if caps.get(9).is_some() {
                BlockType::CodeFence
            } else if caps.get(12).is_some() {
                BlockType::ToolCallSummary
            } else {
                container_tag = caps.get(11).unwrap().as_str().to_lowercase();
                BlockType::HtmlContainer
            };

            // 代码围栏（含 html 围栏）不进入正则定界流程：定界与内容提取统一交给
            // pulldown（CommonMark 语义，嵌套围栏按反引号数配对，天然无换行工件）。
            // lang 为 html 时整块转为全预览卡片；非 html 直接用已提取的 (lang, code)
            // 构造终态节点，不再把围栏全文重走一遍完整 Markdown 管线。
            if matches!(block_type, BlockType::CodeFence) {
                let fence_region = &remaining[start_idx..];
                if let Some((lang, code, block_len)) =
                    crate::vcp_modules::chat::pre_renderer::markdown_parser::parse_fenced_code_block(fence_region)
                {
                    let block = if lang.eq_ignore_ascii_case("html") {
                        ContentBlock::html_preview(code)
                    } else {
                        let mut node =
                            crate::vcp_modules::chat::pre_renderer::markdown_parser::finalized_code_block_node(
                                Some(lang),
                                code,
                            );
                        node.compute_hashes_recursively();
                        ContentBlock::markdown(None, Some(vec![node]))
                    };
                    blocks.push(block);
                    current_pos += start_idx + block_len;
                    continue;
                }
                // pulldown 不认可的起始（如缩进 ≥4，规范上属于缩进代码块）：
                // 回落到下方旧正则定界路径，保持历史行为。
            }

            // 2. 寻找对应的结束标记
            let content_start = end_idx;
            let search_area = &remaining[content_start..];

            let start_marker_text = &remaining[start_idx..end_idx];
            let (end_marker_start, end_marker_end, is_complete) = match block_type {
                BlockType::Tool => find_tool_request_end(search_area)
                    .map_or((None, None, false), |(start, end)| {
                        (Some(start), Some(end), true)
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
                BlockType::ToolCallSummary => TOOL_CALL_SUMMARY_END
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
                BlockType::CodeFence => find_matching_fence_end(search_area, start_marker_text),
            };

            // 容错处理：未闭合的块（流式中断）降级为普通 Markdown
            if !is_complete
                && !matches!(
                    block_type,
                    BlockType::HtmlDoc
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
                    if let Some(note) = parse_daily_note_tool_request(inner_content) {
                        content_block_from_daily_note(note)
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
                    content_block_from_daily_note(parse_legacy_daily_note(inner_content))
                }
                BlockType::ToolCallSummary => {
                    let items = parse_tool_call_summary(inner_content);
                    ContentBlock::tool_call_summary(items, inner_content.to_string())
                }
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
                    let mut nodes = vec![crate::vcp_modules::pre_renderer::MarkdownNode::raw_html(
                        open_tag.to_string(),
                    )];
                    nodes.extend(crate::vcp_modules::pre_renderer::parse_markdown_to_ast(
                        &deindented_inner,
                    ));
                    if is_complete {
                        if let (Some(s), Some(e)) = (end_marker_start, end_marker_end) {
                            let close_tag = &search_area[s..e];
                            nodes.push(crate::vcp_modules::pre_renderer::MarkdownNode::raw_html(
                                close_tag.to_string(),
                            ));
                        }
                    }
                    ContentBlock::markdown(None, Some(nodes))
                }
                BlockType::RoleDivider => {
                    let marker_text = &remaining[start_idx..end_idx];
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
            if remaining.contains("[[点击按钮:") {
                blocks.extend(parse_inline_blocks(remaining));
            } else {
                blocks.push(ContentBlock::markdown(
                    None,
                    Some(crate::vcp_modules::pre_renderer::parse_markdown_to_ast(
                        remaining,
                    )),
                ));
            }
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

fn marked_field_mode(marker: &str) -> MarkedFieldMode {
    let normalized = marker.to_ascii_lowercase();
    if normalized.contains("escape") {
        MarkedFieldMode::Escape
    } else if normalized.contains("exp") {
        MarkedFieldMode::Exp
    } else {
        MarkedFieldMode::Normal
    }
}

fn find_marked_field_end(
    source: &str,
    content_start: usize,
    expected_mode: MarkedFieldMode,
) -> Option<(usize, usize)> {
    for end_match in MARKED_FIELD_END.find_iter(&source[content_start..]) {
        let start = content_start + end_match.start();
        let end = content_start + end_match.end();
        let actual_mode = marked_field_mode(&source[start..end]);
        let mode_matches = match expected_mode {
            MarkedFieldMode::Escape => actual_mode == MarkedFieldMode::Escape,
            MarkedFieldMode::Normal | MarkedFieldMode::Exp => {
                matches!(actual_mode, MarkedFieldMode::Normal | MarkedFieldMode::Exp)
            }
        };
        if mode_matches {
            return Some((start, end));
        }
    }
    None
}

fn find_label_colon(source: &str, labels: &[&str], from: usize) -> Option<usize> {
    for (relative_colon, _) in source[from..].match_indices(':') {
        let colon = from + relative_colon;
        for label in labels {
            if colon < label.len() {
                continue;
            }
            let start = colon - label.len();
            if !source.is_char_boundary(start) {
                continue;
            }
            let candidate = &source[start..colon];
            let has_left_boundary = source[..start]
                .chars()
                .next_back()
                .is_none_or(|ch| !ch.is_ascii_alphanumeric() && ch != '_');
            if has_left_boundary && candidate.eq_ignore_ascii_case(label) {
                return Some(colon);
            }
        }
    }
    None
}

pub(crate) fn extract_marked_field(source: &str, labels: &[&str]) -> Option<String> {
    let mut search_from = 0;

    while let Some(colon) = find_label_colon(source, labels, search_from) {
        let mut marker_start = colon + 1;
        while let Some(ch) = source[marker_start..].chars().next() {
            if !ch.is_whitespace() {
                break;
            }
            marker_start += ch.len_utf8();
        }

        if let Some(start_match) = MARKED_FIELD_START.find_at(source, marker_start) {
            if start_match.start() == marker_start {
                let mode = marked_field_mode(start_match.as_str());
                let content_start = start_match.end();
                let content_end = find_marked_field_end(source, content_start, mode)
                    .map(|(start, _)| start)
                    .unwrap_or(source.len());
                return Some(source[content_start..content_end].trim().to_string());
            }
        }

        search_from = colon + 1;
    }

    None
}

/// 在 ToolRequest 正文中寻找真正的外层结束标记。
/// 标记字段内部出现的 END_TOOL_REQUEST 只是正文，必须跳过对应的字段结束标记后再继续扫描。
pub(crate) fn find_tool_request_end(source: &str) -> Option<(usize, usize)> {
    const TOOL_END_MARKER_LEN: usize = "<<<[END_TOOL_REQUEST]>>>".len();
    let mut cursor = 0;

    loop {
        let tool_end = TOOL_END.find_at(source, cursor);
        let field_start = MARKED_FIELD_START.find_at(source, cursor);

        if let Some(start_match) = field_start {
            if tool_end
                .as_ref()
                .is_none_or(|end_match| start_match.start() < end_match.start())
            {
                let mode = marked_field_mode(start_match.as_str());
                let (_, field_end) = find_marked_field_end(source, start_match.end(), mode)?;
                cursor = field_end;
                continue;
            }
        }

        let end_match = tool_end?;
        let marker_start = end_match.end().saturating_sub(TOOL_END_MARKER_LEN);
        let wrapped_before = source[..marker_start].ends_with('`');
        let wrapped_after = source[end_match.end()..].starts_with('`');
        if wrapped_before || wrapped_after {
            cursor = end_match.end();
            continue;
        }
        return Some((end_match.start(), end_match.end()));
    }
}

fn extract_tool_name_value(content: &str) -> Option<String> {
    let mut name = extract_marked_field(content, &["tool_name"]).or_else(|| {
        TOOL_NAME_XML
            .captures(content)
            .and_then(|captures| captures.get(1))
            .map(|matched| matched.as_str().trim().to_string())
    })?;
    if name.ends_with(',') {
        name.pop();
    }
    let name = name.trim().to_string();
    (!name.is_empty()).then_some(name)
}

pub(crate) fn extract_tool_name(content: &str) -> String {
    extract_tool_name_value(content).unwrap_or_else(|| "Processing...".to_string())
}

pub(crate) fn parse_daily_note_tool_request(content: &str) -> Option<ParsedDailyNote> {
    let tool_name = extract_tool_name_value(content)?;
    if !tool_name.eq_ignore_ascii_case("DailyNote") {
        return None;
    }

    let command = extract_marked_field(content, &["command"])
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let daily_content = extract_marked_field(content, &["Content"]).unwrap_or_default();
    let target = extract_marked_field(content, &["target"]).unwrap_or_default();
    let replace = extract_marked_field(content, &["replace"]).unwrap_or_default();

    let is_update = command == "update"
        || (command.is_empty() && !target.trim().is_empty() && !replace.trim().is_empty());
    let is_create = !is_update
        && (command == "create" || (command.is_empty() && !daily_content.trim().is_empty()));

    if !is_update && !is_create {
        return None;
    }

    let maid = extract_marked_field(content, &["maid", "maidName"]).unwrap_or_default();
    let valet = extract_marked_field(content, &["valet", "valetName"]).unwrap_or_default();
    let folder = extract_marked_field(content, &["folder"]).unwrap_or_default();

    if is_update {
        return Some(ParsedDailyNote::Update {
            maid,
            valet,
            folder,
            target,
            replace,
        });
    }

    let date = extract_marked_field(content, &["Date"]).unwrap_or_default();
    let file_name = extract_marked_field(content, &["fileName"]).unwrap_or_default();
    let mut rendered_content = if daily_content.trim().is_empty() {
        "[日记内容解析失败]".to_string()
    } else {
        daily_content
    };
    if let Some(tag) = extract_marked_field(content, &["Tag"]).filter(|tag| !tag.trim().is_empty())
    {
        rendered_content.push_str("\n\nTag:");
        rendered_content.push_str(&tag);
    }

    Some(ParsedDailyNote::Create {
        maid,
        valet,
        date,
        file_name,
        folder,
        content: rendered_content,
    })
}

fn extract_legacy_line_field(content: &str, label: &str) -> String {
    content
        .lines()
        .find_map(|line| {
            line.trim_start()
                .strip_prefix(label)
                .and_then(|rest| rest.strip_prefix(':'))
                .map(|value| value.trim().to_string())
        })
        .unwrap_or_default()
}

pub(crate) fn parse_legacy_daily_note(content: &str) -> ParsedDailyNote {
    let diary_content = content
        .match_indices("Content:")
        .find_map(|(start, marker)| {
            let has_left_boundary = content[..start]
                .chars()
                .next_back()
                .is_none_or(|ch| ch == '\n' || ch == '\r' || ch.is_whitespace());
            has_left_boundary.then(|| content[start + marker.len()..].trim().to_string())
        })
        .unwrap_or_else(|| content.trim().to_string());

    ParsedDailyNote::Create {
        maid: extract_legacy_line_field(content, "Maid"),
        valet: String::new(),
        date: extract_legacy_line_field(content, "Date"),
        file_name: String::new(),
        folder: String::new(),
        content: diary_content,
    }
}

fn content_block_from_daily_note(note: ParsedDailyNote) -> ContentBlock {
    match note {
        ParsedDailyNote::Create {
            maid,
            valet,
            date,
            file_name,
            folder,
            content,
        } => {
            let nodes = crate::vcp_modules::pre_renderer::parse_markdown_to_ast(&content);
            ContentBlock::diary(maid, valet, date, file_name, folder, content, Some(nodes))
        }
        ParsedDailyNote::Update {
            maid,
            valet,
            folder,
            target,
            replace,
        } => {
            let target_nodes = crate::vcp_modules::pre_renderer::parse_markdown_to_ast(&target);
            let replace_nodes = crate::vcp_modules::pre_renderer::parse_markdown_to_ast(&replace);
            ContentBlock::diary_update(
                maid,
                valet,
                folder,
                target,
                replace,
                Some(target_nodes),
                Some(replace_nodes),
            )
        }
    }
}

fn parse_tool_result(content: &str) -> (String, String, Vec<ToolResultDetail>, String) {
    let mut tool_name = "Unknown Tool".to_string();
    let mut status = "Unknown Status".to_string();
    let mut details = Vec::new();
    let mut footer = String::new();

    let mut current_key: Option<String> = None;
    let mut current_value = String::new();

    for line in content.lines() {
        let trimmed = line.trim();
        let captures = if trimmed.starts_with('-') {
            KV_REGEX.captures(trimmed)
        } else {
            None
        };

        if let Some(captures) = captures {
            if let Some(key) = current_key.take() {
                let val = current_value.trim().to_string();
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
                current_value = val_match.as_str().trim().to_string();
            } else {
                current_value = String::new();
            }
        } else if current_key.is_some() {
            if !current_value.is_empty() {
                current_value.push('\n');
            }
            current_value.push_str(line);
        } else if !trimmed.is_empty() {
            if !footer.is_empty() {
                footer.push('\n');
            }
            footer.push_str(line);
        }
    }

    if let Some(key) = current_key {
        let val = current_value.trim().to_string();
        if key == "工具名称" {
            tool_name = val;
        } else if key == "执行状态" {
            status = val;
        } else {
            details.push(ToolResultDetail { key, value: val });
        }
    }

    (tool_name, status, details, footer)
}

pub(crate) fn parse_tool_call_summary(content: &str) -> Vec<ToolCallSummaryItem> {
    let mut items = Vec::new();
    for entry in content.split(['；', ';', '。']) {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }

        let status = if entry.contains("拒绝")
            || entry.contains("被拒")
            || entry.contains("denied")
            || entry.contains("rejected")
            || entry.contains("refused")
        {
            "rejected"
        } else if entry.contains("失败")
            || entry.contains("错误")
            || entry.contains("异常")
            || entry.contains("error")
            || entry.contains("failed")
        {
            "failure"
        } else if entry.contains("超时") || entry.contains("timeout") {
            "timeout"
        } else if entry.contains("成功")
            || entry.contains("完成")
            || entry.contains("success")
            || entry.contains("succeeded")
            || entry.contains("ok")
        {
            "success"
        } else if entry.contains("取消") || entry.contains("中止") || entry.contains("cancel") {
            "cancelled"
        } else if entry.contains("跳过") || entry.contains("skip") {
            "skipped"
        } else {
            "unknown"
        };

        let tool_name = if let Some(idx) = entry.find("调用") {
            entry[..idx].trim().to_string()
        } else {
            entry.to_string()
        };

        items.push(ToolCallSummaryItem {
            tool_name,
            status: status.to_string(),
        });
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool_request(inner: &str) -> String {
        format!("<<<[TOOL_REQUEST]>>>\n{inner}\n<<<[END_TOOL_REQUEST]>>>")
    }

    fn first_non_markdown(blocks: &[ContentBlock]) -> &ContentBlock {
        blocks
            .iter()
            .find(|block| !matches!(block, ContentBlock::Markdown { .. }))
            .expect("expected a semantic content block")
    }

    #[test]
    fn closed_html_fence_becomes_html_preview_without_artifacts() {
        let blocks = parse_content("前文\n\n```html\n<div class=\"card\">hi</div>\n```\n\n后文");
        let preview = blocks
            .iter()
            .find(|b| matches!(b, ContentBlock::HtmlPreview { .. }))
            .expect("html fence should become HtmlPreview");
        let ContentBlock::HtmlPreview {
            content,
            highlighted_content,
            ..
        } = preview
        else {
            unreachable!()
        };
        // pulldown 提取的内容不带正则切片的首尾换行工件
        assert_eq!(content, "<div class=\"card\">hi</div>\n");
        assert!(highlighted_content.is_some());
        // 前后文 markdown 块仍然健在
        assert!(blocks
            .iter()
            .any(|b| matches!(b, ContentBlock::Markdown { .. })));
    }

    #[test]
    fn test_parse_content_style_blocks() {
        // 1. 正常的独立行 <style> 应该被正确解析为 Style 块
        let raw_style = "<style>\nbody { color: red; }\n</style>";
        let blocks = parse_content(raw_style);
        assert_eq!(blocks.len(), 1);
        match &blocks[0] {
            ContentBlock::Style { content, .. } => {
                assert_eq!(content.trim(), "body { color: red; }");
            }
            _ => panic!("Expected Style block, got {:?}", blocks[0]),
        }

        // 2. 行内代码包裹的 `<style>` 应该被保留在 Markdown 中，而不是被提取为 Style 块
        let raw_inline = "在 HTML 中，`<style>body {}</style>` 用于定义样式。";
        let blocks = parse_content(raw_inline);
        assert_eq!(blocks.len(), 1);
        match &blocks[0] {
            ContentBlock::Markdown { .. } => {}
            _ => panic!("Expected Markdown block, got {:?}", blocks[0]),
        }
    }

    #[test]
    fn unclosed_thought_markers_remain_markdown_in_the_durable_render() {
        for raw in [
            "<think>unfinished **reasoning**",
            "[--- VCP元思考链: 规划 ---]\nunfinished **reasoning**",
        ] {
            let blocks = parse_content(raw);
            assert!(!blocks.is_empty());
            assert!(blocks
                .iter()
                .all(|block| matches!(block, ContentBlock::Markdown { .. })));
            let serialized = serde_json::to_string(&blocks).expect("serialize markdown fallback");
            assert!(serialized.contains("unfinished"));
        }
    }

    #[test]
    fn test_pre_txt_16_parsing() {
        let text = "### 16. 代码块内包含围栏\n\n````markdown\n```python\n# This is code inside markdown inside code\nprint(\"nested\")\n```\n````";
        let blocks = parse_content(text);
        println!("BLOCKS: {:#?}", blocks);
        assert_eq!(blocks.len(), 2);

        // 第一个块应该是 Heading
        if let ContentBlock::Markdown { nodes, .. } = &blocks[0] {
            let nodes = nodes.as_ref().unwrap();
            assert_eq!(nodes.len(), 1);
            assert!(matches!(
                nodes[0],
                crate::vcp_modules::pre_renderer::MarkdownNode::Heading { .. }
            ));
        } else {
            panic!("Expected Heading block");
        }

        // 第二个块应该是 CodeBlock
        if let ContentBlock::Markdown { nodes, .. } = &blocks[1] {
            let nodes = nodes.as_ref().unwrap();
            assert_eq!(nodes.len(), 1);
            let has_nested_code = nodes.iter().any(|node| {
                if let crate::vcp_modules::pre_renderer::MarkdownNode::CodeBlock {
                    lang,
                    code,
                    ..
                } = node
                {
                    lang.as_deref() == Some("markdown") && code.contains("```python")
                } else {
                    false
                }
            });
            assert!(
                has_nested_code,
                "Expected to find a nested CodeBlock with lang=markdown and containing inner code"
            );
        } else {
            panic!(
                "Expected Markdown block with nested code, got {:?}",
                blocks[1]
            );
        }
    }

    #[test]
    fn parses_full_daily_note_create_with_new_fields_and_tag() {
        let raw = tool_request(
            "tool_name:「始」DailyNote「末」\n\
             command:「始」create「末」\n\
             maidName:「始」Sakura「末」\n\
             valetName:「始」Sebastian「末」\n\
             Date:「始」2026-08-10「末」\n\
             fileName:「始」Field Log「末」\n\
             folder:「始」missions/day-1「末」\n\
             Content:「始」## Done\n\n- item「末」\n\
             Tag:「始」mobile, sync「末」",
        );

        let blocks = parse_content(&raw);
        match first_non_markdown(&blocks) {
            ContentBlock::Diary {
                maid,
                valet,
                date,
                file_name,
                folder,
                content,
                nodes,
                ..
            } => {
                assert_eq!(maid, "Sakura");
                assert_eq!(valet, "Sebastian");
                assert_eq!(date, "2026-08-10");
                assert_eq!(file_name, "Field Log");
                assert_eq!(folder, "missions/day-1");
                assert_eq!(content, "## Done\n\n- item\n\nTag:mobile, sync");
                assert!(nodes.as_ref().is_some_and(|nodes| !nodes.is_empty()));
            }
            block => panic!("expected diary, got {block:?}"),
        }
    }

    #[test]
    fn recognizes_commandless_create_and_update_with_update_precedence() {
        let create =
            tool_request("tool_name:{始}dAiLyNoTe{末}\nContent:{始}commandless create{末}");
        assert!(matches!(
            first_non_markdown(&parse_content(&create)),
            ContentBlock::Diary { content, .. } if content == "commandless create"
        ));

        let update = tool_request(
            "tool_name:「始」DailyNote「末」\n\
             target:「始」old「末」\n\
             replace:「始」new「末」\n\
             Content:「始」must not win「末」",
        );
        assert!(matches!(
            first_non_markdown(&parse_content(&update)),
            ContentBlock::DiaryUpdate { target, replace, .. }
                if target == "old" && replace == "new"
        ));
    }

    #[test]
    fn explicit_daily_note_commands_keep_failure_placeholders() {
        let create = tool_request("tool_name:「始」DailyNote「末」\ncommand:「始」create「末」");
        assert!(matches!(
            first_non_markdown(&parse_content(&create)),
            ContentBlock::Diary { content, .. } if content == "[日记内容解析失败]"
        ));

        let update = tool_request(
            "tool_name:「始」DailyNote「末」\n\
             command:「始」update「末」\n\
             target:「始」old「末」",
        );
        assert!(matches!(
            first_non_markdown(&parse_content(&update)),
            ContentBlock::DiaryUpdate { target, replace, .. }
                if target == "old" && replace.is_empty()
        ));
    }

    #[test]
    fn escape_field_can_contain_normal_end_and_fake_tool_end() {
        let raw = tool_request(
            "TOOL_NAME:{始}DailyNote{末}\n\
             COMMAND:{始}create{末}\n\
             Maid:{始EXP}Sakura{末}\n\
             Content:{始EsCaPe}first {末}\n\
             <<<[END_TOOL_REQUEST]>>>\n\
             still content{末EsCaPe}",
        );

        match first_non_markdown(&parse_content(&raw)) {
            ContentBlock::Diary { maid, content, .. } => {
                assert_eq!(maid, "Sakura");
                assert!(content.contains("first {末}"));
                assert!(content.contains("<<<[END_TOOL_REQUEST]>>>"));
                assert!(content.ends_with("still content"));
            }
            block => panic!("expected diary, got {block:?}"),
        }
    }

    #[test]
    fn unknown_or_non_daily_note_commands_remain_tool_use() {
        let unknown = tool_request(
            "tool_name:「始」DailyNote「末」\n\
             command:「始」delete「末」\n\
             Content:「始」do not specialize「末」",
        );
        assert!(matches!(
            first_non_markdown(&parse_content(&unknown)),
            ContentBlock::ToolUse { tool_name, .. } if tool_name == "DailyNote"
        ));

        let other = tool_request(
            "tool_name:「始」OtherTool「末」\n\
             command:「始」create「末」\n\
             Content:「始」mentions DailyNote create「末」",
        );
        assert!(matches!(
            first_non_markdown(&parse_content(&other)),
            ContentBlock::ToolUse { tool_name, .. } if tool_name == "OtherTool"
        ));
    }

    #[test]
    fn legacy_daily_note_uses_full_body_when_content_label_is_missing() {
        let raw =
            "<<<DailyNoteStart>>>\nMaid: Sakura\nDate: 2026-08-10\nlegacy body\n<<<DailyNoteEnd>>>";
        match first_non_markdown(&parse_content(raw)) {
            ContentBlock::Diary {
                maid,
                date,
                content,
                valet,
                ..
            } => {
                assert_eq!(maid, "Sakura");
                assert_eq!(date, "2026-08-10");
                assert!(content.contains("Maid: Sakura"));
                assert!(content.contains("legacy body"));
                assert!(valet.is_empty());
            }
            block => panic!("expected legacy diary, got {block:?}"),
        }
    }

    #[test]
    fn unclosed_escape_request_does_not_become_diary() {
        let raw = "<<<[TOOL_REQUEST]>>>\n\
                   tool_name:「始」DailyNote「末」\n\
                   command:「始」create「末」\n\
                   Content:{始ESCAPE}body\n\
                   <<<[END_TOOL_REQUEST]>>>";
        let blocks = parse_content(raw);
        assert!(!blocks.iter().any(|block| matches!(
            block,
            ContentBlock::Diary { .. } | ContentBlock::DiaryUpdate { .. }
        )));
    }

    #[test]
    fn old_diary_cache_deserializes_with_defaulted_new_fields() {
        let cached = r#"{
            "type":"diary",
            "maid":"Sakura",
            "date":"2026-08-10",
            "content":"legacy cache"
        }"#;
        let block: ContentBlock = serde_json::from_str(cached).expect("old diary cache must load");
        assert!(matches!(
            block,
            ContentBlock::Diary { valet, file_name, folder, .. }
                if valet.is_empty() && file_name.is_empty() && folder.is_empty()
        ));
    }

    #[test]
    fn fence_end_scanner_matches_commonmark_closing_rules() {
        // 基本闭合：返回结束围栏行的 [start, end)，end 不含换行符
        assert_eq!(
            find_matching_fence_end("let x = 1;\n```\n", "```rust"),
            (Some(11), Some(14), true)
        );
        // 结束围栏反引号数必须不少于开围栏（更多可以闭合）
        assert_eq!(
            find_matching_fence_end("```\nx\n````\n", "````rust"),
            (Some(6), Some(10), true)
        );
        assert_eq!(
            find_matching_fence_end("```\n", "````rust"),
            (None, None, false),
            "反引号不足不得闭合"
        );
        // 行首至多 3 个空白，4 个缩进不闭合；尾部空白与 CRLF 合法
        assert_eq!(
            find_matching_fence_end("  ```  \nrest", "```rust"),
            (Some(0), Some(7), true)
        );
        assert_eq!(
            find_matching_fence_end("    ```\n", "```rust"),
            (None, None, false)
        );
        assert_eq!(
            find_matching_fence_end("```\r\n", "```rust"),
            (Some(0), Some(4), true)
        );
        // 行内反引号、非反引号起始标记、反引号不足的开围栏都不是合法配对
        assert_eq!(
            find_matching_fence_end("let ``` x\n```\n", "```rust"),
            (Some(10), Some(13), true)
        );
        assert_eq!(find_matching_fence_end("```\n", "``"), (None, None, false));
        assert_eq!(find_matching_fence_end("```\n", "~~~"), (None, None, false));
    }
}

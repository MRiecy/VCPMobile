use serde::{Deserialize, Serialize};

use crate::vcp_modules::content_parser::{
    BlockType, ToolResultDetail, BUTTON_CLICK, /* extraction helpers */
    CONTENT_REGEX, DATE_REGEX, DIARY_END, DIARY_START, GENERIC_CODE_FENCE_END,
    GENERIC_CODE_FENCE_START, HTML_DOC_END, HTML_DOC_START, HTML_FENCE_START, KV_REGEX, MAID_REGEX,
    ROLE_DIVIDER, STYLE_TAG_END, STYLE_TAG_START, THINK_END, THINK_START, THOUGHT_END,
    THOUGHT_START, TOOL_END, TOOL_NAME, TOOL_RESULT_END, TOOL_RESULT_START, TOOL_START,
};
use crate::vcp_modules::pre_renderer::MarkdownNode;
use crate::vcp_modules::sync_hash::HashAggregator;

/// 流式模式下轻量解析的块类型，前端增量渲染
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum StreamBlock {
    #[serde(rename = "markdown")]
    Markdown {
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        nodes: Option<Vec<MarkdownNode>>,
        hash: String,
    },
    #[serde(rename = "thought")]
    Thought {
        theme: String,
        content: String,
        is_complete: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        nodes: Option<Vec<MarkdownNode>>,
        hash: String,
    },
    #[serde(rename = "tool-use")]
    Tool {
        tool_name: String,
        content: String,
        hash: String,
    },
    #[serde(rename = "tool-result")]
    ToolResult {
        tool_name: String,
        status: String,
        details: Vec<ToolResultDetail>,
        footer: String,
        hash: String,
    },
    #[serde(rename = "diary")]
    Diary {
        maid: String,
        date: String,
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        nodes: Option<Vec<MarkdownNode>>,
        hash: String,
    },
    #[serde(rename = "html-preview")]
    HtmlPreview {
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        highlighted_content: Option<String>,
        hash: String,
    },
    #[serde(rename = "role-divider")]
    RoleDivider {
        role: String,
        is_end: bool,
        hash: String,
    },
    #[serde(rename = "style")]
    Style { content: String, hash: String },
    #[serde(rename = "button-click")]
    ButtonClick { content: String, hash: String },
}

impl StreamBlock {
    pub fn markdown(content: String, nodes: Option<Vec<MarkdownNode>>, hash: String) -> Self {
        Self::Markdown {
            content,
            nodes,
            hash,
        }
    }

    pub fn thought(
        theme: String,
        content: String,
        is_complete: bool,
        nodes: Option<Vec<MarkdownNode>>,
        hash: String,
    ) -> Self {
        Self::Thought {
            theme,
            content,
            is_complete,
            nodes,
            hash,
        }
    }

    pub fn tool(tool_name: String, content: String, hash: String) -> Self {
        Self::Tool {
            tool_name,
            content,
            hash,
        }
    }

    pub fn tool_result(
        tool_name: String,
        status: String,
        details: Vec<ToolResultDetail>,
        footer: String,
        hash: String,
    ) -> Self {
        Self::ToolResult {
            tool_name,
            status,
            details,
            footer,
            hash,
        }
    }

    pub fn diary(
        maid: String,
        date: String,
        content: String,
        nodes: Option<Vec<MarkdownNode>>,
        hash: String,
    ) -> Self {
        Self::Diary {
            maid,
            date,
            content,
            nodes,
            hash,
        }
    }

    pub fn html_preview(content: String, hash: String) -> Self {
        // 流式打字期间完全不调用 syntect 高亮，彻底避免高频流更新对后端 CPU 能耗的无谓消耗
        let highlighted_content = None;
        Self::HtmlPreview {
            content,
            highlighted_content,
            hash,
        }
    }

    pub fn role_divider(role: String, is_end: bool, hash: String) -> Self {
        Self::RoleDivider { role, is_end, hash }
    }

    pub fn style(content: String, hash: String) -> Self {
        Self::Style { content, hash }
    }

    #[allow(dead_code)]
    pub fn button_click(content: String, hash: String) -> Self {
        Self::ButtonClick { content, hash }
    }
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
                #[cfg(test)]
                {
                    let snippet: String = remaining[start..].chars().take(50).collect();
                    println!(
                        "[DIAG] Found marker at pos + {}: {:?}, text snippet: {:?}",
                        pos + start,
                        block_type,
                        snippet
                    );
                }

                // 2. 标记之前的文本 → Markdown 段落
                if start > 0 {
                    let before = &remaining[..start];
                    let (md_blocks, md_tail) = split_markdown_paragraphs(before);
                    blocks.extend(md_blocks);
                    if !md_tail.is_empty() {
                        #[cfg(test)]
                        println!("[DIAG] Precipitating preceding md_tail: {:?}", md_tail);
                        // 因为后面已经紧跟了特种块，说明 before 物理上已全部输出完毕。
                        // 强制将 md_tail 沉淀为 stable 块，绝不阻碍后续特种块的闭合解析！
                        let nodes =
                            crate::vcp_modules::pre_renderer::parse_markdown_to_ast(&md_tail);
                        let hash = HashAggregator::compute_content_hash(&md_tail);
                        blocks.push(StreamBlock::markdown(md_tail, Some(nodes), hash));
                    }
                }

                // 3. 寻找对应结束标记
                let content_start = end;
                let search_area = &remaining[content_start..];

                if let Some((end_start, end_end)) =
                    find_end_marker(remaining, start, end, &block_type)
                {
                    #[cfg(test)]
                    println!("[DIAG] Found end marker for {:?} at pos + {}: relative start: {}, relative end: {}", block_type, pos + content_start, end_start, end_end);
                    let inner_content = &search_area[..end_start];
                    let block = build_stream_block(
                        &block_type,
                        inner_content,
                        remaining,
                        start,
                        end,
                        end_end,
                    );
                    blocks.push(block);
                    pos += content_start + end_end;
                } else {
                    #[cfg(test)]
                    println!("[DIAG] FAILED to find end marker for {:?}. Returning remainder from start as tail.", block_type);
                    // 找不到结束标记 → 之前已强制沉淀 md_tail（即 remaining[..start]），
                    // 故此帧游标推进 start 字节，将未闭合块起始作为 tail 返回，消灭重复渲染
                    self.processed_len = pos + start;
                    return (blocks, remaining[start..].to_string());
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
            let nodes = crate::vcp_modules::pre_renderer::parse_markdown_to_ast(trimmed);
            let hash = HashAggregator::compute_content_hash(trimmed);
            blocks.push(StreamBlock::markdown(
                trimmed.to_string(),
                Some(nodes),
                hash,
            ));
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
    let checks: [(&regex::Regex, BlockType); 11] = [
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
        (
            &crate::vcp_modules::content_parser::HTML_CONTAINER_OPEN_RE,
            BlockType::HtmlContainer,
        ),
    ];

    let mut earliest: Option<(usize, usize, BlockType)> = None;
    for (re, bt) in checks {
        if let Some(m) = re.find(text) {
            if earliest.as_ref().is_none_or(|(s, _, _)| m.start() < *s) {
                earliest = Some((m.start(), m.end(), bt));
            }
        }
    }
    earliest
}

/// 寻找对应块的结束标记
/// 返回 (end_start_offset, end_end_offset) 在 remaining[content_start..] 内的相对偏移量
fn find_end_marker(
    remaining: &str,
    start: usize,
    end: usize,
    block_type: &BlockType,
) -> Option<(usize, usize)> {
    let content_start = end;
    let search_area = &remaining[content_start..];

    #[cfg(test)]
    {
        if *block_type == BlockType::HtmlFence || *block_type == BlockType::CodeFence {
            let snippet: String = search_area.chars().take(100).collect();
            println!(
                "[DIAG_END] find_end_marker for {:?}: search_area len: {}, starts with: {:?}",
                block_type,
                search_area.len(),
                snippet
            );
            let m_direct = GENERIC_CODE_FENCE_END.find(search_area);
            println!(
                "[DIAG_END] Direct regex match in find_end_marker: {:?}",
                m_direct
            );
        }
    }

    if let BlockType::HtmlContainer = block_type {
        let marker_text = &remaining[start..end];
        if let Some(caps) =
            crate::vcp_modules::content_parser::HTML_CONTAINER_OPEN_RE.captures(marker_text)
        {
            let tag_name = caps.get(1).unwrap().as_str().to_lowercase();
            return crate::vcp_modules::chat::pre_renderer::markdown_parser::find_matching_close_tag(remaining, content_start, &tag_name)
                .map(|(s, e)| (s - content_start, e - content_start));
        }
        return None;
    }

    let m = match block_type {
        BlockType::Tool => TOOL_END.find(search_area),
        BlockType::Thought => THOUGHT_END.find(search_area),
        BlockType::Think => THINK_END.find(search_area),
        BlockType::ToolResult => TOOL_RESULT_END.find(search_area),
        BlockType::Diary => DIARY_END.find(search_area),
        BlockType::HtmlFence | BlockType::CodeFence => GENERIC_CODE_FENCE_END.find(search_area),
        BlockType::HtmlDoc => HTML_DOC_END.find(search_area),
        BlockType::HtmlContainer => unreachable!(),
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
    end_end: usize,
) -> StreamBlock {
    match block_type {
        BlockType::Tool => {
            let tool_name = extract_tool_name(inner_content);
            if is_daily_note_create(inner_content) {
                let (maid, date, content) = extract_diary_details(inner_content);
                let nodes = crate::vcp_modules::chat::pre_renderer::parse_markdown_to_ast(&content);
                let hash =
                    HashAggregator::compute_content_hash(&format!("{}:{}:{}", maid, date, content));
                StreamBlock::diary(maid, date, content, Some(nodes), hash)
            } else {
                let hash = HashAggregator::compute_content_hash(&format!(
                    "{}:{}",
                    tool_name, inner_content
                ));
                StreamBlock::tool(tool_name, inner_content.to_string(), hash)
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
                crate::vcp_modules::chat::pre_renderer::parse_markdown_to_ast(inner_content);
            let hash =
                HashAggregator::compute_content_hash(&format!("{}:{}", theme, inner_content));
            StreamBlock::thought(theme, inner_content.to_string(), true, Some(nodes), hash)
        }
        BlockType::Think => {
            let nodes =
                crate::vcp_modules::chat::pre_renderer::parse_markdown_to_ast(inner_content);
            let hash = HashAggregator::compute_content_hash(inner_content);
            StreamBlock::thought(
                "思维链".to_string(),
                inner_content.to_string(),
                true,
                Some(nodes),
                hash,
            )
        }
        BlockType::ToolResult => {
            let (tool_name, status, details, footer) = parse_tool_result(inner_content);
            let mut details_str = String::new();
            for d in &details {
                details_str.push_str(&d.key);
                details_str.push_str(&d.value);
            }
            let hash = HashAggregator::compute_content_hash(&format!(
                "{}:{}:{}:{}",
                tool_name, status, details_str, footer
            ));
            StreamBlock::tool_result(tool_name, status, details, footer, hash)
        }
        BlockType::Diary => {
            let (maid, date, content) = extract_diary_details(inner_content);
            let nodes = crate::vcp_modules::chat::pre_renderer::parse_markdown_to_ast(&content);
            let hash =
                HashAggregator::compute_content_hash(&format!("{}:{}:{}", maid, date, content));
            StreamBlock::diary(maid, date, content, Some(nodes), hash)
        }
        BlockType::HtmlFence | BlockType::HtmlDoc => {
            let hash = HashAggregator::compute_content_hash(inner_content);
            StreamBlock::html_preview(inner_content.to_string(), hash)
        }
        BlockType::HtmlContainer => {
            let open_tag = &remaining[start_idx..end_idx];
            let deindented_inner =
                crate::vcp_modules::chat::pre_renderer::markdown_parser::trim_common_leading_indent(
                    inner_content,
                );
            let mut nodes = vec![crate::vcp_modules::pre_renderer::MarkdownNode::raw_html(
                open_tag.to_string(),
            )];
            nodes.extend(
                crate::vcp_modules::chat::pre_renderer::parse_markdown_to_ast(&deindented_inner),
            );

            let mut full_html = format!("{}{}", open_tag, inner_content);
            if end_end > 0 {
                let search_area = &remaining[end_idx..];
                let end_start = inner_content.len();
                if end_start < end_end && end_end <= search_area.len() {
                    let close_tag = &search_area[end_start..end_end];
                    nodes.push(crate::vcp_modules::pre_renderer::MarkdownNode::raw_html(
                        close_tag.to_string(),
                    ));
                    full_html.push_str(close_tag);
                }
            }

            let hash = HashAggregator::compute_content_hash(&full_html);
            StreamBlock::markdown(full_html, Some(nodes), hash)
        }
        BlockType::CodeFence => {
            let fence = &remaining[start_idx..end_idx];
            let full_text = format!("{}\n{}\n```", fence, inner_content);
            let nodes = crate::vcp_modules::chat::pre_renderer::parse_markdown_to_ast(&full_text);
            let hash = HashAggregator::compute_content_hash(&full_text);
            StreamBlock::markdown(full_text, Some(nodes), hash)
        }
        BlockType::RoleDivider => {
            let marker_text = &remaining[start_idx..end_idx];
            if let Some(caps) = ROLE_DIVIDER.captures(marker_text) {
                let is_end = caps.get(1).is_some();
                let role = caps
                    .get(2)
                    .map(|m| m.as_str().to_lowercase())
                    .unwrap_or_default();
                let hash = HashAggregator::compute_content_hash(&format!("{}:{}", role, is_end));
                StreamBlock::role_divider(role, is_end, hash)
            } else {
                let hash = HashAggregator::compute_content_hash("unknown:false");
                StreamBlock::role_divider("unknown".to_string(), false, hash)
            }
        }
        BlockType::Style => {
            let hash = HashAggregator::compute_content_hash(inner_content);
            StreamBlock::style(inner_content.to_string(), hash)
        }
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
            let nodes = crate::vcp_modules::pre_renderer::parse_markdown_to_ast(trimmed);
            let hash = HashAggregator::compute_content_hash(trimmed);
            blocks.push(StreamBlock::markdown(
                trimmed.to_string(),
                Some(nodes),
                hash,
            ));
        }

        // 针对留下的 tail，进行“句级”自适应切分兜底
        let (mut extra_blocks, final_tail) = check_sentence_precipitation(tail);
        blocks.append(&mut extra_blocks);

        // 检查 inline button clicks
        let blocks = extract_inline_buttons(blocks);
        (blocks, final_tail)
    } else {
        // 全程没有 \n\n，进行句级自适应切分兜底
        let (blocks, final_tail) = check_sentence_precipitation(text);
        let blocks = extract_inline_buttons(blocks);
        (blocks, final_tail)
    }
}

/// 辅助函数：当未分段文本超过阈值时，利用句尾标点强行截断沉淀
fn check_sentence_precipitation(text: &str) -> (Vec<StreamBlock>, String) {
    const PRECIPITATE_THRESHOLD: usize = 500; // 500字阻尼线

    if text.len() < PRECIPITATE_THRESHOLD {
        return (Vec::new(), text.to_string());
    }

    // 寻找距离 500 字最近的一个句尾标点（。 ！ ？ . ! ?）
    let punctuations = ['。', '！', '？', '…', '.', '!', '?'];
    let mut cut_index = None;

    for (i, ch) in text.char_indices().rev() {
        // 保留至少 200 个字节（大约几十个汉字）作为 tail，保证打字机打出的文字有连续视效
        if i < 200 {
            break;
        }
        if punctuations.contains(&ch) {
            cut_index = Some(i + ch.len_utf8());
            break;
        }
    }

    if let Some(idx) = cut_index {
        let stable_part = &text[..idx];
        let tail_part = &text[idx..];

        let trimmed = stable_part.trim();
        if !trimmed.is_empty() {
            let nodes = crate::vcp_modules::pre_renderer::parse_markdown_to_ast(trimmed);
            let hash = HashAggregator::compute_content_hash(trimmed);
            let block = StreamBlock::markdown(trimmed.to_string(), Some(nodes), hash);
            return (vec![block], tail_part.to_string());
        }
    }

    (Vec::new(), text.to_string())
}

/// 从 Markdown 块中提取内联按钮点击
fn extract_inline_buttons(mut blocks: Vec<StreamBlock>) -> Vec<StreamBlock> {
    let mut result = Vec::new();

    for block in blocks.drain(..) {
        match block {
            StreamBlock::Markdown { content, nodes, .. } => {
                let mut last_end = 0;
                let mut has_button = false;

                for cap in BUTTON_CLICK.captures_iter(&content) {
                    has_button = true;
                    let Some(m) = cap.get(0) else { continue };
                    let Some(btn_content) = cap.get(1) else {
                        continue;
                    };

                    // 按钮前的文本作为 Markdown 块
                    if m.start() > last_end {
                        let before = content[last_end..m.start()].trim().to_string();
                        if !before.is_empty() {
                            let before_nodes =
                                crate::vcp_modules::pre_renderer::parse_markdown_to_ast(&before);
                            let hash = HashAggregator::compute_content_hash(&before);
                            result.push(StreamBlock::markdown(before, Some(before_nodes), hash));
                        }
                    }

                    let btn_text = btn_content.as_str().trim().to_string();
                    let hash = HashAggregator::compute_content_hash(&btn_text);
                    result.push(StreamBlock::button_click(btn_text, hash));
                    last_end = m.end();
                }

                if has_button {
                    // 最后一个按钮后的文本
                    if last_end < content.len() {
                        let after = content[last_end..].trim().to_string();
                        if !after.is_empty() {
                            let after_nodes =
                                crate::vcp_modules::pre_renderer::parse_markdown_to_ast(&after);
                            let hash = HashAggregator::compute_content_hash(&after);
                            result.push(StreamBlock::markdown(after, Some(after_nodes), hash));
                        }
                    }
                } else {
                    let hash = HashAggregator::compute_content_hash(&content);
                    result.push(StreamBlock::markdown(content, nodes, hash));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_code_block_precipitation_failure() {
        let mut text = r#"### 代码块沉淀回归

下面是一段 HTML fence，曾经触发尾部沉淀失败:

```html
<section class="demo">
  <h1>Hello</h1>
  <p>inside fenced html</p>
</section>
```

闭合代码块之后继续输出 Markdown。
"#
        .to_string();

        // 兼容处理：如果是转义过的 JSON Payload，使用 serde_json 进行 unescape
        if text.contains("\\n") || text.contains("\\\"") {
            let wrapped = format!("\"{}\"", text);
            if let Ok(unescaped) = serde_json::from_str::<String>(&wrapped) {
                text = unescaped;
            } else {
                // 如果直接 wrapped 失败，可能是有未转义的真实双引号，尝试直接替换
                text = text.replace("\\n", "\n").replace("\\\"", "\"");
            }
        }

        println!("====== REGEX DIRECT MATCH DIAGNOSIS (ORIGINAL UNESCAPED TEXT) ======");
        let fence_start_re = &crate::vcp_modules::content_parser::HTML_FENCE_START;
        let fence_end_re = &crate::vcp_modules::content_parser::GENERIC_CODE_FENCE_END;

        if let Some(m_start) = fence_start_re.find(&text) {
            println!(
                "HTML_FENCE_START matched at: {}..{}",
                m_start.start(),
                m_start.end()
            );
            let search_area = &text[m_start.end()..];
            println!(
                "Search area first 200 chars: {:?}",
                &search_area[..200.min(search_area.len())]
            );

            if let Some(m_end) = fence_end_re.find(search_area) {
                println!(
                    "GENERIC_CODE_FENCE_END matched in search_area at relative: {}..{}",
                    m_end.start(),
                    m_end.end()
                );
            } else {
                println!("GENERIC_CODE_FENCE_END FAILED TO MATCH search_area!");
            }
        } else {
            println!("HTML_FENCE_START failed to match!");
        }

        // 构造包含 HtmlContainer 的测试文本
        let html_container_text =
            "\n<div class=\"chat-container\">\n<p>Hello inside container</p>\n</div>\n";
        let combined_text = format!("{}{}", text, html_container_text);

        let mut parser = StreamBlockParser::new();
        let blocks = parser.finalize(&combined_text);
        println!("====== RUST UNIT TEST PRECIPITATION DIAGNOSIS ======");
        println!("Blocks count: {}", blocks.len());

        assert!(
            !blocks.is_empty(),
            "Parser should successfully yield stable blocks"
        );
    }

    #[test]
    fn test_streaming_typewriter_incremental_precipitation() {
        let mut parser = StreamBlockParser::new();

        // 模拟第 1 帧：输出到代码块开头，未闭合
        let frame_1 = "### 维度二：代码高亮\n\n测试流式传输未闭合时：\n\n```rust";
        let (blocks_1, tail_1) = parser.process(frame_1);
        println!("Frame 1 - Blocks: {}, Tail: {:?}", blocks_1.len(), tail_1);
        // 应该成功沉淀出前面的两个 Markdown 块（因 \n\n 物理分段），且 tail 只包含 ```rust
        assert_eq!(blocks_1.len(), 2);
        assert_eq!(tail_1, "```rust");

        // 模拟第 2 帧：代码块流式增量增长，仍未闭合
        let frame_2 = "### 维度二：代码高亮\n\n测试流式传输未闭合时：\n\n```rust\nuse tokio;\n";
        let (blocks_2, tail_2) = parser.process(frame_2);
        println!("Frame 2 - Blocks: {}, Tail: {:?}", blocks_2.len(), tail_2);
        // 应该没有任何新的 blocks（因为前段已经沉淀，后段未闭合），且 tail 应该是增量代码块且去掉了前段
        assert_eq!(blocks_2.len(), 0);
        assert_eq!(tail_2, "```rust\nuse tokio;\n");

        // 模拟第 3 帧：流式代码块闭合
        let frame_3 = "### 维度二：代码高亮\n\n测试流式传输未闭合时：\n\n```rust\nuse tokio;\n```";
        let (blocks_3, tail_3) = parser.process(frame_3);
        println!("Frame 3 - Blocks: {}, Tail: {:?}", blocks_3.len(), tail_3);
        // 应该成功闭合代码块并将其沉淀，且 tail 为空
        assert_eq!(blocks_3.len(), 1);
        assert!(tail_3.is_empty());
    }
}

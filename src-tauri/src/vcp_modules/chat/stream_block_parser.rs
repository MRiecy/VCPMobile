use serde::{Deserialize, Serialize};

use crate::vcp_modules::content_parser::{
    extract_tool_name, find_tool_request_end, parse_daily_note_tool_request,
    parse_legacy_daily_note, BlockType, ParsedDailyNote, ToolCallSummaryItem, ToolResultDetail,
    BUTTON_CLICK, DIARY_END, DIARY_START, GENERIC_CODE_FENCE_START, HTML_DOC_END, HTML_DOC_START,
    KV_REGEX, ROLE_DIVIDER, STYLE_TAG_END, STYLE_TAG_START, THINK_END, THINK_START, THOUGHT_END,
    THOUGHT_START, TOOL_RESULT_END, TOOL_RESULT_START, TOOL_START,
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
        hash: String,
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
    #[serde(rename = "tool-call-summary")]
    ToolCallSummary {
        items: Vec<ToolCallSummaryItem>,
        raw_content: String,
        hash: String,
    },
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

    pub fn html_preview(content: String, hash: String) -> Self {
        // 未闭合围栏由 Aurora tail 按行增量高亮；闭合后组件类型切换为 HtmlPreview，
        // 此处只做一次终态 classed 高亮（HTML 外壳），避免交接后退回无样式代码。
        let highlighted_content =
            crate::vcp_modules::chat::pre_renderer::code_highlighter::highlight_code_block(
                &content,
                "html",
                crate::vcp_modules::chat::pre_renderer::code_highlighter::CodeBlockShell::Html,
            );
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

    pub fn tool_call_summary(
        items: Vec<ToolCallSummaryItem>,
        raw_content: String,
        hash: String,
    ) -> Self {
        Self::ToolCallSummary {
            items,
            raw_content,
            hash,
        }
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
    /// 上一帧遗留的未闭合特种块书签：流式期间未闭合块（典型：增长中的代码围栏）会让
    /// `processed_len` 停在块首，下一帧起始标记的类型与长度都已知，无需再用 11 条
    /// 起始正则全扫 tail，直接续跑该块的结束匹配即可。
    pending_block: Option<PendingBlock>,
}

#[derive(Clone, Copy)]
struct PendingBlock {
    block_type: BlockType,
    /// 块起始标记在累积全文中的字节偏移（== 当时的 processed_len）。
    start: usize,
    /// 起始标记的字节长度（remaining 视图下恒为 end - start，其中 start 为 0）。
    marker_len: usize,
}

/// 未闭合思维链 tail 的可撤销显示投影。
///
/// `content` 只用于活跃流中的 UI；只要尚未闭合且流仍在继续，就始终保持 Thought tail。
/// 解析器另外保留包含起始标记的原始 tail，仅供流终止后的严格 finalize 在仍未闭合时
/// 按完整原文降级为 Markdown。
pub(crate) struct SpeculativeThoughtTail<'a> {
    pub content: &'a str,
    pub content_offset: usize,
    pub theme: String,
}

pub(crate) fn speculative_thought_tail(tail: &str) -> Option<SpeculativeThoughtTail<'_>> {
    if let Some(captures) = THOUGHT_START.captures(tail) {
        let marker = captures.get(0)?;
        if marker.start() == 0 {
            let theme = captures
                .get(1)
                .map(|matched| matched.as_str().trim().replace('"', ""))
                .unwrap_or_else(|| "元思考链".to_string());
            return Some(SpeculativeThoughtTail {
                content: &tail[marker.end()..],
                content_offset: marker.end(),
                theme,
            });
        }
    }

    let marker = THINK_START.find(tail)?;
    (marker.start() == 0).then(|| SpeculativeThoughtTail {
        content: &tail[marker.end()..],
        content_offset: marker.end(),
        theme: "思维链".to_string(),
    })
}

impl StreamBlockParser {
    pub fn new() -> Self {
        Self {
            processed_len: 0,
            pending_block: None,
        }
    }

    /// 当前未闭合 tail 在累积全文中的起始字节位置。
    /// 起点不变即可证明新 tail 只是同一全文后缀的追加，无需再次扫描旧前缀。
    pub(crate) fn tail_start(&self) -> usize {
        self.processed_len
    }

    /// 处理累积的全文，返回 (已完成的块列表, 尾部纯文本, 未闭合特种块类型)。
    /// 已闭合的块从 tail 中移除加入 stable blocks；普通 Markdown tail 的类型为 None。
    pub fn process(&mut self, full_text: &str) -> (Vec<StreamBlock>, String, Option<BlockType>) {
        let mut blocks = Vec::new();
        let mut pos = self.processed_len.min(full_text.len());
        // 取出上一帧的未闭合块书签；本帧各返回路径负责重建（未闭合）或留空（已闭合/普通 tail）
        let mut pending = self.pending_block.take();

        while pos < full_text.len() {
            let remaining = &full_text[pos..];

            // 1. 寻找最早出现的特种块起始标记。
            //    书签恰好落在当前游标时（流式未闭合块续帧的常见情形），起始标记无需重新
            //    识别——它仍在那里且类型不变，跳过 11 条起始正则的全文扫描。
            let start_marker = match pending {
                Some(p) if p.start == pos => Some((0usize, p.marker_len, p.block_type)),
                _ => find_earliest_start_marker(remaining),
            };

            if let Some((start, end, block_type)) = start_marker {
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
                    // 块已闭合，书签失效；后续游标位置必须重新做起始标记识别
                    pending = None;
                } else {
                    #[cfg(test)]
                    println!("[DIAG] FAILED to find end marker for {:?}. Returning remainder from start as tail.", block_type);
                    // 找不到结束标记 → 之前已强制沉淀 md_tail（即 remaining[..start]），
                    // 故此帧游标推进 start 字节，将未闭合块起始作为 tail 返回，消灭重复渲染；
                    // 同时留下书签，下一帧直接续跑该块的结束匹配。
                    // 例外：起始标记顶到输入末尾时（如仍在增长的 "```ru" 围栏行，`$` 可在 EOI
                    // 命中），marker_len 会随追加过期，不留书签，下一帧重新识别起始标记。
                    self.processed_len = pos + start;
                    self.pending_block = (end < remaining.len()).then_some(PendingBlock {
                        block_type,
                        start: pos + start,
                        marker_len: end - start,
                    });
                    return (blocks, remaining[start..].to_string(), Some(block_type));
                }
            } else {
                // 4. 无任何特种块标记 → 全部按段落分割
                let (md_blocks, md_tail) = split_markdown_paragraphs(remaining);
                blocks.extend(md_blocks);
                if md_tail.is_empty() {
                    self.processed_len = full_text.len();
                    return (blocks, String::new(), None);
                } else {
                    self.processed_len = pos + remaining.len() - md_tail.len();
                    return (blocks, md_tail.to_string(), None);
                }
            }
        }

        self.processed_len = full_text.len();
        (blocks, String::new(), None)
    }

    /// 流结束：强制处理剩余 tail 为最后一个 Markdown 块
    pub fn finalize(&mut self, full_text: &str) -> Vec<StreamBlock> {
        let (mut blocks, tail, tail_type) = self.process(full_text);
        let trimmed = tail.trim();
        if !trimmed.is_empty() {
            let nodes = if tail_type == Some(BlockType::HtmlContainer) {
                let mut node = MarkdownNode::raw_html(trimmed.to_string());
                node.compute_hashes_recursively();
                vec![node]
            } else {
                crate::vcp_modules::pre_renderer::parse_markdown_to_ast(trimmed)
            };
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
        self.pending_block = None;
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
        (&HTML_DOC_START, BlockType::HtmlDoc),
        (&ROLE_DIVIDER, BlockType::RoleDivider),
        (&STYLE_TAG_START, BlockType::Style),
        (&GENERIC_CODE_FENCE_START, BlockType::CodeFence),
        (
            &crate::vcp_modules::content_parser::HTML_CONTAINER_OPEN_RE,
            BlockType::HtmlContainer,
        ),
        (
            &crate::vcp_modules::content_parser::TOOL_CALL_SUMMARY_START,
            BlockType::ToolCallSummary,
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

    if *block_type == BlockType::Tool {
        return find_tool_request_end(search_area);
    }

    let m = match block_type {
        BlockType::Tool => unreachable!(),
        BlockType::Thought => THOUGHT_END.find(search_area),
        BlockType::Think => THINK_END.find(search_area),
        BlockType::ToolResult => TOOL_RESULT_END.find(search_area),
        BlockType::Diary => DIARY_END.find(search_area),
        BlockType::ToolCallSummary => {
            crate::vcp_modules::content_parser::TOOL_CALL_SUMMARY_END.find(search_area)
        }
        // 结束围栏的反引号数必须 ≥ 开围栏（CommonMark）：按起始标记动态计数配对，
        // 嵌套围栏（如 ````html 内含 ```）不会被提前误判闭合。
        BlockType::CodeFence => {
            let (s, e, _) = crate::vcp_modules::content_parser::find_matching_fence_end(
                search_area,
                &remaining[start..end],
            );
            return s.zip(e);
        }
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
fn build_daily_note_stream_block(note: ParsedDailyNote) -> StreamBlock {
    match note {
        ParsedDailyNote::Create {
            maid,
            valet,
            date,
            file_name,
            folder,
            content,
        } => {
            let nodes = crate::vcp_modules::chat::pre_renderer::parse_markdown_to_ast(&content);
            let hash = HashAggregator::compute_content_hash(&format!(
                "diary:create\u{1f}{maid}\u{1f}{valet}\u{1f}{date}\u{1f}{file_name}\u{1f}{folder}\u{1f}{content}"
            ));
            StreamBlock::Diary {
                maid,
                valet,
                date,
                file_name,
                folder,
                content,
                nodes: Some(nodes),
                hash,
            }
        }
        ParsedDailyNote::Update {
            maid,
            valet,
            folder,
            target,
            replace,
        } => {
            let target_nodes =
                crate::vcp_modules::chat::pre_renderer::parse_markdown_to_ast(&target);
            let replace_nodes =
                crate::vcp_modules::chat::pre_renderer::parse_markdown_to_ast(&replace);
            let hash = HashAggregator::compute_content_hash(&format!(
                "diary:update\u{1f}{maid}\u{1f}{valet}\u{1f}{folder}\u{1f}{target}\u{1f}{replace}"
            ));
            StreamBlock::DiaryUpdate {
                maid,
                valet,
                folder,
                target,
                replace,
                target_nodes: Some(target_nodes),
                replace_nodes: Some(replace_nodes),
                hash,
            }
        }
    }
}

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
            if let Some(note) = parse_daily_note_tool_request(inner_content) {
                build_daily_note_stream_block(note)
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
        BlockType::Diary => build_daily_note_stream_block(parse_legacy_daily_note(inner_content)),
        BlockType::ToolCallSummary => {
            let items = crate::vcp_modules::content_parser::parse_tool_call_summary(inner_content);
            let hash = HashAggregator::compute_content_hash(inner_content);
            StreamBlock::tool_call_summary(items, inner_content.to_string(), hash)
        }
        BlockType::HtmlDoc => {
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
            // 原文直接切片（含首尾围栏行），定界与提取交给 pulldown：
            // code 不带正则切片的换行工件；lang 为 html 时整块转为全预览卡片。
            let full_text = &remaining[start_idx..end_idx + end_end];
            if let Some((lang, code, _)) =
                crate::vcp_modules::chat::pre_renderer::markdown_parser::parse_fenced_code_block(
                    full_text,
                )
            {
                if lang.eq_ignore_ascii_case("html") {
                    let hash = HashAggregator::compute_content_hash(&code);
                    return StreamBlock::html_preview(code, hash);
                }
                // 非 html：pulldown 已完成定界与提取，直接构造终态节点，
                // 不再把围栏全文重走一遍完整 Markdown 管线（4 道预处理 + pulldown 重提取）
                let mut node =
                    crate::vcp_modules::chat::pre_renderer::markdown_parser::finalized_code_block_node(
                        Some(lang),
                        code,
                    );
                node.compute_hashes_recursively();
                let hash = HashAggregator::compute_content_hash(full_text);
                return StreamBlock::markdown(full_text.to_string(), Some(vec![node]), hash);
            }
            let nodes = crate::vcp_modules::pre_renderer::parse_markdown_to_ast(full_text);
            let hash = HashAggregator::compute_content_hash(full_text);
            StreamBlock::markdown(full_text.to_string(), Some(nodes), hash)
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

        let blocks = extract_inline_buttons(blocks);
        (blocks, tail.to_string())
    } else {
        // 全程没有 \n\n，直接以 tail 形式返回
        (Vec::new(), text.to_string())
    }
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

    fn tool_request(inner: &str) -> String {
        format!("<<<[TOOL_REQUEST]>>>\n{inner}\n<<<[END_TOOL_REQUEST]>>>")
    }

    fn strip_hash(value: &mut serde_json::Value) {
        if let serde_json::Value::Object(map) = value {
            map.remove("hash");
        }
    }

    fn assert_full_and_stream_match(raw: &str) {
        let full_blocks = crate::vcp_modules::content_parser::parse_content(raw);
        let full = full_blocks
            .iter()
            .find(|block| {
                !matches!(
                    block,
                    crate::vcp_modules::content_parser::ContentBlock::Markdown { .. }
                )
            })
            .expect("full parser should produce a semantic block");

        let mut parser = StreamBlockParser::new();
        let stream_blocks = parser.finalize(raw);
        let stream = stream_blocks
            .iter()
            .find(|block| !matches!(block, StreamBlock::Markdown { .. }))
            .expect("stream parser should produce a semantic block");

        let mut full_json = serde_json::to_value(full).expect("serialize full block");
        let mut stream_json = serde_json::to_value(stream).expect("serialize stream block");
        strip_hash(&mut full_json);
        strip_hash(&mut stream_json);
        assert_eq!(full_json, stream_json);
    }

    #[test]
    fn test_code_block_precipitation_failure() {
        // 真实样本 测试文档.txt（include_str! 编译期内嵌，杜绝绝对路径与 CI panic）
        let mut text = include_str!("fixtures/测试文档.txt").to_string();

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

        // 构造包含 HtmlContainer 的测试文本，验证解析器能沉淀出稳定块
        let html_container_text =
            "\n<div class=\"chat-container\">\n<p>Hello inside container</p>\n</div>\n";
        let combined_text = format!("{}{}", text, html_container_text);

        let mut parser = StreamBlockParser::new();
        let blocks = parser.finalize(&combined_text);

        // 断言：解析器应成功沉淀出至少一个稳定块
        assert!(
            !blocks.is_empty(),
            "Parser should successfully yield stable blocks"
        );
    }

    #[test]
    fn test_streaming_typewriter_incremental_precipitation() {
        let mut parser = StreamBlockParser::new();
        let padding = "这里是一段用来填充文本长度以达到测试新设定之八百字节双换行沉淀阈值物理条件的垫片数据。".repeat(10);

        // 模拟第 1 帧：输出到代码块开头，未闭合
        let frame_1 = format!(
            "{}### 维度二：代码高亮\n\n测试流式传输未闭合时：\n\n```rust",
            padding
        );
        let (blocks_1, tail_1, tail_type_1) = parser.process(&frame_1);
        println!("Frame 1 - Blocks: {}, Tail: {:?}", blocks_1.len(), tail_1);
        // 应该成功沉淀出前面的两个 Markdown 块（因 \n\n 物理分段），且 tail 只包含 ```rust
        assert_eq!(blocks_1.len(), 2);
        assert_eq!(tail_1, "```rust");
        assert_eq!(tail_type_1, Some(BlockType::CodeFence));

        // 模拟第 2 帧：代码块流式增量增长，仍未闭合
        let frame_2 = format!(
            "{}### 维度二：代码高亮\n\n测试流式传输未闭合时：\n\n```rust\nuse tokio;\n",
            padding
        );
        let (blocks_2, tail_2, tail_type_2) = parser.process(&frame_2);
        println!("Frame 2 - Blocks: {}, Tail: {:?}", blocks_2.len(), tail_2);
        // 应该没有任何新的 blocks（因为前段已经沉淀，后段未闭合），且 tail 应该是增量代码块且去掉了前段
        assert_eq!(blocks_2.len(), 0);
        assert_eq!(tail_2, "```rust\nuse tokio;\n");
        assert_eq!(tail_type_2, Some(BlockType::CodeFence));

        // 模拟第 3 帧：流式代码块闭合
        let frame_3 = format!(
            "{}### 维度二：代码高亮\n\n测试流式传输未闭合时：\n\n```rust\nuse tokio;\n```",
            padding
        );
        let (blocks_3, tail_3, tail_type_3) = parser.process(&frame_3);
        println!("Frame 3 - Blocks: {}, Tail: {:?}", blocks_3.len(), tail_3);
        // 应该成功闭合代码块并将其沉淀，且 tail 为空
        assert_eq!(blocks_3.len(), 1);
        assert!(tail_3.is_empty());
        assert_eq!(tail_type_3, None);
    }

    #[test]
    fn html_and_cpp_fences_share_the_generic_code_fence_type() {
        for raw in [
            "```html\n<div class=\"card\">streaming",
            "```cpp\nint main() {",
        ] {
            let mut parser = StreamBlockParser::new();
            let (blocks, tail, tail_type) = parser.process(raw);
            assert!(blocks.is_empty());
            assert_eq!(tail, raw);
            assert_eq!(tail_type, Some(BlockType::CodeFence));
        }
    }

    #[test]
    fn closed_html_fence_becomes_html_preview_with_clean_content() {
        let raw = "前文\n\n```html\n<div class=\"card\">hi</div>\n```\n\n后文";
        let mut parser = StreamBlockParser::new();
        let blocks = parser.finalize(raw);
        let preview = blocks
            .iter()
            .find(|b| matches!(b, StreamBlock::HtmlPreview { .. }))
            .expect("html fence should precipitate as HtmlPreview");
        let StreamBlock::HtmlPreview {
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
    }

    #[test]
    fn nested_inner_fence_does_not_close_html_fence_early() {
        // 外层 4 反引号、内层 3 反引号：内层不闭合外层（结束配对按反引号数动态计数）
        let raw = "````html\n<div>\n```js\nconsole.log(1)\n```\n</div>\n````\n\ndone";
        let mut parser = StreamBlockParser::new();
        let blocks = parser.finalize(raw);
        let preview = blocks
            .iter()
            .find(|b| matches!(b, StreamBlock::HtmlPreview { .. }))
            .expect("nested html fence should precipitate as one HtmlPreview");
        let StreamBlock::HtmlPreview { content, .. } = preview else {
            unreachable!()
        };
        assert_eq!(content, "<div>\n```js\nconsole.log(1)\n```\n</div>\n");
    }

    #[test]
    fn html_fence_full_and_stream_wires_match() {
        assert_full_and_stream_match("前文\n\n```html\n<div class=\"card\">hi</div>\n```\n\n后文");
    }

    #[test]
    fn unclosed_html_container_keeps_its_type_through_finalize() {
        let raw = "<div class=\"card\">\n\n<section>one";
        let mut parser = StreamBlockParser::new();
        let (blocks, tail, tail_type) = parser.process(raw);
        assert!(blocks.is_empty());
        assert_eq!(tail, raw);
        assert_eq!(tail_type, Some(BlockType::HtmlContainer));

        let mut final_parser = StreamBlockParser::new();
        let final_blocks = final_parser.finalize(raw);
        assert!(matches!(
            final_blocks.as_slice(),
            [StreamBlock::Markdown {
                content,
                nodes: Some(nodes),
                ..
            }] if content == raw
                && matches!(
                    nodes.as_slice(),
                    [MarkdownNode::RawHtml {
                        content: raw_content,
                        hash: Some(_),
                    }] if raw_content == raw
                )
        ));
    }

    #[test]
    fn daily_note_create_only_stabilizes_after_true_end_marker() {
        let frame_1 = "<<<[TOOL_REQUEST]>>>\ntool_name:「始」DailyNote「末」";
        let frame_2 = "<<<[TOOL_REQUEST]>>>\n\
                       tool_name:「始」DailyNote「末」\n\
                       command:「始」create「末」\n\
                       Content:{始ESCAPE}before\n\
                       <<<[END_TOOL_REQUEST]>>>\n\
                       after";
        let frame_3 = format!("{frame_2}{{末ESCAPE}}\n<<<[END_TOOL_REQUEST]>>>");

        let mut parser = StreamBlockParser::new();
        let (blocks_1, tail_1, _) = parser.process(frame_1);
        assert!(blocks_1.is_empty());
        assert!(tail_1.starts_with("<<<[TOOL_REQUEST]>>>"));

        let (blocks_2, tail_2, _) = parser.process(frame_2);
        assert!(blocks_2.is_empty());
        assert!(tail_2.contains("after"));

        let (blocks_3, tail_3, _) = parser.process(&frame_3);
        assert!(tail_3.is_empty());
        assert!(matches!(
            blocks_3.as_slice(),
            [StreamBlock::Diary { content, .. }]
                if content.contains("<<<[END_TOOL_REQUEST]>>>") && content.ends_with("after")
        ));
    }

    #[test]
    fn daily_note_update_escape_does_not_close_early() {
        let raw = tool_request(
            "tool_name:{始}DailyNote{末}\n\
             command:{始}update{末}\n\
             target:{始ESCAPE}old\n<<<[END_TOOL_REQUEST]>>>\ntext{末ESCAPE}\n\
             replace:{始}new{末}",
        );
        let mut parser = StreamBlockParser::new();
        let blocks = parser.finalize(&raw);
        assert!(matches!(
            blocks.as_slice(),
            [StreamBlock::DiaryUpdate { target, replace, .. }]
                if target.contains("<<<[END_TOOL_REQUEST]>>>") && replace == "new"
        ));
    }

    #[test]
    fn full_and_stream_daily_note_wires_match() {
        let create = tool_request(
            "tool_name:「始」DailyNote「末」\n\
             command:「始」create「末」\n\
             maid:「始」Sakura「末」\n\
             valet:「始」Sebastian「末」\n\
             Date:「始」2026-08-10「末」\n\
             fileName:「始」Log「末」\n\
             folder:「始」daily「末」\n\
             Content:「始」**done**「末」\n\
             Tag:「始」mobile「末」",
        );
        let update = tool_request(
            "tool_name:「始」DailyNote「末」\n\
             target:「始」old「末」\n\
             replace:「始」new「末」",
        );
        let legacy =
            "<<<DailyNoteStart>>>\nMaid: Sakura\nDate: 2026-08-10\nContent: legacy\n<<<DailyNoteEnd>>>";

        assert_full_and_stream_match(&create);
        assert_full_and_stream_match(&update);
        assert_full_and_stream_match(legacy);
    }

    #[test]
    fn finalize_unclosed_daily_note_request_preserves_raw_markdown() {
        let raw = "<<<[TOOL_REQUEST]>>>\n\
                   tool_name:「始」DailyNote「末」\n\
                   command:「始」create「末」\n\
                   Content:{始ESCAPE}unfinished";
        let mut parser = StreamBlockParser::new();
        let blocks = parser.finalize(raw);
        assert!(matches!(
            blocks.as_slice(),
            [StreamBlock::Markdown { content, .. }] if content == raw.trim()
        ));
    }

    #[test]
    fn pending_block_bookmark_matches_one_shot_parse_across_chunked_growth() {
        // 未闭合代码围栏逐块增长（书签续扫路径）与整段一次解析必须产出相同的沉淀块。
        // 7 字节步进会切开围栏起始行（如 "```ru"），覆盖标记未完整时不留书签的回退路径。
        let full = "intro para\n\n```rust\nfn main() {\n    let x = 1;\n}\n```\n\noutro";
        let mut incremental = StreamBlockParser::new();
        let mut inc_blocks = Vec::new();
        let mut fed = String::new();
        let mut seen_unclosed = false;
        while fed.len() < full.len() {
            let end = (fed.len() + 7).min(full.len());
            fed.push_str(&full[fed.len()..end]);
            let (blocks, tail, tail_type) = incremental.process(&fed);
            inc_blocks.extend(blocks);
            if tail_type == Some(BlockType::CodeFence) {
                seen_unclosed = true;
                assert!(
                    tail.starts_with('`'),
                    "tail 必须起自围栏起始标记: {tail:?}"
                );
            }
        }
        assert!(seen_unclosed, "增长过程必须经历未闭合 CodeFence 阶段");
        inc_blocks.extend(incremental.finalize(full));

        let mut oneshot = StreamBlockParser::new();
        let mut one_blocks = oneshot.process(full).0;
        one_blocks.extend(oneshot.finalize(full));
        assert_eq!(
            format!("{:?}", inc_blocks),
            format!("{:?}", one_blocks),
            "书签续扫的增量解析必须与整段一次解析产出一致"
        );
    }

    #[test]
    fn pending_block_bookmark_invalidates_after_block_close() {
        let mut parser = StreamBlockParser::new();
        let part1 = "```js\nlet a = 1;";
        let (_, tail, tail_type) = parser.process(part1);
        assert_eq!(tail_type, Some(BlockType::CodeFence));
        assert_eq!(tail, part1);
        assert!(parser.pending_block.is_some(), "未闭合块必须留下书签");

        let mut text = part1.to_string();
        text.push_str("\n```\n\n```py\nprint(1)\n```");
        let (blocks, tail, tail_type) = parser.process(&text);
        assert_eq!(blocks.len(), 2, "两个围栏都应在同一帧闭合沉淀");
        assert!(tail.is_empty(), "闭合后无剩余 tail: {tail:?}");
        assert_eq!(tail_type, None);
        assert!(
            parser.pending_block.is_none(),
            "块闭合后书签必须失效，不得带入后续帧"
        );

        // 无新数据时保持安静：不重复产出块或 tail
        let (blocks2, tail2, tail_type2) = parser.process(&text);
        assert!(blocks2.is_empty());
        assert!(tail2.is_empty());
        assert_eq!(tail_type2, None);
    }
}

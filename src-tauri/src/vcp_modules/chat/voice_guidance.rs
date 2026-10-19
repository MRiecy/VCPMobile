//! One local control protocol shared by history, rendering and speech projection.
//! Stored model output stays intact. Only standalone, top-level marker lines
//! belong to the protocol; examples inside Markdown and other VCP blocks do not.
use super::content_parser::{
    find_tool_request_end, DIARY_END, DIARY_START, HTML_CONTAINER_OPEN_RE, HTML_DOC_END,
    HTML_DOC_START, STYLE_TAG_END, STYLE_TAG_START, THOUGHT_END, THOUGHT_START,
    TOOL_CALL_SUMMARY_END, TOOL_CALL_SUMMARY_START, TOOL_RESULT_END, TOOL_RESULT_START, TOOL_START,
};
use super::pre_renderer::markdown_parser::find_matching_close_tag;
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use regex::Regex;
use std::{ops::Range, sync::LazyLock};

pub const GUIDANCE_START: &str = "<<<[VOICE_GUIDANCE]>>>";
pub const GUIDANCE_END: &str = "<<<[END_VOICE_GUIDANCE]>>>";
pub const MAX_GUIDANCE_CHARS: usize = 2000;
// A conforming response has one block. Bound recovery work for deliberately
// repeated blocks whose contents try to open scopes over following text.
const MAX_SCOPE_RESCANS: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuidanceBlock {
    /// Whole marker lines, including their newline when present.
    pub range: Range<usize>,
    /// Untrimmed content coordinates in the original source.
    pub content_range: Range<usize>,
    pub content: String,
    pub complete: bool,
    pub valid: bool,
}

#[derive(Debug, Default)]
pub struct GuidanceProjection {
    pub blocks: Vec<GuidanceBlock>,
    pub body: String,
    /// Abnormal repeated scope pollution exceeded the bounded recovery work.
    /// The unclassified suffix is preserved verbatim, never guessed as a block.
    pub exhausted: bool,
}

static THOUGHT_TAG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?is)</?think(?:ing)?\b[^>]*>").expect("literal thought tag pattern")
});

enum ScopeEnd {
    Thought,
    Tool,
    Html(String),
    Pattern(&'static Regex),
    Fixed(usize),
    Markdown(bool),
}

fn contains(ranges: &[Range<usize>], offset: usize) -> bool {
    ranges.iter().any(|range| range.contains(&offset))
}

fn scope_candidates(source: &str) -> Vec<(Range<usize>, ScopeEnd)> {
    let mut candidates = Vec::new();
    let mut markdown = Vec::new();
    let mut html = Vec::new();
    for (event, range) in Parser::new_ext(source, Options::all()).into_offset_iter() {
        match event {
            Event::Start(Tag::CodeBlock(_)) => markdown.push((range.start, true)),
            Event::Start(Tag::BlockQuote(_) | Tag::List(_)) => markdown.push((range.start, false)),
            Event::End(TagEnd::CodeBlock | TagEnd::BlockQuote(_) | TagEnd::List(_)) => {
                if let Some((start, hidden)) = markdown.pop() {
                    candidates.push((start..range.end, ScopeEnd::Markdown(hidden)));
                }
            }
            Event::Start(Tag::HtmlBlock) => html.push(range.start),
            Event::End(TagEnd::HtmlBlock) => {
                if let Some(start) = html.pop() {
                    candidates.push((start..range.end, ScopeEnd::Fixed(range.end)));
                }
            }
            Event::Code(_) => candidates.push((range, ScopeEnd::Markdown(true))),
            Event::InlineHtml(_) => candidates.push((range.clone(), ScopeEnd::Fixed(range.end))),
            _ => {}
        }
    }
    for tag in THOUGHT_TAG.find_iter(source) {
        if !tag.as_str().starts_with("</") {
            candidates.push((tag.range(), ScopeEnd::Thought));
        }
    }
    for captures in HTML_CONTAINER_OPEN_RE.captures_iter(source) {
        let (Some(open), Some(tag)) = (captures.get(0), captures.get(1)) else {
            continue;
        };
        candidates.push((open.range(), ScopeEnd::Html(tag.as_str().to_owned())));
    }
    for open in TOOL_START.find_iter(source) {
        candidates.push((open.range(), ScopeEnd::Tool));
    }
    let paired: [(&'static Regex, &'static Regex); 6] = [
        (&THOUGHT_START, &THOUGHT_END),
        (&TOOL_RESULT_START, &TOOL_RESULT_END),
        (&DIARY_START, &DIARY_END),
        (&TOOL_CALL_SUMMARY_START, &TOOL_CALL_SUMMARY_END),
        (&HTML_DOC_START, &HTML_DOC_END),
        (&STYLE_TAG_START, &STYLE_TAG_END),
    ];
    for (start, end) in paired {
        for open in start.find_iter(source) {
            candidates.push((open.range(), ScopeEnd::Pattern(end)));
        }
    }
    // A semantic owner wins over the CommonMark HTML block beginning at the
    // same position: its real closing tag may precede CommonMark's blank line.
    candidates.sort_by_key(|(range, kind)| (range.start, matches!(kind, ScopeEnd::Fixed(_))));
    candidates
}

fn mask_range(masked: &mut [u8], range: Range<usize>) {
    for byte in &mut masked[range] {
        if !matches!(*byte, b'\n' | b'\r') {
            *byte = b' ';
        }
    }
}

fn scope_ranges(source: &str, rescans: &mut usize) -> (Vec<Range<usize>>, Vec<Range<usize>>, bool) {
    let mut ranges = Vec::new();
    let mut speech_opaque = Vec::new();
    let mut masked = source.as_bytes().to_vec();
    let mut candidates = scope_candidates(source);
    let mut cursor = 0;
    let mut index = 0;
    while index < candidates.len() {
        let (open, kind) = &candidates[index];
        index += 1;
        if open.start < cursor {
            continue;
        }
        let open = open.clone();
        let semantic = !matches!(kind, ScopeEnd::Fixed(_) | ScopeEnd::Markdown(_));
        let hidden = !matches!(kind, ScopeEnd::Fixed(_) | ScopeEnd::Markdown(false));
        let end = match kind {
            ScopeEnd::Tool => find_tool_request_end(&source[open.end..])
                .map_or(source.len(), |(_, end)| open.end + end),
            ScopeEnd::Html(tag) => {
                find_matching_close_tag(source, open.end, tag).map_or(source.len(), |(_, end)| end)
            }
            ScopeEnd::Pattern(pattern) => pattern
                .find_at(source, open.end)
                .map_or(source.len(), |end| end.end()),
            ScopeEnd::Fixed(end) => *end,
            ScopeEnd::Markdown(_) => open.end,
            ScopeEnd::Thought => {
                let mut depth = 1usize;
                let mut end = source.len();
                for tag in THOUGHT_TAG.find_iter(&source[open.end..]) {
                    if tag.as_str().starts_with("</") {
                        depth -= 1;
                        if depth == 0 {
                            end = open.end + tag.end();
                            break;
                        }
                    } else {
                        depth += 1;
                    }
                }
                end
            }
        };
        let crosses_boundary = semantic
            && candidates.iter().any(|(range, derived)| {
                matches!(derived, ScopeEnd::Markdown(_) | ScopeEnd::Fixed(_))
                    && range.start >= open.start
                    && range.start < end
                    && range.end > end
            });
        if crosses_boundary && *rescans == MAX_SCOPE_RESCANS {
            return (ranges, speech_opaque, true);
        }
        if hidden {
            speech_opaque.push(open.start..end);
        }
        // List and quote prose is audible, but nested code remains silent.
        // Its ranges are admitted only by this actual Markdown owner, never
        // pre-installed from text inside a VCP field or a thought.
        if matches!(kind, ScopeEnd::Markdown(false)) {
            for (range, nested) in &candidates {
                if matches!(nested, ScopeEnd::Markdown(true))
                    && range.start >= open.start
                    && range.end <= end
                {
                    speech_opaque.push(range.clone());
                }
            }
        }
        ranges.push(open.start..end);
        cursor = end;
        if semantic {
            mask_range(&mut masked, open.start..end);
        }
        if crosses_boundary {
            *rescans += 1;
            candidates = scope_candidates(&String::from_utf8_lossy(&masked));
            index = 0;
        }
    }
    (ranges, speech_opaque, false)
}

/// Speech uses the same outer-owner boundaries as guidance admission. Normal
/// Markdown quotes and list prose remain audible; code and control bodies do
/// not become new scopes when another text-cleaning pass removes their owner.
pub(crate) fn speech_opaque_projection(source: &str) -> Result<String, String> {
    let (_, mut ranges, exhausted) = scope_ranges(source, &mut 0);
    if exhausted {
        return Err("语音指导格式过于复杂，已停止本次朗读；聊天正文已保留。".into());
    }
    ranges.sort_by_key(|range| range.start);
    let mut body = String::new();
    let mut cursor = 0;
    for range in ranges {
        if range.end <= cursor {
            continue;
        }
        if range.start > cursor {
            body.push_str(&source[cursor..range.start]);
        }
        cursor = range.end;
    }
    body.push_str(&source[cursor..]);
    Ok(body)
}

fn marker_line(line: &str) -> Option<&str> {
    let line = line.trim_end_matches(['\n', '\r']);
    let leading = line.len() - line.trim_start_matches(' ').len();
    // Four spaces / a tab are an indented code example, never a control line.
    if leading > 3 || line.as_bytes().get(leading) == Some(&b'\t') {
        return None;
    }
    Some(line.trim_matches([' ', '\t']))
}

/// `finished` permits the final marker line to end at EOF. During streaming a
/// complete-looking EOF marker remains pending until its line is stable.
fn scan_blocks(source: &str, finished: bool) -> (Vec<GuidanceBlock>, bool) {
    // Ordinary chat/history does not pay for Markdown scope parsing. During
    // streaming the unfinished last line can still be a marker prefix.
    if !source.contains("<<<[VOICE_")
        && (finished
            || !source
                .rsplit('\n')
                .next()
                .and_then(marker_line)
                .is_some_and(|line| !line.is_empty() && GUIDANCE_START.starts_with(line)))
    {
        return (Vec::new(), false);
    }
    let mut rescans = 0;
    let (mut protected, _, scope_exhausted) = scope_ranges(source, &mut rescans);
    if scope_exhausted {
        return (Vec::new(), true);
    }
    let mut masked = source.as_bytes().to_vec();
    let mut exhausted = false;
    let mut blocks = Vec::new();
    let mut active: Option<(usize, usize)> = None;
    let mut nested = false;
    let mut offset = 0;
    for line in source.split_inclusive('\n') {
        let end = offset + line.len();
        let marker = marker_line(line);
        if let Some((start, content_start)) = active {
            if marker == Some(GUIDANCE_START) {
                nested = true;
            }
            if marker == Some(GUIDANCE_END) && (line.ends_with('\n') || finished) {
                let crosses_boundary = protected
                    .iter()
                    .any(|range| range.start >= start && range.start < end && range.end > end);
                if crosses_boundary && rescans == MAX_SCOPE_RESCANS {
                    // Keep the unclassified source intact for history and UI.
                    // Speech sees the explicit failure instead of reading an
                    // uncertain protocol tail or hiding ordinary prose.
                    exhausted = true;
                    active = None;
                    break;
                }
                let content = source[content_start..offset].trim();
                blocks.push(GuidanceBlock {
                    range: start..end,
                    content_range: content_start..offset,
                    content: content.to_owned(),
                    complete: true,
                    valid: !nested
                        && !content.is_empty()
                        && content.chars().count() <= MAX_GUIDANCE_CHARS,
                });
                // The body is free text, not executable Markdown/VCP scope.
                // Keep byte offsets and line breaks while removing every
                // confirmed block from the scope scanner's independent copy.
                mask_range(&mut masked, start..end);
                if crosses_boundary {
                    rescans += 1;
                    let (next, _, scope_exhausted) =
                        scope_ranges(&String::from_utf8_lossy(&masked), &mut rescans);
                    protected = next;
                    if scope_exhausted {
                        exhausted = true;
                        active = None;
                        break;
                    }
                }
                active = None;
                nested = false;
            }
        } else if !contains(&protected, offset + line.len() - line.trim_start().len()) {
            if marker == Some(GUIDANCE_START) {
                active = Some((offset, end));
            } else if !line.ends_with('\n')
                && marker.is_some_and(|text| {
                    !text.is_empty()
                        && GUIDANCE_START.starts_with(text)
                        && (!finished || text.starts_with("<<<[VOICE_"))
                })
            {
                blocks.push(GuidanceBlock {
                    range: offset..end,
                    content_range: end..end,
                    content: String::new(),
                    complete: false,
                    valid: false,
                });
            }
        }
        offset = end;
    }
    if let Some((start, content_start)) = active {
        // Retain all raw text for display/history storage, but never turn an
        // unfinished control block into spoken prose.
        blocks.push(GuidanceBlock {
            range: start..source.len(),
            content_range: content_start..source.len(),
            content: source[content_start..].trim().to_owned(),
            complete: false,
            valid: false,
        });
    }
    (blocks, exhausted)
}

pub fn blocks(source: &str, finished: bool) -> Vec<GuidanceBlock> {
    scan_blocks(source, finished).0
}

pub fn scan(source: &str, finished: bool) -> GuidanceProjection {
    let (blocks, exhausted) = scan_blocks(source, finished);
    let mut result = GuidanceProjection {
        blocks,
        body: String::new(),
        exhausted,
    };
    let mut cursor = 0;
    for block in &result.blocks {
        result.body.push_str(&source[cursor..block.range.start]);
        cursor = block.range.end;
    }
    result.body.push_str(&source[cursor..]);
    result
}

pub fn strip_guidance_for_history(source: &str) -> String {
    scan(source, true).body
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(content: &str) -> String {
        format!("{GUIDANCE_START}\n{content}\n{GUIDANCE_END}\n")
    }

    #[test]
    fn guidance_is_removed_without_touching_audio_tags_or_normal_brackets() {
        let raw = format!(
            "{}[轻笑]正文（保持）[官网](https://example.com)。",
            block("自然清晰")
        );
        let parsed = scan(&raw, true);
        assert_eq!(parsed.blocks.len(), 1);
        assert!(parsed.blocks[0].valid);
        assert_eq!(parsed.blocks[0].content, "自然清晰");
        assert_eq!(
            parsed.body,
            "[轻笑]正文（保持）[官网](https://example.com)。"
        );
        assert_eq!(strip_guidance_for_history(&raw), parsed.body);
    }

    #[test]
    fn every_partial_opening_and_unclosed_body_remain_private() {
        for end in 1..=GUIDANCE_START.len() {
            let raw = format!("正文。\n{}", &GUIDANCE_START[..end]);
            assert_eq!(scan(&raw, false).body, "正文。\n");
        }
        let raw = format!("{GUIDANCE_START}\n声音描述\n{GUIDANCE_END}");
        assert!(!scan(&raw, false).blocks[0].complete);
        assert!(scan(&raw, true).blocks[0].complete);
        assert!(scan(&raw, false).body.is_empty());
        assert_eq!(
            strip_guidance_for_history(&format!("正文。\n{GUIDANCE_START}\n秘密")),
            "正文。\n"
        );
    }

    #[test]
    fn code_quotes_thoughts_and_tool_results_are_examples_not_controls() {
        let example = block("示例，不生效");
        for raw in [
            format!("```text\n{example}```\n"),
            format!("~~~text\n{example}~~~\n"),
            example.lines().map(|line| format!("> {line}\n")).collect(),
            example
                .lines()
                .map(|line| format!("    {line}\n"))
                .collect(),
            format!("<think>\n\n<think>思考</think>\n{example}</think>\n"),
            format!("<<<[TOOL_REQUEST]>>>\n{example}<<<[END_TOOL_REQUEST]>>>\n"),
            format!("[[VCP调用结果信息汇总:\n{example}VCP调用结果结束]]\n"),
            format!("`{GUIDANCE_START}`\n普通正文\n"),
        ] {
            assert!(scan(&raw, true).blocks.is_empty(), "{raw}");
            assert_eq!(strip_guidance_for_history(&raw), raw);
        }
    }

    #[test]
    fn empty_nested_and_oversized_blocks_are_identified_but_never_valid() {
        for content in [
            String::new(),
            "字".repeat(MAX_GUIDANCE_CHARS + 1),
            format!("指导\n{GUIDANCE_START}\n嵌套"),
        ] {
            let parsed = scan(&block(&content), true);
            assert_eq!(parsed.blocks.len(), 1);
            assert!(parsed.blocks[0].complete);
            assert!(!parsed.blocks[0].valid);
            assert!(parsed.body.is_empty());
        }
    }

    #[test]
    fn marker_like_inline_text_and_escaped_examples_stay_ordinary() {
        for raw in [
            format!("示例 {GUIDANCE_START} 内容"),
            format!("\\{GUIDANCE_START}\n内容"),
            format!("{GUIDANCE_START} 同一行还有正文\n"),
            "小于号：\n<".into(),
        ] {
            assert!(scan(&raw, true).blocks.is_empty());
            assert_eq!(strip_guidance_for_history(&raw), raw);
        }
    }

    #[test]
    fn free_text_inside_guidance_cannot_open_a_scope_over_the_following_body() {
        let raw = format!(
            "{}{}正文。",
            block("不要读出工具标记：\n<<<[TOOL_REQUEST]>>>"),
            block("第二个指导")
        );
        let parsed = scan(&raw, true);
        assert_eq!(parsed.blocks.len(), 2);
        assert_eq!(parsed.body, "正文。");
    }

    #[test]
    fn guidance_text_cannot_erase_a_real_following_tool_or_fence_scope() {
        let first = block(&format!(
            "{}\n<<<[TOOL_REQUEST]>>>",
            "字".repeat(MAX_GUIDANCE_CHARS + 1)
        ));
        let tool = format!(
            "<<<[TOOL_REQUEST]>>>\n{}<<<[END_TOOL_REQUEST]>>>\n",
            block("工具内部示例")
        );
        let raw = format!("{first}{tool}正文。");
        let parsed = scan(&raw, true);
        assert_eq!(parsed.blocks.len(), 1);
        assert!(!parsed.blocks[0].valid);
        assert_eq!(parsed.body, format!("{tool}正文。"));

        let first = block("自然一点\n```text");
        let code = format!("```text\n{}```\n", block("代码内部示例"));
        let raw = format!("{first}{code}正文。");
        let parsed = scan(&raw, true);
        assert_eq!(parsed.blocks.len(), 1);
        assert_eq!(parsed.body, format!("{code}正文。"));
    }

    #[test]
    fn wrapped_tool_fields_and_html_containers_keep_their_real_scope() {
        let example = block("字段内部示例");
        for raw in [
            format!("<<<[TOOL_REQUEST]>>>\ncontent:「始」\n<<<[END_TOOL_REQUEST]>>>\n{example}「末」\n<<<[END_TOOL_REQUEST]>>>\n正文。"),
            format!("<div>\n\n{example}\n</div>\n正文。"),
        ] {
            assert!(scan(&raw, true).blocks.is_empty());
            assert_eq!(strip_guidance_for_history(&raw), raw);
        }
    }

    #[test]
    fn repeated_scope_poisoning_has_a_bounded_recovery_budget() {
        let raw = format!(
            "{}正文。",
            block("自然一点\n<think>").repeat(MAX_SCOPE_RESCANS + 2)
        );
        let parsed = scan(&raw, true);
        assert_eq!(parsed.blocks.len(), MAX_SCOPE_RESCANS);
        assert!(parsed.exhausted);
        assert_eq!(
            parsed.body,
            format!("{}正文。", block("自然一点\n<think>").repeat(2))
        );
        assert!(strip_guidance_for_history(&raw).ends_with("正文。"));
    }

    #[test]
    fn actual_outer_opaque_blocks_own_mentions_of_other_scope_openings() {
        for prefix in [
            "<<<[TOOL_REQUEST]>>>\ncontent:「始」<think>「末」\n<<<[END_TOOL_REQUEST]>>>\n",
            "<think>\n<<<[TOOL_REQUEST]>>>\n</think>\n",
            "[[VCP调用结果信息汇总:\n<think>\nVCP调用结果结束]]\n",
        ] {
            let raw = format!("{prefix}{}正文。", block("真正的指导"));
            let parsed = scan(&raw, true);
            assert_eq!(parsed.blocks.len(), 1, "{raw}");
            assert_eq!(parsed.blocks[0].content, "真正的指导");
            assert_eq!(parsed.body, format!("{prefix}正文。"));
        }
    }

    #[test]
    fn tool_fields_cannot_leave_a_markdown_or_html_scope_after_their_real_end() {
        for foreign in ["```\n示例", "> 引用示例", "- 列表示例", "<think>"] {
            let tool = format!("<<<[TOOL_REQUEST]>>>\ncontent:「始」\n{foreign}\n「末」\n<<<[END_TOOL_REQUEST]>>>\n");
            let raw = format!("{tool}{}正文。", block("真正指导"));
            let parsed = scan(&raw, true);
            assert!(!parsed.exhausted, "{raw}");
            assert_eq!(parsed.blocks.len(), 1, "{raw}");
            assert_eq!(parsed.blocks[0].content, "真正指导");
            assert_eq!(parsed.body, format!("{tool}正文。"));
        }
        let tool =
            "<<<[TOOL_REQUEST]>>>\ncontent:「始」\n```\n示例\n「末」\n<<<[END_TOOL_REQUEST]>>>\n";
        let external_code = format!("```text\n{}```\n", block("外部代码示例"));
        let raw = format!("{tool}{external_code}{}正文。", block("真正指导"));
        let parsed = scan(&raw, true);
        assert_eq!(parsed.blocks.len(), 1);
        assert_eq!(parsed.blocks[0].content, "真正指导");
        assert_eq!(parsed.body, format!("{tool}{external_code}正文。"));
    }
}

//! Speech-only projection. This never changes stored or rendered chat content.
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use regex::Regex;
use std::sync::LazyLock;

pub(super) const MAX_AUDIO_TAG_CHARS: usize = 80;

// These are inline controls, not a second scan for message-level special blocks.
static INLINE_CONTROLS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\[\[点击按钮:[^\r\n]*?\]\]|\[button:[^\]\r\n]*\]|@tag\([^\)\r\n]*\)")
        .expect("literal inline control pattern")
});

static TERMINAL_MARKERS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"<<<\[[A-Z_]+\]>>>|<<<DailyNote(?:Start|End)>>>")
        .expect("literal terminal delimiter pattern")
});

fn without_html(source: &str, preserve_contents: bool) -> String {
    // HTML-looking syntax inside code or math belongs to that Markdown node.
    // Removing it first could swallow the closing fence and following prose.
    let mut protected = Vec::new();
    let mut code_start = None;
    for (event, range) in Parser::new_ext(source, Options::ENABLE_MATH).into_offset_iter() {
        match event {
            Event::Code(_) | Event::InlineMath(_) | Event::DisplayMath(_) => protected.push(range),
            Event::Start(Tag::CodeBlock(_)) => code_start = Some(range.start),
            Event::End(TagEnd::CodeBlock) => {
                if let Some(start) = code_start.take() {
                    protected.push(start..range.end);
                }
            }
            _ => {}
        }
    }
    let lower = source.to_ascii_lowercase();
    let mut output = String::new();
    let mut offset = 0;
    let mut protected_cursor = 0;
    while let Some(start) = source[offset..].find('<').map(|index| offset + index) {
        while protected
            .get(protected_cursor)
            .is_some_and(|range| range.end <= start)
        {
            protected_cursor += 1;
        }
        if let Some(range) = protected
            .get(protected_cursor)
            .filter(|range| range.contains(&start))
        {
            output.push_str(&source[offset..range.end]);
            offset = range.end;
            continue;
        }
        output.push_str(&source[offset..start]);
        let Some(end) = source[start..].find('>').map(|index| start + index + 1) else {
            output.push_str(&source[start..]);
            return output;
        };
        let raw = &lower[start + 1..end - 1];
        // Markdown autolinks are not HTML elements. Their labels are handled
        // with ordinary text below, so email addresses remain intact.
        if raw.starts_with("https://")
            || raw.starts_with("http://")
            || (raw.contains('@') && !raw.contains([' ', '\t', '\n', '=']))
        {
            output.push_str(&source[start..end]);
            offset = end;
            continue;
        }
        let closing = raw.starts_with('/');
        let name: String = raw
            .trim_start_matches('/')
            .chars()
            .take_while(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | ':'))
            .collect();
        if !name.starts_with(|ch: char| ch.is_ascii_alphabetic()) && !raw.starts_with('!') {
            output.push('<');
            offset = start + 1;
            continue;
        }
        if matches!(name.as_str(), "br" | "hr") {
            push_block_break(&mut output);
        }
        if preserve_contents
            && matches!(
                name.as_str(),
                "p" | "div" | "li" | "tr" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6"
            )
        {
            push_block_break(&mut output);
        }
        if preserve_contents
            || closing
            || raw.ends_with('/')
            || matches!(
                name.as_str(),
                "area"
                    | "base"
                    | "br"
                    | "col"
                    | "embed"
                    | "hr"
                    | "img"
                    | "input"
                    | "link"
                    | "meta"
                    | "param"
                    | "source"
                    | "track"
                    | "wbr"
            )
        {
            offset = end;
            continue;
        }
        // Suppress the complete HTML element, including nested same-name tags.
        let mut depth = 1usize;
        let mut cursor = end;
        while depth > 0 {
            let Some(next) = lower[cursor..].find('<').map(|index| cursor + index) else {
                return output;
            };
            let Some(tag_end) = lower[next..].find('>').map(|index| next + index + 1) else {
                return output;
            };
            let body = &lower[next + 1..tag_end - 1];
            let tag: String = body
                .trim_start_matches('/')
                .chars()
                .take_while(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | ':'))
                .collect();
            if tag == name {
                if body.starts_with('/') {
                    depth -= 1;
                } else if !body.ends_with('/') {
                    depth += 1;
                }
            }
            cursor = tag_end;
        }
        offset = cursor;
    }
    output.push_str(&source[offset..]);
    output
}

fn push_block_break(output: &mut String) {
    if !output.is_empty() && !output.ends_with('\n') {
        output.push('\n');
    }
}

fn normalize_speech_whitespace(source: &str) -> String {
    let mut output = String::with_capacity(source.len());
    let mut horizontal_space = false;
    for ch in source.chars() {
        if matches!(ch, '\n' | '\r') {
            horizontal_space = false;
            push_block_break(&mut output);
        } else if ch.is_whitespace() {
            horizontal_space = true;
        } else {
            if horizontal_space && !output.is_empty() && !output.ends_with('\n') {
                output.push(' ');
            }
            horizontal_space = false;
            output.push(ch);
        }
    }
    output.truncate(output.trim_end().len());
    output
}

fn is_url_start(source: &str) -> bool {
    source.starts_with("https://") || source.starts_with("http://") || source.starts_with("www.")
}

fn is_speakable_inline_code(code: &str) -> bool {
    code.chars().count() <= 40
        && !is_url_start(code)
        && code.chars().filter(|ch| matches!(ch, '/' | '\\')).count() < 2
        && code.chars().any(char::is_alphanumeric)
}

fn is_mention_boundary(ch: char) -> bool {
    ch.is_whitespace() || "([{（【“‘，。！？；：、!?;:".contains(ch)
}

fn clean_text_artifacts(source: &str) -> String {
    let mut output = String::with_capacity(source.len());
    let mut cursor = 0;
    while cursor < source.len() {
        let rest = &source[cursor..];
        let ch = rest.chars().next().unwrap_or_default();
        if is_url_start(rest)
            && source[..cursor].chars().next_back().is_none_or(|previous| {
                !previous.is_ascii_alphanumeric() && !matches!(previous, '_' | '@')
            })
        {
            let end = rest
                .char_indices()
                .find(|(_, ch)| {
                    ch.is_whitespace() || "<>\"'()[]{}，。！？；：、…“”‘’（）【】".contains(*ch)
                })
                .map_or(rest.len(), |(offset, _)| offset);
            let url_end = rest[..end]
                .trim_end_matches(['.', ',', ';', ':', '!', '?'])
                .len();
            cursor += url_end;
            continue;
        }
        if ch == '@'
            && source[..cursor]
                .chars()
                .next_back()
                .is_none_or(is_mention_boundary)
        {
            let name_start = cursor + 1 + usize::from(rest.starts_with("@!"));
            let name = &source[name_start..];
            let length = name
                .char_indices()
                .take_while(|(_, ch)| ch.is_alphanumeric() || matches!(ch, '_' | '-'))
                .map(|(offset, ch)| offset + ch.len_utf8())
                .last()
                .unwrap_or(0);
            let after = &name[length..];
            // A mention is a standalone token. Dots and a second @ keep
            // identifiers and email-like strings intact.
            if length > 0 && !after.starts_with(['.', '@']) {
                cursor = name_start + length;
                continue;
            }
        }
        output.push(ch);
        cursor += ch.len_utf8();
    }
    output
}

/// The caller admits only stable Markdown or an ordinary terminal remainder.
pub(super) fn project_markdown_block(source: &str) -> String {
    project(source, false)
}

/// The parser has removed an unclosed block's outer opening delimiter. Its
/// contents must survive code/HTML filtering that applies to closed blocks.
pub(super) fn project_terminal_contents(source: &str) -> String {
    project(source, true)
}

fn project(source: &str, terminal_contents: bool) -> String {
    let source = INLINE_CONTROLS.replace_all(source, "");
    let source = if terminal_contents {
        TERMINAL_MARKERS.replace_all(&source, "").into_owned()
    } else {
        source.into_owned()
    };
    let source =
        crate::vcp_modules::chat::pre_renderer::markdown_parser::normalize_math_for_speech(&source);
    let source = without_html(&source, terminal_contents);
    let mut output = String::new();
    let mut opaque = 0usize;
    let mut table_cells = 0usize;
    for event in Parser::new_ext(
        &source,
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_MATH,
    ) {
        match event {
            Event::Start(Tag::Image { .. } | Tag::Strikethrough) => opaque += 1,
            Event::End(TagEnd::Image | TagEnd::Strikethrough) => opaque = opaque.saturating_sub(1),
            Event::Start(Tag::CodeBlock(_)) if !terminal_contents => opaque += 1,
            Event::End(TagEnd::CodeBlock) if !terminal_contents => {
                opaque = opaque.saturating_sub(1)
            }
            Event::Text(text) if opaque == 0 => {
                output.push_str(&text);
            }
            Event::Code(code) if opaque == 0 && is_speakable_inline_code(&code) => {
                output.push_str(&code)
            }
            Event::Start(Tag::TableHead | Tag::TableRow) if opaque == 0 => table_cells = 0,
            Event::Start(Tag::TableCell) if opaque == 0 => {
                if table_cells > 0 {
                    output.push('，');
                }
                table_cells += 1;
            }
            Event::SoftBreak | Event::HardBreak | Event::Rule if opaque == 0 => {
                push_block_break(&mut output)
            }
            Event::End(
                TagEnd::Paragraph
                | TagEnd::Heading(_)
                | TagEnd::Item
                | TagEnd::BlockQuote(_)
                | TagEnd::TableHead
                | TagEnd::TableRow
                | TagEnd::CodeBlock,
            ) if opaque == 0 => push_block_break(&mut output),
            _ => {}
        }
    }
    normalize_speech_whitespace(&clean_text_artifacts(&output))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_formatting_preserves_literal_characters_and_short_inline_code() {
        assert_eq!(
            project_markdown_block(
                "请将参数设为 `true`，文件是 `config.json`。 **重点**，a_b 与 x~y。"
            ),
            "请将参数设为 true，文件是 config.json。 重点，a_b 与 x~y。"
        );
        assert!(!is_speakable_inline_code("https://example.com"));
        assert!(!is_speakable_inline_code("C:\\\\long\\path"));
        assert!(!is_speakable_inline_code("{}[]()"));
        assert!(!is_speakable_inline_code(&"字".repeat(41)));
    }

    #[test]
    fn closed_code_images_strikethrough_and_html_do_not_enter_speech() {
        let raw = "保留~~废弃~~![图片](image.png)文字。\n\n```rust\nsecret()\n```\n\n<div>内部<span>文字</span></div>结尾。";
        assert_eq!(project_markdown_block(raw), "保留文字。\n结尾。");
        assert_eq!(project_markdown_block("    hidden_code()\n"), "");
        assert_eq!(
            project_markdown_block("```html\n<div>\n```\n\n后续正文"),
            "后续正文"
        );
    }

    #[test]
    fn void_and_self_closing_elements_keep_the_following_prose() {
        assert_eq!(
            project_markdown_block("前文<br>后文<hr/><img src='x'>继续<widget/>结尾。"),
            "前文\n后文\n继续结尾。"
        );
    }

    #[test]
    fn urls_preserve_the_following_chinese_prose_and_links_keep_their_labels() {
        assert_eq!(
            project_markdown_block("详情 https://example.com。下一句必须保留。"),
            "详情 。下一句必须保留。"
        );
        assert_eq!(
            project_markdown_block(
                "查看[官方文档](https://example.com)，或 <https://example.com>。"
            ),
            "查看官方文档，或 。"
        );
    }

    #[test]
    fn inline_controls_and_mentions_are_removed_without_damaging_email() {
        assert_eq!(
            project_markdown_block("[button:运行] [[点击按钮:运行]] @tag(动作信息)你好 @Agent @!Agent。邮箱 user@example.com 可用。"),
            "你好 。邮箱 user@example.com 可用。"
        );
        assert_eq!(
            project_markdown_block("prefix@Agent <user@example.com>"),
            "prefix@Agent user@example.com"
        );
    }

    #[test]
    fn headings_lists_tables_and_blank_lines_keep_structural_pauses() {
        assert_eq!(
            project_markdown_block("### 核心结论\n\n- 第一点\n- 第二点\n\n| 名称 | 状态 |\n| --- | --- |\n| 服务 | 正常 |"),
            "核心结论\n第一点\n第二点\n名称，状态\n服务，正常"
        );
        assert_eq!(
            normalize_speech_whitespace("  第一行 \t文字 \r\n\n  第二行  \n"),
            "第一行 文字\n第二行"
        );
    }

    #[test]
    fn renderer_math_delimiters_are_skipped_without_losing_explanation() {
        assert_eq!(
            project_markdown_block(r"先看 $x=1$，然后 \(y=2\)，最后 $$z=3$$。"),
            "先看 ，然后 ，最后 。"
        );
        assert_eq!(
            project_markdown_block("解释\n\\[\nx=1\n\\]\n继续"),
            "解释\n继续"
        );
        assert_eq!(
            project_markdown_block("公式 $x<y>z$ 后续正文"),
            "公式 后续正文"
        );
    }

    #[test]
    fn style_and_audio_tags_are_not_markdown_control_artifacts() {
        assert_eq!(
            project_markdown_block("(温柔)[轻笑]你好。[叹气]继续。"),
            "(温柔)[轻笑]你好。[叹气]继续。"
        );
    }

    #[test]
    fn terminal_unclosed_contents_are_not_filtered_as_a_closed_outer_block() {
        assert_eq!(
            project_terminal_contents("    let result = true;"),
            "let result = true;"
        );
        assert_eq!(
            project_terminal_contents("<p>剩余正文</p><span>继续</span>"),
            "剩余正文\n继续"
        );
        assert_eq!(
            project_terminal_contents("正常 < 3，仍应保留"),
            "正常 < 3，仍应保留"
        );
        assert_eq!(
            project_terminal_contents("正文 $x=1$ https://example.com。结尾。"),
            "正文 。结尾。"
        );
    }
}

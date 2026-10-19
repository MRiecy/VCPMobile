//! Speech-only projection. This never changes stored or rendered chat content.
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use regex::Regex;
use std::sync::LazyLock;

static THOUGHT_TAG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?is)</?think(?:ing)?\b[^>]*>").expect("literal thought pattern")
});
static OPAQUE: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"(?is)\[---\s*VCP元思考链[^\]]*\].*?(?:\[---\s*元思考链结束\s*---\]|$)",
        r"(?is)<<<\[TOOL_REQUEST\]>>>.*?(?:<<<\[END_TOOL_REQUEST\]>>>|$)",
        r"(?is)\[\[VCP调用结果信息汇总:.*?(?:VCP调用结果结束\]\]|$)",
        r"(?s)\[本轮工具调用摘要:.*?(?:\[本轮工具调用摘要结束\]|$)",
        r"(?is)<<<DailyNoteStart>>>.*?(?:<<<DailyNoteEnd>>>|$)",
        r"(?is)<<<\[(?:END_)?ROLE_DIVIDE_[^\]]+\]>>>",
        r"(?s)\[\[点击按钮:.*?\]\]",
    ]
    .into_iter()
    .map(|pattern| Regex::new(pattern).expect("literal speech block pattern"))
    .collect()
});

fn without_thoughts(source: &str) -> String {
    let mut depth = 0usize;
    let mut offset = 0;
    let mut result = String::new();
    for matched in THOUGHT_TAG.find_iter(source) {
        if depth == 0 {
            result.push_str(&source[offset..matched.start()]);
        }
        if matched.as_str().starts_with("</") {
            depth = depth.saturating_sub(1);
        } else {
            depth += 1;
        }
        offset = matched.end();
    }
    if depth == 0 {
        result.push_str(&source[offset..]);
    }
    result
}

fn without_html(source: &str) -> String {
    let lower = source.to_ascii_lowercase();
    let mut output = String::new();
    let mut offset = 0;
    while let Some(start) = source[offset..].find('<').map(|index| offset + index) {
        output.push_str(&source[offset..start]);
        let Some(end) = source[start..].find('>').map(|index| start + index + 1) else {
            return output;
        };
        let raw = &lower[start + 1..end - 1];
        let closing = raw.starts_with('/');
        let name: String = raw
            .trim_start_matches('/')
            .chars()
            .take_while(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | ':'))
            .collect();
        if name.is_empty() {
            offset = end;
            continue;
        }
        if matches!(name.as_str(), "br" | "hr") {
            output.push(' ');
        }
        if closing
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

fn stable_suffix(source: &str) -> &str {
    const PREFIXES: [&str; 5] = [
        "[本轮工具调用摘要:",
        "[--- VCP元思考链",
        "<<<[TOOL_REQUEST]>>>",
        "[[VCP调用结果信息汇总:",
        "<<<DailyNoteStart>>>",
    ];
    let mut end = source.len();
    for (offset, _) in source.char_indices().rev().take(40) {
        if PREFIXES
            .iter()
            .any(|prefix| prefix.starts_with(&source[offset..]))
        {
            end = end.min(offset);
        }
    }
    let source = &source[..end];
    let mut openings = Vec::new();
    let mut escaped = false;
    for (offset, ch) in source.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == '[' {
            openings.push(offset);
        }
        if ch == ']' {
            openings.pop();
        }
    }
    if let Some(start) = openings.first().copied() {
        return &source[..start.saturating_sub(usize::from(
            start > 0 && source.as_bytes()[start - 1] == b'!',
        ))];
    }
    if source.ends_with(']') {
        if let Some(start) = source.rfind('[') {
            return &source[..start];
        }
    }
    if let Some(label_end) = source.rfind("](") {
        if !source[label_end + 2..].contains(')') {
            if let Some(start) = source[..label_end].rfind('[') {
                return &source[..start];
            }
        }
    }
    source
}

pub(super) fn text(source: &str) -> String {
    let mut source = without_thoughts(source);
    for pattern in OPAQUE.iter() {
        source = pattern.replace_all(&source, "").into_owned();
    }
    source = without_html(&source);
    let source = stable_suffix(&source);
    let mut output = String::new();
    let mut opaque = 0usize;
    for event in Parser::new_ext(
        source,
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH,
    ) {
        match event {
            Event::Start(Tag::CodeBlock(_) | Tag::Image { .. }) => opaque += 1,
            Event::End(TagEnd::CodeBlock | TagEnd::Image) => opaque = opaque.saturating_sub(1),
            Event::Text(text) if opaque == 0 => {
                // An unfinished inline-code delimiter must not turn already
                // spoken text into code when the next model delta arrives.
                let text = text.split('`').next().unwrap_or_default();
                output.extend(text.chars().filter(|ch| !matches!(ch, '*' | '_' | '~')));
            }
            Event::SoftBreak | Event::HardBreak | Event::Rule if opaque == 0 => output.push(' '),
            Event::End(
                TagEnd::Paragraph
                | TagEnd::Heading(_)
                | TagEnd::Item
                | TagEnd::TableCell
                | TagEnd::TableRow,
            ) if opaque == 0 => output.push(' '),
            _ => {}
        }
    }
    output.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn never_reads_nested_thoughts_tools_code_or_html_contents() {
        let raw = "你好。<think>秘密<think>更深</think>仍秘密</think>正文。<<<[TOOL_REQUEST]>>>key=value<<<[END_TOOL_REQUEST]>>>\n```js\nsecret()\n```\n<div>不要念<span>内部</span>剩余</div>结尾。";
        assert_eq!(text(raw), "你好。正文。 结尾。");
    }
    #[test]
    fn partial_control_markers_and_links_do_not_escape_to_speech() {
        assert_eq!(text("已完成。<<<[TOOL_"), "已完成。");
        assert_eq!(text("正文[附件"), "正文");
        assert_eq!(text("正文[官网](https://example.com"), "正文");
        assert_eq!(text("正文[官网](https://example.com)。"), "正文官网。");
    }

    #[test]
    fn void_and_self_closing_elements_keep_the_following_prose() {
        assert_eq!(
            text("前文<br>后文<hr/><img src='x'>继续<widget/>结尾。"),
            "前文 后文 继续结尾。"
        );
    }

    #[test]
    fn tool_summary_prefixes_and_unfinished_thought_or_code_are_never_read() {
        let marker = "[本轮工具调用摘要:";
        for (end, ch) in marker.char_indices() {
            assert_eq!(
                text(&format!("正文。{}", &marker[..end + ch.len_utf8()])),
                "正文。"
            );
        }
        assert_eq!(
            text("正文。[本轮工具调用摘要:不可朗读[本轮工具调用摘要结束]结尾。"),
            "正文。结尾。"
        );
        assert_eq!(text("正文。<think>还没闭合"), "正文。");
        assert_eq!(text("正文。\n```rust\nlet secret = 1;"), "正文。");
    }
}

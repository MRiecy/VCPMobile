use crate::vcp_modules::pre_renderer::code_highlighter::highlight_code_block;
use crate::vcp_modules::pre_renderer::markdown_ast::{InlineNode, MarkdownNode};
use lazy_static::lazy_static;
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use regex::Regex;
use std::borrow::Cow;

lazy_static! {
    static ref FENCE_RE: Regex =
        Regex::new(r"(?m)^[ \t]*(`{3,})[a-zA-Z0-9-]*[ \t]*\r?$").unwrap();

    // 合并 LaTeX 匹配：[ ... ] 和 ( ... )
    static ref MATH_RE: Regex = Regex::new(r"(?s)\\\[(?P<display>.*?)\\\]|\\\((?P<inline>.*?)\\\)").unwrap();

    static ref MAGIC_RE: Regex =
        Regex::new(r##"(@![^\s@!]+)|(@[^\s@]+)"##).unwrap();

    static ref HTML_CONTAINER_PLACEHOLDER_RE: Regex =
        Regex::new(r"<!--VCP_HTML_CONTAINER:(\d+)-->").unwrap();

    static ref TAG_SCANNER: Regex = Regex::new(r"(?i)(</?)([a-z0-9\-]+)(\s[^>]*)?>").unwrap();

    static ref INLINE_CODE_RE: Regex = Regex::new(r"(?m)`+[^`\n\r]+`+").unwrap();

    static ref COMMENT_RE: Regex = Regex::new(r"(?s)<!--[\s\S]*?(?:-->|$)").unwrap();

    // 匹配行首 ≥4 空格/Tab 缩进后紧跟 $$ 的模式（块级公式被误判为缩进代码块的根因）
    static ref INDENTED_DOLLAR_RE: Regex =
        Regex::new(r"(?m)^[ \t]{4,}(\$\$)").unwrap();
}

fn is_punctuation(c: char) -> bool {
    c.is_ascii_punctuation()
        || ('\u{2000}'..='\u{206F}').contains(&c)
        || ('\u{3000}'..='\u{303F}').contains(&c)
        || ('\u{FE30}'..='\u{FE4F}').contains(&c)
        || ('\u{FE10}'..='\u{FE1F}').contains(&c)
        || ('\u{FF01}'..='\u{FF0F}').contains(&c)
        || ('\u{FF1A}'..='\u{FF20}').contains(&c)
        || ('\u{FF3B}'..='\u{FF40}').contains(&c)
        || ('\u{FF5B}'..='\u{FF60}').contains(&c)
        || ('\u{FFE0}'..='\u{FFE6}').contains(&c)
        || c == '\u{00B7}'
}

fn get_fence_ranges(text: &str) -> Vec<std::ops::Range<usize>> {
    let mut ranges = Vec::new();
    let mut current_start: Option<(usize, usize)> = None;

    for cap in FENCE_RE.captures_iter(text) {
        let m = cap.get(0).unwrap();
        let backticks = cap.get(1).unwrap().as_str().len();

        match current_start {
            None => {
                current_start = Some((m.start(), backticks));
            }
            Some((start_pos, start_backticks)) => {
                if backticks >= start_backticks {
                    ranges.push(start_pos..m.end());
                    current_start = None;
                }
            }
        }
    }

    if let Some((start_pos, _)) = current_start {
        ranges.push(start_pos..text.len());
    }

    ranges
}

fn apply_flanking_fix(segment: &str) -> String {
    let mut result = String::with_capacity(segment.len() + 8);
    let chars: Vec<char> = segment.chars().collect();
    let len = chars.len();

    let mut in_strong = false;
    let mut in_emphasis = false;
    let mut inline_code_backticks = 0; // 0 means not in inline code

    let mut i = 0;
    while i < len {
        // 1. 换行符重置（支持多种换行符，防跨行状态泄露）
        if chars[i] == '\n' || chars[i] == '\r' || chars[i] == '\u{2028}' || chars[i] == '\u{2029}'
        {
            in_strong = false;
            in_emphasis = false;
            inline_code_backticks = 0;
            result.push(chars[i]);
            i += 1;
            continue;
        }

        // 2. 连续反引号匹配（精准识别行内代码边界）
        if chars[i] == '`' {
            let mut count = 0;
            while i + count < len && chars[i + count] == '`' {
                count += 1;
            }
            if inline_code_backticks == 0 {
                // 开启行内代码
                inline_code_backticks = count;
                for _ in 0..count {
                    result.push('`');
                }
                i += count;
                continue;
            } else if count == inline_code_backticks {
                // 闭合行内代码
                inline_code_backticks = 0;
                for _ in 0..count {
                    result.push('`');
                }
                i += count;
                continue;
            } else {
                // 数量不匹配，视为普通代码内容
                for _ in 0..count {
                    result.push('`');
                }
                i += count;
                continue;
            }
        }

        if inline_code_backticks > 0 {
            result.push(chars[i]);
            i += 1;
            continue;
        }

        if chars[i] == '\\' && i + 1 < len {
            result.push('\\');
            result.push(chars[i + 1]);
            i += 2;
            continue;
        }

        if i + 1 < len && chars[i] == '*' && chars[i + 1] == '*' {
            let has_prev = i > 0;
            let has_next = i + 2 < len;
            let prev_char = if has_prev { Some(chars[i - 1]) } else { None };
            let next_char = if has_next { Some(chars[i + 2]) } else { None };

            let is_left_flanking = {
                if !has_next {
                    false
                } else {
                    let next = next_char.unwrap();
                    let is_next_whitespace = next == ' '
                        || next == '\t'
                        || next == '\n'
                        || next == '\r'
                        || next == '\u{2028}'
                        || next == '\u{2029}';
                    if is_next_whitespace {
                        false
                    } else {
                        let is_next_punctuation = is_punctuation(next);
                        if !is_next_punctuation {
                            true
                        } else {
                            !has_prev || {
                                let prev = prev_char.unwrap();
                                prev == ' '
                                    || prev == '\t'
                                    || prev == '\n'
                                    || prev == '\r'
                                    || prev == '\u{2028}'
                                    || prev == '\u{2029}'
                                    || is_punctuation(prev)
                            }
                        }
                    }
                }
            };

            let is_right_flanking = {
                if !has_prev {
                    false
                } else {
                    let prev = prev_char.unwrap();
                    let is_prev_whitespace = prev == ' '
                        || prev == '\t'
                        || prev == '\n'
                        || prev == '\r'
                        || prev == '\u{2028}'
                        || prev == '\u{2029}';
                    if is_prev_whitespace {
                        false
                    } else {
                        let is_prev_punctuation = is_punctuation(prev);
                        if !is_prev_punctuation {
                            true
                        } else {
                            !has_next || {
                                let next = next_char.unwrap();
                                next == ' '
                                    || next == '\t'
                                    || next == '\n'
                                    || next == '\r'
                                    || next == '\u{2028}'
                                    || next == '\u{2029}'
                                    || is_punctuation(next)
                            }
                        }
                    }
                }
            };

            let is_left_fix = if let (Some(p), Some(n)) = (prev_char, next_char) {
                p.is_alphanumeric() && is_punctuation(n)
            } else {
                false
            };
            let is_left = is_left_flanking || is_left_fix;

            let is_right_fix = if let Some(p) = prev_char {
                is_punctuation(p)
            } else {
                false
            };
            let is_right = is_right_flanking || is_right_fix;

            if !in_strong && is_left {
                if is_left_fix {
                    result.push_str("**\u{200B}");
                } else {
                    result.push_str("**");
                }
                in_strong = true;
            } else if in_strong && is_right {
                if is_right_fix {
                    result.push_str("\u{200B}**");
                } else {
                    result.push_str("**");
                }
                in_strong = false;
            } else {
                result.push_str("**");
            }
            i += 2;
            continue;
        }

        if chars[i] == '*' {
            let has_prev = i > 0;
            let has_next = i + 1 < len;
            let prev_char = if has_prev { Some(chars[i - 1]) } else { None };
            let next_char = if has_next { Some(chars[i + 1]) } else { None };

            // 识别列表项标志：前面是行首或空格，后面是空格
            let is_list_item_marker = {
                let mut prev_is_indent = true;
                if has_prev {
                    let mut temp_idx = i;
                    while temp_idx > 0 {
                        temp_idx -= 1;
                        let c = chars[temp_idx];
                        if c == '\n' || c == '\r' || c == '\u{2028}' || c == '\u{2029}' {
                            break;
                        }
                        if c != ' ' && c != '\t' {
                            prev_is_indent = false;
                            break;
                        }
                    }
                }
                prev_is_indent && next_char == Some(' ')
            };

            if is_list_item_marker {
                result.push('*');
                i += 1;
                continue;
            }

            let is_left_flanking = {
                if !has_next {
                    false
                } else {
                    let next = next_char.unwrap();
                    let is_next_whitespace = next == ' '
                        || next == '\t'
                        || next == '\n'
                        || next == '\r'
                        || next == '\u{2028}'
                        || next == '\u{2029}';
                    if is_next_whitespace {
                        false
                    } else {
                        let is_next_punctuation = is_punctuation(next);
                        if !is_next_punctuation {
                            true
                        } else {
                            !has_prev || {
                                let prev = prev_char.unwrap();
                                prev == ' '
                                    || prev == '\t'
                                    || prev == '\n'
                                    || prev == '\r'
                                    || prev == '\u{2028}'
                                    || prev == '\u{2029}'
                                    || is_punctuation(prev)
                            }
                        }
                    }
                }
            };

            let is_right_flanking = {
                if !has_prev {
                    false
                } else {
                    let prev = prev_char.unwrap();
                    let is_prev_whitespace = prev == ' '
                        || prev == '\t'
                        || prev == '\n'
                        || prev == '\r'
                        || prev == '\u{2028}'
                        || prev == '\u{2029}';
                    if is_prev_whitespace {
                        false
                    } else {
                        let is_prev_punctuation = is_punctuation(prev);
                        if !is_prev_punctuation {
                            true
                        } else {
                            !has_next || {
                                let next = next_char.unwrap();
                                next == ' '
                                    || next == '\t'
                                    || next == '\n'
                                    || next == '\r'
                                    || next == '\u{2028}'
                                    || next == '\u{2029}'
                                    || is_punctuation(next)
                            }
                        }
                    }
                }
            };

            let is_left_fix = if let (Some(p), Some(n)) = (prev_char, next_char) {
                p.is_alphanumeric() && is_punctuation(n)
            } else {
                false
            };
            let is_left = is_left_flanking || is_left_fix;

            let is_right_fix = if let Some(p) = prev_char {
                is_punctuation(p)
            } else {
                false
            };
            let is_right = is_right_flanking || is_right_fix;

            if !in_emphasis && is_left {
                if is_left_fix {
                    result.push_str("*\u{200B}");
                } else {
                    result.push('*');
                }
                in_emphasis = true;
            } else if in_emphasis && is_right {
                if is_right_fix {
                    result.push_str("\u{200B}*");
                } else {
                    result.push('*');
                }
                in_emphasis = false;
            } else {
                result.push('*');
            }
            i += 1;
            continue;
        }

        result.push(chars[i]);
        i += 1;
    }
    result
}

fn fix_flanking_delimiters(text: &str) -> String {
    if !text.contains('*') {
        return text.to_string();
    }

    let mut result = String::with_capacity(text.len() + 16);
    let mut last_end = 0;
    let ranges = get_fence_ranges(text);

    for range in &ranges {
        let segment = &text[last_end..range.start];
        result.push_str(&apply_flanking_fix(segment));
        result.push_str(&text[range.start..range.end]);
        last_end = range.end;
    }

    let tail = &text[last_end..];
    result.push_str(&apply_flanking_fix(tail));

    result
}

fn strip_display_math_indent(text: &str) -> Cow<'_, str> {
    if !text.contains("$$") {
        return Cow::Borrowed(text);
    }
    let mut result = String::with_capacity(text.len());
    let mut last_end = 0;
    let ranges = get_fence_ranges(text);

    for range in &ranges {
        let segment = &text[last_end..range.start];
        result.push_str(INDENTED_DOLLAR_RE.replace_all(segment, "$1").as_ref());
        result.push_str(&text[range.start..range.end]);
        last_end = range.end;
    }

    let tail = &text[last_end..];
    result.push_str(INDENTED_DOLLAR_RE.replace_all(tail, "$1").as_ref());

    Cow::Owned(result)
}

fn preprocess_latex_math(text: &str) -> Cow<'_, str> {
    if !text.contains("\\[") && !text.contains("\\(") {
        return Cow::Borrowed(text);
    }

    let mut result = String::with_capacity(text.len());
    let mut last_end = 0;
    let ranges = get_fence_ranges(text);

    for range in &ranges {
        let segment = &text[last_end..range.start];
        push_math_replaced(&mut result, segment);
        result.push_str(&text[range.start..range.end]);
        last_end = range.end;
    }

    let tail = &text[last_end..];
    push_math_replaced(&mut result, tail);

    Cow::Owned(result)
}

/// 辅助函数：将含有 LaTeX 的片段高效推送到结果缓冲区，不产生中间 String
fn push_math_replaced(dest: &mut String, segment: &str) {
    let mut last_match_end = 0;
    for caps in MATH_RE.captures_iter(segment) {
        let full_match = caps.get(0).unwrap();
        // 推送匹配项之前的普通文本
        dest.push_str(&segment[last_match_end..full_match.start()]);

        // 识别是哪种模式并直接推送
        if let Some(display) = caps.name("display") {
            dest.push_str("$$");
            dest.push_str(display.as_str());
            dest.push_str("$$");
        } else if let Some(inline) = caps.name("inline") {
            dest.push('$');
            dest.push_str(inline.as_str());
            dest.push('$');
        }

        last_match_end = full_match.end();
    }
    // 推送剩余文本
    dest.push_str(&segment[last_match_end..]);
}

/// 提取 HTML 容器块，将其替换为占位符，并递归解析内部 Markdown
#[allow(clippy::type_complexity)]
fn extract_html_containers(text: &str) -> (Cow<'_, str>, Vec<(String, Vec<MarkdownNode>, String)>) {
    if !text.contains('<') {
        return (Cow::Borrowed(text), Vec::new());
    }

    let mut result = String::with_capacity(text.len());
    let mut containers: Vec<(String, Vec<MarkdownNode>, String)> = Vec::new();
    let mut last_pos = 0;

    // 预先收集所有代码围栏的物理范围
    let fences = get_fence_ranges(text);

    // 预先收集所有内联反引号的范围以跳过误提取
    let inline_codes: Vec<(usize, usize)> = INLINE_CODE_RE
        .find_iter(text)
        .map(|m| (m.start(), m.end()))
        .collect();

    for cap in crate::vcp_modules::content_parser::HTML_CONTAINER_OPEN_RE.captures_iter(text) {
        let m = cap.get(0).unwrap();
        let tag = cap.get(1).unwrap().as_str().to_lowercase();

        if m.start() < last_pos {
            continue;
        }

        // 跳过被行内反引号包裹的标签（例如 `<div>`）
        let is_in_inline = inline_codes
            .iter()
            .any(|&(start, end)| m.start() >= start && m.end() <= end);
        if is_in_inline {
            continue;
        }

        // 健壮性防御：如果当前标签处于代码围栏内部，直接跳过
        if fences.iter().any(|range| range.contains(&m.start())) {
            continue;
        }

        // 找到匹配的闭标签（考虑嵌套）
        if let Some((close_start, close_end)) = find_matching_close_tag(text, m.end(), &tag) {
            let open_tag = text[m.start()..m.end()].to_string();
            let inner_text = text[m.end()..close_start].to_string();
            let close_tag = text[close_start..close_end].to_string();

            // 将之前的内容加入结果
            result.push_str(&text[last_pos..m.start()]);

            // 创建占位符
            let placeholder = format!("<!--VCP_HTML_CONTAINER:{}-->", containers.len());
            result.push_str(&placeholder);

            // 递归解析内部内容
            let deindented_inner = trim_common_leading_indent(&inner_text);
            let inner_nodes = parse_markdown_to_ast(&deindented_inner);
            containers.push((open_tag, inner_nodes, close_tag));

            last_pos = close_end;
        }
    }

    result.push_str(&text[last_pos..]);
    (Cow::Owned(result), containers)
}

/// 去除文本中所有非空行的公共前导缩进（空格/制表符）。
/// 用于 HTML 容器内部文本：去除嵌套带来的绝对缩进，保留相对结构，
/// 防止 pulldown-cmark 将缩进内容误识别为 Indented Code Block。
pub(crate) fn trim_common_leading_indent(text: &str) -> String {
    let mut min_indent = usize::MAX;

    // 第一遍：纯计算最小公共前导缩进（利用 split 惰性迭代，零堆分配）
    for line in text.split('\n') {
        let trimmed = line.trim();
        if !trimmed.is_empty() && trimmed != "<br>" && trimmed != "<br/>" {
            let mut indent = 0;
            for c in line.chars() {
                if c == ' ' {
                    indent += 1;
                } else if c == '\t' {
                    indent += 4;
                } else {
                    break;
                }
            }
            if indent < min_indent {
                min_indent = indent;
            }
        }
    }

    if min_indent == usize::MAX || min_indent == 0 {
        return text.to_string();
    }

    // 第二遍：直接 split('\n') 惰性迭代追加到预分容量的 result 中，彻底消除 Vec 缓存
    let mut result = String::with_capacity(text.len());
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            result.push('\n');
        }
        if line.chars().all(|c| c.is_whitespace()) {
            // 保留空行，清除空格噪音
        } else {
            let mut skipped = 0;
            let mut char_indices = line.char_indices();
            let mut skip_bytes = 0;

            while skipped < min_indent {
                if let Some((idx, c)) = char_indices.next() {
                    if c == ' ' {
                        skipped += 1;
                        skip_bytes = idx + 1;
                    } else if c == '\t' {
                        skipped += 4;
                        skip_bytes = idx + 1;
                    } else {
                        break;
                    }
                } else {
                    break;
                }
            }
            result.push_str(&line[skip_bytes..]);
        }
    }

    result
}

fn is_void_html_tag(tag: &str) -> bool {
    matches!(
        tag.to_ascii_lowercase().as_str(),
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
}

/// 从字符串末尾向前查找匹配的 HTML 闭标签，返回 (close_start, close_end)
pub(crate) fn find_matching_close_tag(
    text: &str,
    start_pos: usize,
    tag: &str,
) -> Option<(usize, usize)> {
    let mut depth = 1;
    let search_area = &text[start_pos..];

    // 预先收集 search_area 中所有标准代码围栏的物理范围（支持流式未闭合边界）
    let fence_ranges = get_fence_ranges(search_area);

    // 预先收集 search_area 中所有行内代码范围
    let inline_codes: Vec<(usize, usize)> = INLINE_CODE_RE
        .find_iter(search_area)
        .map(|m| (m.start(), m.end()))
        .collect();

    // 预先收集 search_area 中所有 HTML 注释的物理范围（支持流式未闭合注释边界）
    let mut comment_ranges = Vec::new();
    for m in COMMENT_RE.find_iter(search_area) {
        comment_ranges.push(m.start()..m.end());
    }

    for cap in TAG_SCANNER.captures_iter(search_area) {
        let full_match = cap.get(0).unwrap();
        let cap_start = full_match.start();

        // 跨越围栏防御：如果在开始标签之后、当前扫描标签之前横跨了代码围栏的起点，
        // 说明已经穿透跨越了代码块边界，必须立即强行终止，防止吞噬黑洞
        for range in &fence_ranges {
            if range.start > 0 && range.start < cap_start {
                return None;
            }
        }

        // 健壮性防御：如果当前扫描到的 HTML 标签处于代码块围栏内部，直接跳过
        if fence_ranges.iter().any(|range| range.contains(&cap_start)) {
            continue;
        }

        // 健壮性防御：如果当前扫描到的 HTML 标签处于行内代码内部，直接跳过
        let is_in_inline = inline_codes
            .iter()
            .any(|&(start, end)| cap_start >= start && cap_start < end);
        if is_in_inline {
            continue;
        }

        // 健壮性防御：如果当前扫描到的 HTML 标签处于 HTML 注释内部，直接跳过
        if comment_ranges
            .iter()
            .any(|range| range.contains(&cap_start))
        {
            continue;
        }

        let is_close_tag = cap.get(1).unwrap().as_str() == "</";
        let tag_name = cap.get(2).unwrap().as_str();
        let tag_text = full_match.as_str();
        let is_self_closing = tag_text.trim_end().ends_with("/>");

        if is_void_html_tag(tag_name) || is_self_closing {
            continue;
        }

        if tag_name.eq_ignore_ascii_case(tag) {
            if is_close_tag {
                depth -= 1;
                if depth == 0 {
                    let full_match = cap.get(0).unwrap();
                    return Some((start_pos + full_match.start(), start_pos + full_match.end()));
                }
            } else {
                depth += 1;
            }
        }
    }
    None
}

/// 后处理：将 AST 中的占位符替换为开标签 + 子节点 + 闭标签
fn replace_container_placeholders(
    nodes: &mut Vec<MarkdownNode>,
    containers: &[(String, Vec<MarkdownNode>, String)],
) {
    let mut i = 0;
    while i < nodes.len() {
        if let MarkdownNode::RawHtml { content, .. } = &nodes[i] {
            if let Some(caps) = HTML_CONTAINER_PLACEHOLDER_RE.captures(content) {
                if let Some(idx_match) = caps.get(1) {
                    if let Ok(idx) = idx_match.as_str().parse::<usize>() {
                        if idx < containers.len() {
                            let (open_tag, children, close_tag) = &containers[idx];
                            let mut replacement = Vec::new();
                            replacement.push(MarkdownNode::raw_html(open_tag.clone()));
                            replacement.extend(children.clone());
                            replacement.push(MarkdownNode::raw_html(close_tag.clone()));
                            nodes.splice(i..=i, replacement);
                            i += children.len() + 2;
                            continue;
                        }
                    }
                }
            }
        }
        i += 1;
    }
}

pub fn parse_markdown_to_ast(text: &str) -> Vec<MarkdownNode> {
    parse_markdown_to_ast_opt(text, false)
}

pub fn parse_markdown_to_ast_streaming(text: &str) -> Vec<MarkdownNode> {
    parse_markdown_to_ast_opt(text, true)
}

fn parse_markdown_to_ast_opt(text: &str, is_streaming: bool) -> Vec<MarkdownNode> {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        parse_markdown_to_ast_impl(text, is_streaming)
    }));
    match result {
        Ok(nodes) => nodes,
        Err(e) => {
            log::error!("[PreRender] parse_markdown_to_ast panicked: {:?}", e);
            let mut fallback_node =
                MarkdownNode::paragraph(vec![InlineNode::text(text.to_string())]);
            fallback_node.compute_hashes_recursively();
            vec![fallback_node]
        }
    }
}

fn parse_markdown_to_ast_impl(text: &str, is_streaming: bool) -> Vec<MarkdownNode> {
    let text_fixed = fix_flanking_delimiters(text);
    let text = preprocess_latex_math(&text_fixed);
    let text = strip_display_math_indent(text.as_ref());
    let (text, containers) = extract_html_containers(text.as_ref());

    let mut nodes = Vec::new();
    let parser = Parser::new_ext(
        text.as_ref(),
        Options::ENABLE_MATH | Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH,
    );

    let mut stack: Vec<PartialNode> = Vec::new();
    let mut accumulated_text = String::new();

    let flush_accumulated_text =
        |accumulated: &mut String, stack: &mut Vec<PartialNode>, nodes: &mut Vec<MarkdownNode>| {
            if !accumulated.is_empty() {
                let inline_nodes = if matches!(stack.last(), Some(PartialNode::CodeBlock { .. })) {
                    vec![InlineNode::text(accumulated.clone())]
                } else {
                    process_text_magic(accumulated)
                };
                if let Some(top) = stack.last_mut() {
                    top.push_inlines(inline_nodes);
                } else {
                    nodes.push(MarkdownNode::paragraph(inline_nodes));
                }
                accumulated.clear();
            }
        };

    for event in parser {
        if let Event::Text(text) = event {
            accumulated_text.push_str(&text);
            continue;
        }

        flush_accumulated_text(&mut accumulated_text, &mut stack, &mut nodes);

        match event {
            Event::Start(tag) => {
                stack.push(PartialNode::from_tag(tag));
            }
            Event::Code(code) => {
                if let Some(top) = stack.last_mut() {
                    top.push_inline(InlineNode::code(code.to_string()));
                }
            }
            Event::InlineMath(math) => {
                if let Some(top) = stack.last_mut() {
                    top.push_inline(InlineNode::inline_math(math.to_string(), false));
                } else {
                    nodes.push(MarkdownNode::paragraph(vec![InlineNode::inline_math(
                        math.to_string(),
                        false,
                    )]));
                }
            }
            Event::DisplayMath(math) => {
                let inline_node = InlineNode::inline_math(math.to_string(), true);
                if let Some(parent) = stack.last_mut() {
                    parent.push_inline(inline_node);
                } else {
                    nodes.push(MarkdownNode::paragraph(vec![inline_node]));
                }
            }
            Event::End(tag_end) => {
                if let Some(node) = stack.pop() {
                    match node {
                        PartialNode::Item { children } => {
                            if let Some(parent) = stack.last_mut() {
                                parent.push_list_item(children);
                            }
                        }
                        PartialNode::TableCell { children } => {
                            if let Some(parent) = stack.last_mut() {
                                parent.push_table_cell(children);
                            }
                        }
                        PartialNode::TableHead { cells } => {
                            if let Some(parent) = stack.last_mut() {
                                parent.set_table_header(cells);
                            }
                        }
                        PartialNode::TableRow { cells } => {
                            if let Some(parent) = stack.last_mut() {
                                parent.push_table_row(cells);
                            }
                        }
                        PartialNode::Strong { children } => {
                            let inline = InlineNode::strong(children);
                            if let Some(parent) = stack.last_mut() {
                                parent.push_inline(inline);
                            } else {
                                nodes.push(MarkdownNode::paragraph(vec![inline]));
                            }
                        }
                        PartialNode::Emphasis { children } => {
                            let inline = InlineNode::emphasis(children);
                            if let Some(parent) = stack.last_mut() {
                                parent.push_inline(inline);
                            } else {
                                nodes.push(MarkdownNode::paragraph(vec![inline]));
                            }
                        }
                        PartialNode::Strikethrough { children } => {
                            let inline = InlineNode::strikethrough(children);
                            if let Some(parent) = stack.last_mut() {
                                parent.push_inline(inline);
                            } else {
                                nodes.push(MarkdownNode::paragraph(vec![inline]));
                            }
                        }
                        PartialNode::Link {
                            href,
                            title,
                            children,
                        } => {
                            let needs_asset_conversion =
                                href.starts_with("vcp-asset:") || href.starts_with("/");
                            let mut inline = InlineNode::link(href, title, children);
                            if let InlineNode::Link {
                                needs_asset_conversion: nac,
                                ..
                            } = &mut inline
                            {
                                *nac = needs_asset_conversion;
                            }

                            if let Some(parent) = stack.last_mut() {
                                parent.push_inline(inline);
                            } else {
                                nodes.push(MarkdownNode::paragraph(vec![inline]));
                            }
                        }
                        PartialNode::Image { src, alt, title } => {
                            let needs_asset_conversion =
                                src.starts_with("vcp-asset:") || src.starts_with("/");
                            let mut inline = InlineNode::image(src, alt, title);
                            if let InlineNode::Image {
                                needs_asset_conversion: nac,
                                ..
                            } = &mut inline
                            {
                                *nac = needs_asset_conversion;
                            }

                            if let Some(parent) = stack.last_mut() {
                                parent.push_inline(inline);
                            } else {
                                nodes.push(MarkdownNode::paragraph(vec![inline]));
                            }
                        }
                        _ => {
                            let completed = node.finalize(tag_end, is_streaming);
                            if let Some(parent) = stack.last_mut() {
                                parent.push_child(completed);
                            } else {
                                nodes.push(completed);
                            }
                        }
                    }
                }
            }
            Event::Html(html) => {
                nodes.push(MarkdownNode::raw_html(html.to_string()));
            }
            Event::InlineHtml(html) => {
                if let Some(top) = stack.last_mut() {
                    top.push_inline(InlineNode::raw_html_inline(html.to_string()));
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if let Some(top) = stack.last_mut() {
                    top.push_inline(InlineNode::r#break());
                }
            }
            Event::Rule => {
                nodes.push(MarkdownNode::thematic_break());
            }
            _ => {}
        }
    }

    flush_accumulated_text(&mut accumulated_text, &mut stack, &mut nodes);

    // 后处理：将 HTML 容器占位符替换为实际的开标签 + 解析后的子节点 + 闭标签
    replace_container_placeholders(&mut nodes, &containers);

    // 运行引号 AST 合并逻辑
    for node in &mut nodes {
        apply_quote_merging(node);
    }

    // 计算全量 AST 节点的稳定哈希指纹
    for node in &mut nodes {
        node.compute_hashes_recursively();
    }

    nodes
}

enum PartialNode {
    Paragraph {
        children: Vec<InlineNode>,
    },
    Heading {
        level: u8,
        children: Vec<InlineNode>,
    },
    CodeBlock {
        lang: Option<String>,
        code: String,
    },
    Blockquote {
        children: Vec<MarkdownNode>,
    },
    List {
        ordered: bool,
        items: Vec<Vec<MarkdownNode>>,
    },
    Item {
        children: Vec<MarkdownNode>,
    },
    Table {
        header: Vec<Vec<InlineNode>>,
        rows: Vec<Vec<Vec<InlineNode>>>,
    },
    TableHead {
        cells: Vec<Vec<InlineNode>>,
    },
    TableRow {
        cells: Vec<Vec<InlineNode>>,
    },
    TableCell {
        children: Vec<InlineNode>,
    },
    Link {
        href: String,
        title: Option<String>,
        children: Vec<InlineNode>,
    },
    Image {
        src: String,
        alt: String,
        title: Option<String>,
    },
    Strong {
        children: Vec<InlineNode>,
    },
    Emphasis {
        children: Vec<InlineNode>,
    },
    Strikethrough {
        children: Vec<InlineNode>,
    },
}

impl PartialNode {
    fn from_tag(tag: Tag) -> Self {
        match tag {
            Tag::Paragraph => PartialNode::Paragraph {
                children: Vec::new(),
            },
            Tag::Heading { level, .. } => PartialNode::Heading {
                level: level as u8,
                children: Vec::new(),
            },
            Tag::CodeBlock(kind) => {
                let lang = match kind {
                    CodeBlockKind::Fenced(l) => Some(l.to_string()),
                    CodeBlockKind::Indented => None,
                };
                PartialNode::CodeBlock {
                    lang,
                    code: String::new(),
                }
            }
            Tag::BlockQuote(_) => PartialNode::Blockquote {
                children: Vec::new(),
            },
            Tag::List(start) => PartialNode::List {
                ordered: start.is_some(),
                items: Vec::new(),
            },
            Tag::Item => PartialNode::Item {
                children: Vec::new(),
            },
            Tag::Table(_) => PartialNode::Table {
                header: Vec::new(),
                rows: Vec::new(),
            },
            Tag::TableHead => PartialNode::TableHead { cells: Vec::new() },
            Tag::TableRow => PartialNode::TableRow { cells: Vec::new() },
            Tag::TableCell => PartialNode::TableCell {
                children: Vec::new(),
            },
            Tag::Link {
                dest_url, title, ..
            } => PartialNode::Link {
                href: dest_url.to_string(),
                title: if title.is_empty() {
                    None
                } else {
                    Some(title.to_string())
                },
                children: Vec::new(),
            },
            Tag::Image {
                dest_url, title, ..
            } => PartialNode::Image {
                src: dest_url.to_string(),
                alt: String::new(),
                title: if title.is_empty() {
                    None
                } else {
                    Some(title.to_string())
                },
            },
            Tag::Strong => PartialNode::Strong {
                children: Vec::new(),
            },
            Tag::Emphasis => PartialNode::Emphasis {
                children: Vec::new(),
            },
            Tag::Strikethrough => PartialNode::Strikethrough {
                children: Vec::new(),
            },
            _ => PartialNode::Paragraph {
                children: Vec::new(),
            },
        }
    }

    fn push_inline(&mut self, node: InlineNode) {
        match self {
            PartialNode::Paragraph { children } => children.push(node),
            PartialNode::Heading { children, .. } => children.push(node),
            PartialNode::CodeBlock { code, .. } => {
                if let InlineNode::Text { value } = node {
                    code.push_str(&value);
                }
            }
            PartialNode::Link { children, .. } => children.push(node),
            PartialNode::Image { alt, .. } => {
                if let InlineNode::Text { value } = node {
                    alt.push_str(&value);
                }
            }
            PartialNode::Strong { children } => children.push(node),
            PartialNode::Emphasis { children } => children.push(node),
            PartialNode::Strikethrough { children } => children.push(node),
            PartialNode::TableCell { children } => children.push(node),
            PartialNode::Item { children } | PartialNode::Blockquote { children } => {
                if let Some(MarkdownNode::Paragraph {
                    children: para_children,
                    ..
                }) = children.last_mut()
                {
                    para_children.push(node);
                } else {
                    children.push(MarkdownNode::paragraph(vec![node]));
                }
            }
            _ => {}
        }
    }

    fn push_inlines(&mut self, mut nodes: Vec<InlineNode>) {
        match self {
            PartialNode::Paragraph { children } => children.append(&mut nodes),
            PartialNode::Heading { children, .. } => children.append(&mut nodes),
            PartialNode::CodeBlock { code, .. } => {
                for node in nodes {
                    if let InlineNode::Text { value } = node {
                        code.push_str(&value);
                    }
                }
            }
            PartialNode::Link { children, .. } => children.append(&mut nodes),
            PartialNode::Image { alt, .. } => {
                for node in nodes {
                    if let InlineNode::Text { value } = node {
                        alt.push_str(&value);
                    }
                }
            }
            PartialNode::Strong { children } => children.append(&mut nodes),
            PartialNode::Emphasis { children } => children.append(&mut nodes),
            PartialNode::Strikethrough { children } => children.append(&mut nodes),
            PartialNode::TableCell { children } => children.append(&mut nodes),
            PartialNode::Item { children } | PartialNode::Blockquote { children } => {
                if let Some(MarkdownNode::Paragraph {
                    children: para_children,
                    ..
                }) = children.last_mut()
                {
                    para_children.append(&mut nodes);
                } else {
                    children.push(MarkdownNode::paragraph(nodes));
                }
            }
            _ => {}
        }
    }

    fn push_child(&mut self, node: MarkdownNode) {
        match self {
            PartialNode::Blockquote { children } => children.push(node),
            PartialNode::Item { children } => children.push(node),
            _ => {}
        }
    }

    fn push_list_item(&mut self, item: Vec<MarkdownNode>) {
        if let PartialNode::List { items, .. } = self {
            items.push(item);
        }
    }

    fn push_table_cell(&mut self, cell: Vec<InlineNode>) {
        match self {
            PartialNode::TableHead { cells } => cells.push(cell),
            PartialNode::TableRow { cells } => cells.push(cell),
            _ => {}
        }
    }

    fn set_table_header(&mut self, header: Vec<Vec<InlineNode>>) {
        if let PartialNode::Table { header: h, .. } = self {
            *h = header;
        }
    }

    fn push_table_row(&mut self, row: Vec<Vec<InlineNode>>) {
        if let PartialNode::Table { rows, .. } = self {
            rows.push(row);
        }
    }

    fn finalize(self, _tag_end: TagEnd, is_streaming: bool) -> MarkdownNode {
        match self {
            PartialNode::Paragraph { children } => MarkdownNode::paragraph(children),
            PartialNode::Heading { level, children } => MarkdownNode::heading(level, children),
            PartialNode::CodeBlock { lang, code } => {
                let lang_str = lang.as_deref().unwrap_or("plaintext");
                let highlighted = if lang_str == "mermaid" || (is_streaming && code.len() > 4096) {
                    None
                } else {
                    highlight_code_block(&code, lang_str)
                };
                let mut node = MarkdownNode::code_block(lang, code);
                if let MarkdownNode::CodeBlock {
                    highlighted_html, ..
                } = &mut node
                {
                    *highlighted_html = highlighted;
                }
                node
            }
            PartialNode::Blockquote { children } => MarkdownNode::blockquote(children),
            PartialNode::List { ordered, items } => MarkdownNode::list(ordered, items),
            PartialNode::Table { header, rows } => {
                let mut node = MarkdownNode::table(header, rows);
                if let MarkdownNode::Table { wrapper_class, .. } = &mut node {
                    *wrapper_class = Some("vcp-scrollable no-swipe".to_string());
                }
                node
            }
            PartialNode::Link {
                href,
                title,
                children,
            } => {
                let needs_asset_conversion =
                    href.starts_with("vcp-asset:") || href.starts_with("/");
                let mut node = InlineNode::link(href, title, children);
                if let InlineNode::Link {
                    needs_asset_conversion: nac,
                    ..
                } = &mut node
                {
                    *nac = needs_asset_conversion;
                }
                MarkdownNode::paragraph(vec![node])
            }
            PartialNode::Image { src, alt, title } => {
                let needs_asset_conversion = src.starts_with("vcp-asset:") || src.starts_with("/");
                let mut node = InlineNode::image(src, alt, title);
                if let InlineNode::Image {
                    needs_asset_conversion: nac,
                    ..
                } = &mut node
                {
                    *nac = needs_asset_conversion;
                }
                MarkdownNode::paragraph(vec![node])
            }
            PartialNode::Strong { children } => {
                MarkdownNode::paragraph(vec![InlineNode::strong(children)])
            }
            PartialNode::Emphasis { children } => {
                MarkdownNode::paragraph(vec![InlineNode::emphasis(children)])
            }
            PartialNode::Strikethrough { children } => {
                MarkdownNode::paragraph(vec![InlineNode::strikethrough(children)])
            }
            _ => MarkdownNode::paragraph(Vec::new()),
        }
    }
}

fn process_text_magic(text: &str) -> Vec<InlineNode> {
    if !text.contains('@') {
        return vec![InlineNode::text(text.to_string())];
    }

    let mut nodes = Vec::new();
    let mut last_end = 0;

    for cap in MAGIC_RE.captures_iter(text) {
        let m = cap.get(0).unwrap();
        if m.start() > last_end {
            nodes.push(InlineNode::text(text[last_end..m.start()].to_string()));
        }

        let node = if let Some(alert) = cap.get(1) {
            InlineNode::vcp_custom("alert".to_string(), Some(alert.as_str().to_string()), None)
        } else if let Some(tag) = cap.get(2) {
            InlineNode::vcp_custom(
                "highlight".to_string(),
                Some(tag.as_str().to_string()),
                None,
            )
        } else {
            unreachable!()
        };

        nodes.push(node);
        last_end = m.end();
    }

    if last_end < text.len() {
        nodes.push(InlineNode::text(text[last_end..].to_string()));
    }

    nodes
}

fn split_text_by_quotes(inlines: Vec<InlineNode>) -> Vec<InlineNode> {
    let mut result = Vec::new();
    for node in inlines {
        match node {
            InlineNode::Text { value } => {
                let mut temp = String::new();
                let chars: Vec<char> = value.chars().collect();
                let len = chars.len();
                let mut j = 0;

                while j < len {
                    let c = chars[j];
                    if c == '“' || c == '”' || c == '"' {
                        if !temp.is_empty() {
                            result.push(InlineNode::text(temp.clone()));
                            temp.clear();
                        }
                        result.push(InlineNode::text(c.to_string()));
                    } else {
                        temp.push(c);
                    }
                    j += 1;
                }
                if !temp.is_empty() {
                    result.push(InlineNode::text(temp));
                }
            }
            mut other => {
                match &mut other {
                    InlineNode::Strong { children, .. } => {
                        *children = split_text_by_quotes(children.clone());
                    }
                    InlineNode::Emphasis { children, .. } => {
                        *children = split_text_by_quotes(children.clone());
                    }
                    InlineNode::Link { children, .. } => {
                        *children = split_text_by_quotes(children.clone());
                    }
                    InlineNode::Strikethrough { children, .. } => {
                        *children = split_text_by_quotes(children.clone());
                    }
                    InlineNode::VcpCustom {
                        children: Some(children),
                        ..
                    } => {
                        *children = split_text_by_quotes(children.clone());
                    }
                    _ => {}
                }
                result.push(other);
            }
        }
    }
    result
}

#[allow(clippy::needless_range_loop)]
fn merge_quote_nodes(inlines: Vec<InlineNode>) -> Vec<InlineNode> {
    let mut result = Vec::new();
    let mut i = 0;
    let len = inlines.len();

    while i < len {
        let mut start_idx = None;
        let mut open_char = None;

        if let InlineNode::Text { value } = &inlines[i] {
            if value.starts_with('“') {
                start_idx = Some(i);
                open_char = Some('“');
            } else if value.starts_with('"') {
                start_idx = Some(i);
                open_char = Some('"');
            }
        }

        if let (Some(s_idx), Some(op_c)) = (start_idx, open_char) {
            let cl_c = if op_c == '“' { '”' } else { '"' };
            let mut end_idx = None;
            for j in s_idx..len {
                if let InlineNode::Text { value } = &inlines[j] {
                    if j == s_idx {
                        if value.len() >= 2 && value.ends_with(cl_c) {
                            end_idx = Some(j);
                            break;
                        }
                    } else {
                        if value.ends_with(cl_c) {
                            end_idx = Some(j);
                            break;
                        }
                    }
                }
            }

            if let Some(e_idx) = end_idx {
                let mut children = Vec::new();

                if s_idx == e_idx {
                    if let InlineNode::Text { value } = &inlines[s_idx] {
                        let inner_val = &value[op_c.len_utf8()..value.len() - cl_c.len_utf8()];
                        if !inner_val.is_empty() {
                            children.push(InlineNode::text(inner_val.to_string()));
                        }
                    }
                } else {
                    if let InlineNode::Text { value } = &inlines[s_idx] {
                        let inner_val = &value[op_c.len_utf8()..];
                        if !inner_val.is_empty() {
                            children.push(InlineNode::text(inner_val.to_string()));
                        }
                    }

                    children.extend(inlines[(s_idx + 1)..e_idx].iter().cloned());

                    if let InlineNode::Text { value } = &inlines[e_idx] {
                        let inner_val = &value[..value.len() - cl_c.len_utf8()];
                        if !inner_val.is_empty() {
                            children.push(InlineNode::text(inner_val.to_string()));
                        }
                    }
                }

                // 递归合并内部子节点（绝对不含外层引号，安全防御无限递归）
                let merged_children = merge_quote_nodes(children);

                // 将外层引号和已合并的内部子节点组装起来
                let mut final_children = Vec::new();
                final_children.push(InlineNode::text(op_c.to_string()));
                final_children.extend(merged_children);
                final_children.push(InlineNode::text(cl_c.to_string()));

                result.push(InlineNode::vcp_custom(
                    "quote".to_string(),
                    None,
                    Some(final_children),
                ));
                i = e_idx + 1;
                continue;
            }
        }

        let mut node = inlines[i].clone();
        match &mut node {
            InlineNode::Strong { children, .. } => {
                *children = merge_quote_nodes(children.clone());
            }
            InlineNode::Emphasis { children, .. } => {
                *children = merge_quote_nodes(children.clone());
            }
            InlineNode::Link { children, .. } => {
                *children = merge_quote_nodes(children.clone());
            }
            InlineNode::Strikethrough { children, .. } => {
                *children = merge_quote_nodes(children.clone());
            }
            InlineNode::VcpCustom {
                children: Some(children),
                ..
            } => {
                *children = merge_quote_nodes(children.clone());
            }
            _ => {}
        }
        result.push(node);
        i += 1;
    }

    result
}

fn apply_quote_merging(node: &mut MarkdownNode) {
    match node {
        MarkdownNode::Paragraph { children, .. } => {
            let split = split_text_by_quotes(children.clone());
            *children = merge_quote_nodes(split);
        }
        MarkdownNode::Heading { children, .. } => {
            let split = split_text_by_quotes(children.clone());
            *children = merge_quote_nodes(split);
        }
        MarkdownNode::Blockquote { children, .. } => {
            for child in children {
                apply_quote_merging(child);
            }
        }
        MarkdownNode::List { items, .. } => {
            for item in items {
                for child in item {
                    apply_quote_merging(child);
                }
            }
        }
        MarkdownNode::Table { header, rows, .. } => {
            for cell in header {
                let split = split_text_by_quotes(cell.clone());
                *cell = merge_quote_nodes(split);
            }
            for row in rows {
                for cell in row {
                    let split = split_text_by_quotes(cell.clone());
                    *cell = merge_quote_nodes(split);
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_punctuation() {
        assert!(is_punctuation('“'));
        assert!(is_punctuation('”'));
        assert!(is_punctuation('，'));
        assert!(is_punctuation('。'));
        assert!(is_punctuation('【'));
        assert!(is_punctuation('·'));
    }

    #[test]
    fn test_fix_flanking_delimiters() {
        // 1. “**加粗**” 本身能完美闭合，不应该注入任何字符
        assert_eq!(fix_flanking_delimiters("“**加粗**”"), "“**加粗**”");

        // 2. 这很**“重要”**的 -> 应该在开启后注入，闭合前注入
        assert_eq!(
            fix_flanking_delimiters("这很**“重要”**的"),
            "这很**\u{200B}“重要”\u{200B}**的"
        );

        // 3. 代码块内部的加粗不应该被处理
        let code_text = "```\n这很**“重要”**的\n```";
        assert_eq!(fix_flanking_delimiters(code_text), code_text);

        // 4. 并排复杂的加粗引号：看哪些**“应该加粗的部分没有加粗”**，或者**“不该加粗的部分泄漏了”**
        assert_eq!(
            fix_flanking_delimiters("看哪些**“应该加粗的部分没有加粗”**，或者**“不该加粗的部分泄漏了”**"),
            "看哪些**\u{200B}“应该加粗的部分没有加粗”\u{200B}**，或者**\u{200B}“不该加粗的部分泄漏了”\u{200B}**"
        );
    }

    #[test]
    fn test_quote_merging() {
        // 1. 简单引号合并
        let nodes = parse_markdown_to_ast("“你好”");
        assert_eq!(nodes.len(), 1);
        if let MarkdownNode::Paragraph { children, .. } = &nodes[0] {
            assert_eq!(children.len(), 1);
            if let InlineNode::VcpCustom {
                kind,
                children: Some(ch),
                ..
            } = &children[0]
            {
                assert_eq!(kind, "quote");
                assert_eq!(ch.len(), 3);
                assert_eq!(ch[0], InlineNode::text("“".to_string()));
                assert_eq!(ch[1], InlineNode::text("你好".to_string()));
                assert_eq!(ch[2], InlineNode::text("”".to_string()));
            } else {
                panic!("Expected VcpCustom quote node");
            }
        } else {
            panic!("Expected Paragraph");
        }

        // 2. 引号与加粗嵌套合并： “你**必须**现在走！”
        let nodes = parse_markdown_to_ast("“你**必须**现在走！”");
        assert_eq!(nodes.len(), 1);
        if let MarkdownNode::Paragraph { children, .. } = &nodes[0] {
            assert_eq!(children.len(), 1);
            if let InlineNode::VcpCustom {
                kind,
                children: Some(ch),
                ..
            } = &children[0]
            {
                assert_eq!(kind, "quote");
                assert_eq!(ch.len(), 5);
                assert_eq!(ch[0], InlineNode::text("“".to_string()));
                assert_eq!(ch[1], InlineNode::text("你".to_string()));
                assert!(matches!(ch[2], InlineNode::Strong { .. }));
                assert_eq!(ch[3], InlineNode::text("现在走！".to_string()));
                assert_eq!(ch[4], InlineNode::text("”".to_string()));
            } else {
                panic!("Expected VcpCustom quote node");
            }
        } else {
            panic!("Expected Paragraph");
        }
    }

    #[test]
    fn test_complex_bold_quote_parsing() {
        // 1. 测试单加粗块内包裹两对引号
        let text_single_bold = "看哪些**“应该加粗的部分没有加粗”，或者“不该加粗的部分泄漏了”**";
        let nodes_single_bold = parse_markdown_to_ast(text_single_bold);
        assert_eq!(nodes_single_bold.len(), 1);
        if let MarkdownNode::Paragraph { children, .. } = &nodes_single_bold[0] {
            assert_eq!(children.len(), 2);
            assert_eq!(children[0], InlineNode::text("看哪些".to_string()));
            if let InlineNode::Strong {
                children: strong_children,
                ..
            } = &children[1]
            {
                // \u{200b}, VcpCustom("quote"), Text("，或者"), VcpCustom("quote"), \u{200b}
                assert_eq!(strong_children.len(), 5);
                assert_eq!(strong_children[0], InlineNode::text("\u{200b}".to_string()));
                assert_eq!(strong_children[4], InlineNode::text("\u{200b}".to_string()));

                if let InlineNode::VcpCustom {
                    kind,
                    children: Some(q_ch),
                    ..
                } = &strong_children[1]
                {
                    assert_eq!(kind, "quote");
                    assert_eq!(q_ch[0], InlineNode::text("“".to_string()));
                    assert_eq!(
                        q_ch[1],
                        InlineNode::text("应该加粗的部分没有加粗".to_string())
                    );
                    assert_eq!(q_ch[2], InlineNode::text("”".to_string()));
                }

                assert_eq!(strong_children[2], InlineNode::text("，或者".to_string()));

                if let InlineNode::VcpCustom {
                    kind,
                    children: Some(q_ch),
                    ..
                } = &strong_children[3]
                {
                    assert_eq!(kind, "quote");
                    assert_eq!(q_ch[0], InlineNode::text("“".to_string()));
                    assert_eq!(
                        q_ch[1],
                        InlineNode::text("不该加粗的部分泄漏了".to_string())
                    );
                    assert_eq!(q_ch[2], InlineNode::text("”".to_string()));
                }
            } else {
                panic!("Expected Strong");
            }
        }

        // 2. 测试长文本行隔离及多层复杂嵌套
        let text1 = "主人，如果真的是由于“跨容器截断”导致的 DOM 崩溃，我们在前端解析器的 `contentProcessor.js` 里，必须要加装一个**「自愈判定阀」**：\n\n> **核心逻辑**：只有当 `Marked.parser` 的当前 **AST 嵌套深度等于 0（`astDepth === 0`）**，且不处于任何未闭合的代码块/表格内部时，才允许执行 `<!--brk-->` 物理切片！\n> 如果在深度嵌套里遇到了 `<!--brk-->`，则将其自动**挂起并延后**，直到检测到当前容器完全 `</div>` 闭合，再在根节点上执行优雅 of “呼吸切片”！";
        let nodes1 = parse_markdown_to_ast(text1);
        assert_eq!(nodes1.len(), 2);

        // 验证第一行中的“自愈判定阀”被正确加粗了
        if let MarkdownNode::Paragraph { children, .. } = &nodes1[0] {
            // “自愈判定阀”在第 6 个子节点（i = 5）
            if let InlineNode::Strong {
                children: strong_children,
                ..
            } = &children[5]
            {
                assert_eq!(strong_children.len(), 1);
                assert_eq!(
                    strong_children[0],
                    InlineNode::text("\u{200b}「自愈判定阀」\u{200b}".to_string())
                );
            } else {
                panic!("Expected Strong for self-cure valve");
            }
        }

        // 3. 原本的四星号并排加粗引号测试
        let text = "看哪些**“应该加粗的部分没有加粗”**，或者**“不该加粗的部分泄漏了”**";
        let nodes = parse_markdown_to_ast(text);
        assert_eq!(nodes.len(), 1);
        if let MarkdownNode::Paragraph { children, .. } = &nodes[0] {
            assert_eq!(children.len(), 4);
            assert_eq!(children[0], InlineNode::text("看哪些".to_string()));
            if let InlineNode::Strong {
                children: strong_children,
                ..
            } = &children[1]
            {
                assert_eq!(strong_children.len(), 3);
                assert_eq!(strong_children[0], InlineNode::text("\u{200b}".to_string()));
                assert_eq!(strong_children[2], InlineNode::text("\u{200b}".to_string()));
                if let InlineNode::VcpCustom {
                    kind,
                    children: Some(q_ch),
                    ..
                } = &strong_children[1]
                {
                    assert_eq!(kind, "quote");
                    assert_eq!(
                        q_ch[1],
                        InlineNode::text("应该加粗的部分没有加粗".to_string())
                    );
                } else {
                    panic!("Expected quote 1");
                }
            } else {
                panic!("Expected Strong 1");
            }

            assert_eq!(children[2], InlineNode::text("，或者".to_string()));

            if let InlineNode::Strong {
                children: strong_children,
                ..
            } = &children[3]
            {
                assert_eq!(strong_children.len(), 3);
                assert_eq!(strong_children[0], InlineNode::text("\u{200b}".to_string()));
                assert_eq!(strong_children[2], InlineNode::text("\u{200b}".to_string()));
                if let InlineNode::VcpCustom {
                    kind,
                    children: Some(q_ch),
                    ..
                } = &strong_children[1]
                {
                    assert_eq!(kind, "quote");
                    assert_eq!(
                        q_ch[1],
                        InlineNode::text("不该加粗的部分泄漏了".to_string())
                    );
                } else {
                    panic!("Expected quote 2");
                }
            } else {
                panic!("Expected Strong 2");
            }
        }
    }

    #[test]
    fn test_user_reproduce() {
        let text = include_str!("../fixtures/Strong.txt");
        let nodes = parse_markdown_to_ast(text);

        // 查找包含“自愈判定阀”的 Strong 节点
        let mut found = false;
        for node in &nodes {
            if let MarkdownNode::Paragraph { children, .. } = node {
                for child in children {
                    if let InlineNode::Strong {
                        children: strong_children,
                        ..
                    } = child
                    {
                        if strong_children.len() > 0 {
                            if let InlineNode::Text { value, .. } = &strong_children[0] {
                                if value.contains("自愈判定阀") {
                                    found = true;
                                    // 检查是否正确包含零宽空格
                                    assert!(value.contains("\u{200b}「自愈判定阀」\u{200b}"));
                                }
                            }
                        }
                    }
                }
            }
        }
        assert!(
            found,
            "Could not find bolded '自愈判定阀' node in Strong.txt"
        );
    }

    #[test]
    fn test_code_block_nesting_isolation() {
        // 外层 4 个反引号，内层 3 个反引号
        let text = "````markdown\n这是一段嵌套代码块：\n```rust\nfn main() {\n    // **这不该被flanking修改**\n    let a = \"**hello**\";\n}\n```\n````";
        let fixed = fix_flanking_delimiters(text);

        // 应该完全没有任何修改，因为这段内容全部在 4 个反引号的代码围栏中
        assert_eq!(fixed, text);
    }

    #[test]
    fn test_brk_text_37() {
        // 真实样本 brk.txt，覆盖 parse_content 切分（include_str! 编译期内嵌，杜绝绝对路径）
        let text = include_str!("../fixtures/brk.txt");
        // 模拟 content_parser.rs 的 parse_content 切分
        let blocks = crate::vcp_modules::content_parser::parse_content(text);
        // 断言切分出至少一个块（原诊断 println 已转为 assert）
        assert!(
            !blocks.is_empty(),
            "brk.txt 经 parse_content 后应至少切分出一个块"
        );
    }
}

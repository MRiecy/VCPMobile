use crate::vcp_modules::pre_renderer::code_highlighter::highlight_code_block;
use crate::vcp_modules::pre_renderer::markdown_ast::{InlineNode, MarkdownNode};
use lazy_static::lazy_static;
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use regex::Regex;

lazy_static! {
    static ref FENCE_RE: Regex =
        Regex::new(r"(?m)^[ \t]*```[a-zA-Z0-9-]*[ \t]*$").unwrap();

    // 合并 LaTeX 匹配：[ ... ] 和 ( ... )
    static ref MATH_RE: Regex = Regex::new(r"(?s)\\\[(?P<display>.*?)\\\]|\\\((?P<inline>.*?)\\\)").unwrap();

    static ref MAGIC_RE: Regex =
        Regex::new(r##"(?s)(["“”](?:[^"“”]|\\.)+?["“”])|(@![^\s@!]+)|(@[^\s@]+)"##).unwrap();

    static ref HTML_CONTAINER_OPEN_RE: Regex =
        Regex::new(r"(?im)^[ \t]*<(div|section|article|header|footer|main|aside|figure|figcaption)\b[^>]*>").unwrap();

    static ref HTML_CONTAINER_PLACEHOLDER_RE: Regex =
        Regex::new(r"<!--VCP_HTML_CONTAINER:(\d+)-->").unwrap();

    static ref TAG_SCANNER: Regex = Regex::new(r"(?i)(</?)([a-z0-9\-]+)(\s[^>]*)?>").unwrap();
}

fn preprocess_latex_math(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut last_end = 0;
    let mut in_fence = false;

    // 1. 扫描代码围栏
    for m in FENCE_RE.find_iter(text) {
        let segment = &text[last_end..m.start()];
        if !in_fence {
            // 在围栏外：执行极速公式替换
            push_math_replaced(&mut result, segment);
        } else {
            // 在围栏内：直接追加
            result.push_str(segment);
        }
        result.push_str(m.as_str());
        last_end = m.end();
        in_fence = !in_fence;
    }

    // 2. 处理尾部
    let tail = &text[last_end..];
    if !in_fence {
        push_math_replaced(&mut result, tail);
    } else {
        result.push_str(tail);
    }

    result
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
fn extract_html_containers(text: &str) -> (String, Vec<(String, Vec<MarkdownNode>, String)>) {
    let mut result = String::with_capacity(text.len());
    let mut containers: Vec<(String, Vec<MarkdownNode>, String)> = Vec::new();
    let mut last_pos = 0;

    // 预先收集所有代码围栏的位置以供快速查询 (标准 regex find_iter)
    let fences: Vec<regex::Match> = FENCE_RE.find_iter(text).collect();
    let mut fence_cursor = 0;
    let mut in_fence = false;

    for cap in HTML_CONTAINER_OPEN_RE.captures_iter(text) {
        let m = cap.get(0).unwrap();
        let tag = cap.get(1).unwrap().as_str().to_lowercase();

        if m.start() < last_pos {
            continue;
        }

        // 高效同步围栏状态：跳过当前匹配位置之前的围栏切换
        while fence_cursor < fences.len() && fences[fence_cursor].start() <= m.start() {
            in_fence = !in_fence;
            fence_cursor += 1;
        }

        if in_fence {
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

            // 由于 last_pos 跳跃了，同步同步同步同步同步同步同步同步同步同步同步同步同步同步同步同步同步同步同步同步
            while fence_cursor < fences.len() && fences[fence_cursor].start() < last_pos {
                in_fence = !in_fence;
                fence_cursor += 1;
            }
        }
    }

    result.push_str(&text[last_pos..]);
    (result, containers)
}

/// 去除文本中所有非空行的公共前导缩进（空格/制表符）。
/// 用于 HTML 容器内部文本：去除嵌套带来的绝对缩进，保留相对结构，
/// 防止 pulldown-cmark 将缩进内容误识别为 Indented Code Block。
pub(crate) fn trim_common_leading_indent(text: &str) -> String {
    let mut min_indent = usize::MAX;

    for line in text.split('\n') {
        if !line.chars().all(|c| c.is_whitespace()) {
            let indent = line.chars().take_while(|c| *c == ' ' || *c == '\t').count();
            if indent < min_indent {
                min_indent = indent;
            }
        }
    }

    if min_indent == usize::MAX || min_indent == 0 {
        return text.to_string();
    }

    text.split('\n')
        .map(|line| {
            if line.chars().all(|c| c.is_whitespace()) {
                String::new()
            } else {
                line.chars().skip(min_indent).collect::<String>()
            }
        })
        .collect::<Vec<String>>()
        .join("\n")
}

/// 从字符串末尾向前查找匹配的 HTML 闭标签，返回 (close_start, close_end)
pub(crate) fn find_matching_close_tag(text: &str, start_pos: usize, tag: &str) -> Option<(usize, usize)> {
    let mut depth = 1;
    let search_area = &text[start_pos..];

    for cap in TAG_SCANNER.captures_iter(search_area) {
        let is_close_tag = cap.get(1).unwrap().as_str() == "</";
        let tag_name = cap.get(2).unwrap().as_str();

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
    let text = preprocess_latex_math(text);
    let (text, containers) = extract_html_containers(&text);
    let mut nodes = Vec::new();
    let parser = Parser::new_ext(
        &text,
        Options::ENABLE_MATH | Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH,
    );

    let mut stack: Vec<PartialNode> = Vec::new();

    for event in parser {
        match event {
            Event::Start(tag) => {
                stack.push(PartialNode::from_tag(tag));
            }
            Event::Text(text) => {
                let inline_nodes = if matches!(stack.last(), Some(PartialNode::CodeBlock { .. })) {
                    vec![InlineNode::text(text.to_string())]
                } else {
                    process_text_magic(&text)
                };
                if let Some(top) = stack.last_mut() {
                    top.push_inlines(inline_nodes);
                } else {
                    nodes.push(MarkdownNode::paragraph(inline_nodes));
                }
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
                            let completed = node.finalize(tag_end);
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
            Event::SoftBreak => {
                if let Some(top) = stack.last_mut() {
                    top.push_inline(InlineNode::soft_break());
                }
            }
            Event::HardBreak => {
                if let Some(top) = stack.last_mut() {
                    top.push_inline(InlineNode::line_break());
                }
            }
            Event::Rule => {
                nodes.push(MarkdownNode::thematic_break());
            }
            _ => {}
        }
    }

    // 后处理：将 HTML 容器占位符替换为实际的开标签 + 解析后的子节点 + 闭标签
    replace_container_placeholders(&mut nodes, &containers);

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

    fn finalize(self, _tag_end: TagEnd) -> MarkdownNode {
        match self {
            PartialNode::Paragraph { children } => MarkdownNode::paragraph(children),
            PartialNode::Heading { level, children } => MarkdownNode::heading(level, children),
            PartialNode::CodeBlock { lang, code } => {
                let lang_str = lang.as_deref().unwrap_or("plaintext");
                if lang_str == "mermaid" {
                    MarkdownNode::mermaid(code)
                } else {
                    let highlighted = highlight_code_block(&code, lang_str);
                    let mut node = MarkdownNode::code_block(lang, code);
                    if let MarkdownNode::CodeBlock {
                        highlighted_html, ..
                    } = &mut node
                    {
                        *highlighted_html = highlighted;
                    }
                    node
                }
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
    process_vcp_magic(text)
}

/// 轻量级 inline-only 解析器：只处理标准 Markdown 内联语法（strong/emphasis/strikethrough/code/link/image/math），
/// 不解析 VCP Magic 引号，避免 process_vcp_magic 的无限递归。
fn parse_inline_standard(text: &str) -> Vec<InlineNode> {
    let wrapped = format!("{}\n", text);
    let parser = Parser::new_ext(
        &wrapped,
        Options::ENABLE_MATH | Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH,
    );

    let mut nodes = Vec::new();
    let mut in_paragraph = false;
    let mut stack: Vec<PartialInlineNode> = Vec::new();

    for event in parser {
        match event {
            Event::Start(Tag::Paragraph) => in_paragraph = true,
            Event::End(TagEnd::Paragraph) => in_paragraph = false,
            Event::Text(t) if in_paragraph || !stack.is_empty() => {
                let node = InlineNode::text(t.to_string());
                push_inline_to_context(&mut stack, &mut nodes, node);
            }
            Event::Code(code) => {
                let node = InlineNode::code(code.to_string());
                push_inline_to_context(&mut stack, &mut nodes, node);
            }
            Event::InlineMath(math) => {
                let node = InlineNode::inline_math(math.to_string(), false);
                push_inline_to_context(&mut stack, &mut nodes, node);
            }
            Event::DisplayMath(math) => {
                let node = InlineNode::inline_math(math.to_string(), true);
                push_inline_to_context(&mut stack, &mut nodes, node);
            }
            Event::Start(Tag::Strong) => stack.push(PartialInlineNode::Strong { children: vec![] }),
            Event::End(TagEnd::Strong) => {
                if let Some(PartialInlineNode::Strong { children }) = stack.pop() {
                    let node = InlineNode::strong(children);
                    push_inline_to_context(&mut stack, &mut nodes, node);
                }
            }
            Event::Start(Tag::Emphasis) => {
                stack.push(PartialInlineNode::Emphasis { children: vec![] })
            }
            Event::End(TagEnd::Emphasis) => {
                if let Some(PartialInlineNode::Emphasis { children }) = stack.pop() {
                    let node = InlineNode::emphasis(children);
                    push_inline_to_context(&mut stack, &mut nodes, node);
                }
            }
            Event::Start(Tag::Strikethrough) => {
                stack.push(PartialInlineNode::Strikethrough { children: vec![] })
            }
            Event::End(TagEnd::Strikethrough) => {
                if let Some(PartialInlineNode::Strikethrough { children }) = stack.pop() {
                    let node = InlineNode::strikethrough(children);
                    push_inline_to_context(&mut stack, &mut nodes, node);
                }
            }
            Event::Start(Tag::Link {
                dest_url, title, ..
            }) => {
                stack.push(PartialInlineNode::Link {
                    href: dest_url.to_string(),
                    title: if title.is_empty() {
                        None
                    } else {
                        Some(title.to_string())
                    },
                    children: vec![],
                });
            }
            Event::End(TagEnd::Link) => {
                if let Some(PartialInlineNode::Link {
                    href,
                    title,
                    children,
                }) = stack.pop()
                {
                    let node = InlineNode::link(href, title, children);
                    push_inline_to_context(&mut stack, &mut nodes, node);
                }
            }
            Event::Start(Tag::Image {
                dest_url, title, ..
            }) => {
                stack.push(PartialInlineNode::Image {
                    src: dest_url.to_string(),
                    alt: String::new(),
                    title: if title.is_empty() {
                        None
                    } else {
                        Some(title.to_string())
                    },
                });
            }
            Event::End(TagEnd::Image) => {
                if let Some(PartialInlineNode::Image { src, alt, title }) = stack.pop() {
                    let node = InlineNode::image(src, alt, title);
                    push_inline_to_context(&mut stack, &mut nodes, node);
                }
            }
            Event::SoftBreak => {
                let node = InlineNode::SoftBreak;
                push_inline_to_context(&mut stack, &mut nodes, node);
            }
            Event::HardBreak => {
                let node = InlineNode::LineBreak;
                push_inline_to_context(&mut stack, &mut nodes, node);
            }
            _ => {}
        }
    }

    nodes
}

enum PartialInlineNode {
    Strong {
        children: Vec<InlineNode>,
    },
    Emphasis {
        children: Vec<InlineNode>,
    },
    Strikethrough {
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
}

#[allow(clippy::ptr_arg)]
fn push_inline_to_context(
    stack: &mut Vec<PartialInlineNode>,
    nodes: &mut Vec<InlineNode>,
    node: InlineNode,
) {
    if let Some(top) = stack.last_mut() {
        match top {
            PartialInlineNode::Strong { children } => children.push(node),
            PartialInlineNode::Emphasis { children } => children.push(node),
            PartialInlineNode::Strikethrough { children } => children.push(node),
            PartialInlineNode::Link { children, .. } => children.push(node),
            PartialInlineNode::Image { alt, .. } => {
                if let InlineNode::Text { value } = &node {
                    alt.push_str(value);
                }
            }
        }
    } else {
        nodes.push(node);
    }
}

fn process_vcp_magic(text: &str) -> Vec<InlineNode> {
    let mut nodes = Vec::new();
    let mut last_end = 0;

    for cap in MAGIC_RE.captures_iter(text) {
        let m = cap.get(0).unwrap();
        if m.start() > last_end {
            nodes.push(InlineNode::text(text[last_end..m.start()].to_string()));
        }

        if let Some(quote) = cap.get(1) {
            let quote_text = quote.as_str();
            let children = if quote_text.is_empty() {
                vec![]
            } else {
                parse_inline_standard(quote_text)
            };
            nodes.push(InlineNode::quoted_text(children));
        } else if let Some(alert) = cap.get(2) {
            nodes.push(InlineNode::alert_tag(alert.as_str().to_string()));
        } else if let Some(tag) = cap.get(3) {
            nodes.push(InlineNode::highlight_tag(tag.as_str().to_string()));
        }

        last_end = m.end();
    }

    if last_end < text.len() {
        nodes.push(InlineNode::text(text[last_end..].to_string()));
    }

    nodes
}

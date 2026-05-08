use crate::vcp_modules::pre_renderer::code_highlighter::highlight_code_block;
use crate::vcp_modules::pre_renderer::markdown_ast::{InlineNode, MarkdownNode};
use fancy_regex::Regex;
use lazy_static::lazy_static;
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

lazy_static! {
    static ref FENCE_RE: Regex =
        Regex::new(r"(?m)^[ \t]*```[a-zA-Z0-9-]*[ \t]*$").unwrap();

    static ref DISPLAY_RE: Regex = Regex::new(r"(?s)\\\[(.*?)\\\]").unwrap();
    static ref INLINE_RE: Regex = Regex::new(r"(?s)\\\((.*?)\\\)").unwrap();
    static ref MAGIC_RE: Regex =
        Regex::new(r##"(?s)([\u{0022}\u{201C}\u{201D}](?:[^\u{0022}\u{201C}\u{201D}]|\\.)+?[\u{0022}\u{201C}\u{201D}])|(@![^\s@!]+)|(@[^\s@]+)"##).unwrap();
}

fn preprocess_latex_math(text: &str) -> String {
    let mut segments: Vec<(bool, &str)> = Vec::new();
    let mut last_end = 0;
    let mut in_fence = false;

    for cap in FENCE_RE.find_iter(text) {
        let m = cap.unwrap();
        segments.push((!in_fence, &text[last_end..m.start()]));
        segments.push((in_fence, m.as_str()));
        last_end = m.end();
        in_fence = !in_fence;
    }
    segments.push((!in_fence, &text[last_end..]));

    let mut result = String::new();
    for (is_outside, seg) in segments {
        if !is_outside {
            result.push_str(seg);
            continue;
        }

        let seg2 = seg;
        let mut processed = String::new();
        let mut last = 0;
        for cap in DISPLAY_RE.find_iter(seg2) {
            let m = cap.unwrap();
            processed.push_str(&seg2[last..m.start()]);
            if let Ok(Some(caps)) = DISPLAY_RE.captures(m.as_str()) {
                if let Some(inner) = caps.get(1) {
                    processed.push_str("$$");
                    processed.push_str(inner.as_str());
                    processed.push_str("$$");
                } else {
                    processed.push_str(m.as_str());
                }
            } else {
                processed.push_str(m.as_str());
            }
            last = m.end();
        }
        processed.push_str(&seg2[last..]);

        let seg3 = processed;
        let mut processed = String::new();
        let mut last = 0;
        for cap in INLINE_RE.find_iter(&seg3) {
            let m = cap.unwrap();
            processed.push_str(&seg3[last..m.start()]);
            if let Ok(Some(caps)) = INLINE_RE.captures(m.as_str()) {
                if let Some(inner) = caps.get(1) {
                    processed.push('$');
                    processed.push_str(inner.as_str());
                    processed.push('$');
                } else {
                    processed.push_str(m.as_str());
                }
            } else {
                processed.push_str(m.as_str());
            }
            last = m.end();
        }
        processed.push_str(&seg3[last..]);

        result.push_str(&processed);
    }

    result
}

pub fn parse_markdown_to_ast(text: &str) -> Vec<MarkdownNode> {
    let text = preprocess_latex_math(text);
    let mut nodes = Vec::new();
    let parser = Parser::new_ext(&text, Options::ENABLE_MATH | Options::ENABLE_TABLES);

    let mut stack: Vec<PartialNode> = Vec::new();

    for event in parser {
        match event {
            Event::Start(tag) => {
                stack.push(PartialNode::from_tag(tag));
            }
            Event::Text(text) => {
                let inline_nodes = process_text_magic(&text);
                if let Some(top) = stack.last_mut() {
                    top.push_inlines(inline_nodes);
                } else {
                    nodes.push(MarkdownNode::Paragraph {
                        children: inline_nodes,
                    });
                }
            }
            Event::Code(code) => {
                if let Some(top) = stack.last_mut() {
                    top.push_inline(InlineNode::Code {
                        value: code.to_string(),
                    });
                }
            }
            Event::InlineMath(math) => {
                if let Some(top) = stack.last_mut() {
                    top.push_inline(InlineNode::InlineMath {
                        content: math.to_string(),
                        svg: None,
                        display_mode: false,
                    });
                } else {
                    nodes.push(MarkdownNode::Paragraph {
                        children: vec![InlineNode::InlineMath {
                            content: math.to_string(),
                            svg: None,
                            display_mode: false,
                        }],
                    });
                }
            }
            Event::DisplayMath(math) => {
                let math_node = MarkdownNode::Paragraph {
                    children: vec![InlineNode::InlineMath {
                        content: math.to_string(),
                        svg: None,
                        display_mode: true,
                    }],
                };
                if let Some(parent) = stack.last_mut() {
                    parent.push_child(math_node);
                } else {
                    nodes.push(math_node);
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
                            let inline = InlineNode::Strong { children };
                            if let Some(parent) = stack.last_mut() {
                                parent.push_inline(inline);
                            } else {
                                nodes.push(MarkdownNode::Paragraph {
                                    children: vec![inline],
                                });
                            }
                        }
                        PartialNode::Emphasis { children } => {
                            let inline = InlineNode::Emphasis { children };
                            if let Some(parent) = stack.last_mut() {
                                parent.push_inline(inline);
                            } else {
                                nodes.push(MarkdownNode::Paragraph {
                                    children: vec![inline],
                                });
                            }
                        }
                        PartialNode::Link {
                            href,
                            title,
                            children,
                        } => {
                            let needs_asset_conversion =
                                href.starts_with("vcp-asset:") || href.starts_with("/");
                            let inline = InlineNode::Link {
                                href,
                                title,
                                children,
                                needs_asset_conversion,
                            };
                            if let Some(parent) = stack.last_mut() {
                                parent.push_inline(inline);
                            } else {
                                nodes.push(MarkdownNode::Paragraph {
                                    children: vec![inline],
                                });
                            }
                        }
                        PartialNode::Image { src, alt, title } => {
                            let needs_asset_conversion =
                                src.starts_with("vcp-asset:") || src.starts_with("/");
                            let inline = InlineNode::Image {
                                src,
                                alt,
                                title,
                                needs_asset_conversion,
                            };
                            if let Some(parent) = stack.last_mut() {
                                parent.push_inline(inline);
                            } else {
                                nodes.push(MarkdownNode::Paragraph {
                                    children: vec![inline],
                                });
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
                nodes.push(MarkdownNode::RawHtml {
                    content: html.to_string(),
                });
            }
            Event::InlineHtml(html) => {
                if let Some(top) = stack.last_mut() {
                    top.push_inline(InlineNode::RawHtmlInline {
                        content: html.to_string(),
                    });
                }
            }
            Event::SoftBreak => {
                if let Some(top) = stack.last_mut() {
                    top.push_inline(InlineNode::SoftBreak);
                }
            }
            Event::HardBreak => {
                if let Some(top) = stack.last_mut() {
                    top.push_inline(InlineNode::LineBreak);
                }
            }
            Event::Rule => {
                nodes.push(MarkdownNode::ThematicBreak);
            }
            _ => {}
        }
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
            PartialNode::TableCell { children } => children.push(node),
            PartialNode::Item { children } => {
                if let Some(MarkdownNode::Paragraph {
                    children: para_children,
                }) = children.last_mut()
                {
                    para_children.push(node);
                } else {
                    children.push(MarkdownNode::Paragraph {
                        children: vec![node],
                    });
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
            PartialNode::TableCell { children } => children.append(&mut nodes),
            PartialNode::Item { children } if !nodes.is_empty() => {
                if let Some(MarkdownNode::Paragraph {
                    children: para_children,
                }) = children.last_mut()
                {
                    para_children.append(&mut nodes);
                } else {
                    children.push(MarkdownNode::Paragraph { children: nodes });
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
            PartialNode::Paragraph { children } => MarkdownNode::Paragraph { children },
            PartialNode::Heading { level, children } => MarkdownNode::Heading { level, children },
            PartialNode::CodeBlock { lang, code } => {
                let lang_str = lang.as_deref().unwrap_or("plaintext");
                if lang_str == "mermaid" {
                    MarkdownNode::MermaidPlaceholder { code }
                } else {
                    let highlighted = highlight_code_block(&code, lang_str);
                    MarkdownNode::CodeBlock {
                        lang,
                        code,
                        highlighted_html: highlighted,
                        theme: None,
                    }
                }
            }
            PartialNode::Blockquote { children } => MarkdownNode::Blockquote { children },
            PartialNode::List { ordered, items } => MarkdownNode::List { ordered, items },
            PartialNode::Table { header, rows } => MarkdownNode::Table {
                header,
                rows,
                wrapper_class: Some("vcp-scrollable no-swipe".to_string()),
            },
            PartialNode::Link {
                href,
                title,
                children,
            } => {
                let needs_asset_conversion =
                    href.starts_with("vcp-asset:") || href.starts_with("/");
                MarkdownNode::Paragraph {
                    children: vec![InlineNode::Link {
                        href,
                        title,
                        children,
                        needs_asset_conversion,
                    }],
                }
            }
            PartialNode::Image { src, alt, title } => {
                let needs_asset_conversion = src.starts_with("vcp-asset:") || src.starts_with("/");
                MarkdownNode::Paragraph {
                    children: vec![InlineNode::Image {
                        src,
                        alt,
                        title,
                        needs_asset_conversion,
                    }],
                }
            }
            PartialNode::Strong { children } => MarkdownNode::Paragraph {
                children: vec![InlineNode::Strong { children }],
            },
            PartialNode::Emphasis { children } => MarkdownNode::Paragraph {
                children: vec![InlineNode::Emphasis { children }],
            },
            _ => MarkdownNode::Paragraph {
                children: Vec::new(),
            },
        }
    }
}

fn process_text_magic(text: &str) -> Vec<InlineNode> {
    process_vcp_magic(text)
}

fn process_vcp_magic(text: &str) -> Vec<InlineNode> {
    let mut nodes = Vec::new();
    let mut last_end = 0;

    for cap in MAGIC_RE.captures_iter(text) {
        let cap = cap.unwrap();
        let m = cap.get(0).unwrap();
        if m.start() > last_end {
            nodes.push(InlineNode::Text {
                value: text[last_end..m.start()].to_string(),
            });
        }

        if let Some(quote) = cap.get(1) {
            nodes.push(InlineNode::QuotedText {
                value: quote.as_str().to_string(),
            });
        } else if let Some(alert) = cap.get(2) {
            nodes.push(InlineNode::AlertTag {
                value: alert.as_str().to_string(),
            });
        } else if let Some(tag) = cap.get(3) {
            nodes.push(InlineNode::HighlightTag {
                value: tag.as_str().to_string(),
            });
        }

        last_end = m.end();
    }

    if last_end < text.len() {
        nodes.push(InlineNode::Text {
            value: text[last_end..].to_string(),
        });
    }

    nodes
}

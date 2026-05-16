use serde::{Deserialize, Serialize};
use std::hash::{Hash, Hasher};

/// 块级元素
#[derive(Debug, Clone, Serialize, Deserialize, Hash)]
#[serde(tag = "type")]
pub enum MarkdownNode {
    #[serde(rename = "paragraph")]
    Paragraph {
        children: Vec<InlineNode>,
        #[serde(skip_serializing_if = "Option::is_none")]
        hash: Option<u64>,
    },

    #[serde(rename = "heading")]
    Heading {
        level: u8,
        children: Vec<InlineNode>,
        #[serde(skip_serializing_if = "Option::is_none")]
        hash: Option<u64>,
    },

    #[serde(rename = "code_block")]
    CodeBlock {
        lang: Option<String>,
        code: String,
        highlighted_html: Option<String>, // syntect 预渲染结果
        theme: Option<String>,            // "github-dark" | "github-light"
        #[serde(skip_serializing_if = "Option::is_none")]
        hash: Option<u64>,
    },

    #[serde(rename = "blockquote")]
    Blockquote {
        children: Vec<MarkdownNode>,
        #[serde(skip_serializing_if = "Option::is_none")]
        hash: Option<u64>,
    },

    #[serde(rename = "list")]
    List {
        ordered: bool,
        items: Vec<Vec<MarkdownNode>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        hash: Option<u64>,
    },

    #[serde(rename = "table")]
    Table {
        header: Vec<Vec<InlineNode>>,
        rows: Vec<Vec<Vec<InlineNode>>>,
        wrapper_class: Option<String>, // "vcp-scrollable no-swipe"
        #[serde(skip_serializing_if = "Option::is_none")]
        hash: Option<u64>,
    },

    #[serde(rename = "thematic_break")]
    ThematicBreak,

    #[serde(rename = "raw_html")]
    RawHtml {
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        hash: Option<u64>,
    },

    #[serde(rename = "mermaid")]
    MermaidPlaceholder {
        code: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        hash: Option<u64>,
    },
}

/// 行内元素
#[derive(Debug, Clone, Serialize, Deserialize, Hash)]
#[serde(tag = "type")]
pub enum InlineNode {
    #[serde(rename = "text")]
    Text { value: String },

    #[serde(rename = "strong")]
    Strong {
        children: Vec<InlineNode>,
        #[serde(skip_serializing_if = "Option::is_none")]
        hash: Option<u64>,
    },

    #[serde(rename = "emphasis")]
    Emphasis {
        children: Vec<InlineNode>,
        #[serde(skip_serializing_if = "Option::is_none")]
        hash: Option<u64>,
    },

    #[serde(rename = "code")]
    Code { value: String },

    #[serde(rename = "link")]
    Link {
        href: String,
        title: Option<String>,
        children: Vec<InlineNode>,
        needs_asset_conversion: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        hash: Option<u64>,
    },

    #[serde(rename = "image")]
    Image {
        src: String,
        alt: String,
        title: Option<String>,
        needs_asset_conversion: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        hash: Option<u64>,
    },

    #[serde(rename = "line_break")]
    LineBreak,

    #[serde(rename = "soft_break")]
    SoftBreak,

    #[serde(rename = "inline_math")]
    InlineMath {
        content: String,
        display_mode: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        hash: Option<u64>,
    },

    // VCP 魔法标记
    #[serde(rename = "quoted_text")]
    QuotedText {
        children: Vec<InlineNode>,
        #[serde(skip_serializing_if = "Option::is_none")]
        hash: Option<u64>,
    },

    #[serde(rename = "strikethrough")]
    Strikethrough {
        children: Vec<InlineNode>,
        #[serde(skip_serializing_if = "Option::is_none")]
        hash: Option<u64>,
    },

    #[serde(rename = "highlight_tag")]
    HighlightTag { value: String }, // #标签

    #[serde(rename = "alert_tag")]
    AlertTag { value: String }, // !告警

    #[serde(rename = "raw_html_inline")]
    RawHtmlInline {
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        hash: Option<u64>,
    },
}

impl MarkdownNode {
    pub fn paragraph(children: Vec<InlineNode>) -> Self {
        Self::Paragraph {
            children,
            hash: None,
        }
    }

    pub fn heading(level: u8, children: Vec<InlineNode>) -> Self {
        Self::Heading {
            level,
            children,
            hash: None,
        }
    }

    pub fn code_block(lang: Option<String>, code: String) -> Self {
        Self::CodeBlock {
            lang,
            code,
            highlighted_html: None,
            theme: None,
            hash: None,
        }
    }

    pub fn blockquote(children: Vec<MarkdownNode>) -> Self {
        Self::Blockquote {
            children,
            hash: None,
        }
    }

    pub fn list(ordered: bool, items: Vec<Vec<MarkdownNode>>) -> Self {
        Self::List {
            ordered,
            items,
            hash: None,
        }
    }

    pub fn table(header: Vec<Vec<InlineNode>>, rows: Vec<Vec<Vec<InlineNode>>>) -> Self {
        Self::Table {
            header,
            rows,
            wrapper_class: None,
            hash: None,
        }
    }

    pub fn thematic_break() -> Self {
        Self::ThematicBreak
    }

    pub fn raw_html(content: String) -> Self {
        Self::RawHtml {
            content,
            hash: None,
        }
    }

    pub fn mermaid(code: String) -> Self {
        Self::MermaidPlaceholder { code, hash: None }
    }

    pub fn compute_hash(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.hash(&mut hasher);
        hasher.finish()
    }

    pub fn set_hash(&mut self, h: u64) {
        match self {
            MarkdownNode::Paragraph { hash, .. } => *hash = Some(h),
            MarkdownNode::Heading { hash, .. } => *hash = Some(h),
            MarkdownNode::CodeBlock { hash, .. } => *hash = Some(h),
            MarkdownNode::Blockquote { hash, .. } => *hash = Some(h),
            MarkdownNode::List { hash, .. } => *hash = Some(h),
            MarkdownNode::Table { hash, .. } => *hash = Some(h),
            MarkdownNode::ThematicBreak => {}
            MarkdownNode::RawHtml { hash, .. } => *hash = Some(h),
            MarkdownNode::MermaidPlaceholder { hash, .. } => *hash = Some(h),
        }
    }

    pub fn compute_hashes_recursively(&mut self) {
        match self {
            MarkdownNode::Paragraph { children, .. } => {
                for c in children {
                    c.compute_hashes_recursively();
                }
            }
            MarkdownNode::Heading { children, .. } => {
                for c in children {
                    c.compute_hashes_recursively();
                }
            }
            MarkdownNode::Blockquote { children, .. } => {
                for node in children {
                    node.compute_hashes_recursively();
                }
            }
            MarkdownNode::List { items, .. } => {
                for item in items {
                    for node in item {
                        node.compute_hashes_recursively();
                    }
                }
            }
            MarkdownNode::Table { header, rows, .. } => {
                for cell in header {
                    for node in cell {
                        node.compute_hashes_recursively();
                    }
                }
                for row in rows {
                    for cell in row {
                        for node in cell {
                            node.compute_hashes_recursively();
                        }
                    }
                }
            }
            _ => {}
        }
        let h = self.compute_hash();
        self.set_hash(h);
    }
}

impl InlineNode {
    pub fn text(value: String) -> Self {
        Self::Text { value }
    }

    pub fn strong(children: Vec<InlineNode>) -> Self {
        Self::Strong {
            children,
            hash: None,
        }
    }

    pub fn emphasis(children: Vec<InlineNode>) -> Self {
        Self::Emphasis {
            children,
            hash: None,
        }
    }

    pub fn code(value: String) -> Self {
        Self::Code { value }
    }

    pub fn link(href: String, title: Option<String>, children: Vec<InlineNode>) -> Self {
        Self::Link {
            href,
            title,
            children,
            needs_asset_conversion: false,
            hash: None,
        }
    }

    pub fn image(src: String, alt: String, title: Option<String>) -> Self {
        Self::Image {
            src,
            alt,
            title,
            needs_asset_conversion: false,
            hash: None,
        }
    }

    pub fn line_break() -> Self {
        Self::LineBreak
    }

    pub fn soft_break() -> Self {
        Self::SoftBreak
    }

    pub fn inline_math(content: String, display_mode: bool) -> Self {
        Self::InlineMath {
            content,
            display_mode,
            hash: None,
        }
    }

    pub fn quoted_text(children: Vec<InlineNode>) -> Self {
        Self::QuotedText {
            children,
            hash: None,
        }
    }

    pub fn strikethrough(children: Vec<InlineNode>) -> Self {
        Self::Strikethrough {
            children,
            hash: None,
        }
    }

    pub fn highlight_tag(value: String) -> Self {
        Self::HighlightTag { value }
    }

    pub fn alert_tag(value: String) -> Self {
        Self::AlertTag { value }
    }

    pub fn raw_html_inline(content: String) -> Self {
        Self::RawHtmlInline {
            content,
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
            InlineNode::Text { .. } => {}
            InlineNode::Strong { hash, .. } => *hash = Some(h),
            InlineNode::Emphasis { hash, .. } => *hash = Some(h),
            InlineNode::Code { .. } => {}
            InlineNode::Link { hash, .. } => *hash = Some(h),
            InlineNode::Image { hash, .. } => *hash = Some(h),
            InlineNode::LineBreak => {}
            InlineNode::SoftBreak => {}
            InlineNode::InlineMath { hash, .. } => *hash = Some(h),
            InlineNode::QuotedText { hash, .. } => *hash = Some(h),
            InlineNode::Strikethrough { hash, .. } => *hash = Some(h),
            InlineNode::HighlightTag { .. } => {}
            InlineNode::AlertTag { .. } => {}
            InlineNode::RawHtmlInline { hash, .. } => *hash = Some(h),
        }
    }

    pub fn compute_hashes_recursively(&mut self) {
        match self {
            InlineNode::Strong { children, .. }
            | InlineNode::Emphasis { children, .. }
            | InlineNode::Link { children, .. }
            | InlineNode::QuotedText { children, .. }
            | InlineNode::Strikethrough { children, .. } => {
                for c in children {
                    c.compute_hashes_recursively();
                }
            }
            _ => {}
        }
        let h = self.compute_hash();
        self.set_hash(h);
    }
}

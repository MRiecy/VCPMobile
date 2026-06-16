use serde::{Deserialize, Serialize};

/// 块级元素
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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
}

/// 行内元素
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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

    #[serde(rename = "break")]
    Break,

    #[serde(rename = "inline_math")]
    InlineMath {
        content: String,
        display_mode: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        hash: Option<u64>,
    },

    // VCP 魔法标记
    #[serde(rename = "vcp_custom")]
    VcpCustom {
        kind: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        value: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        children: Option<Vec<InlineNode>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        hash: Option<u64>,
    },

    #[serde(rename = "strikethrough")]
    Strikethrough {
        children: Vec<InlineNode>,
        #[serde(skip_serializing_if = "Option::is_none")]
        hash: Option<u64>,
    },

    #[serde(rename = "raw_html_inline")]
    RawHtmlInline {
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        hash: Option<u64>,
    },
}

impl MarkdownNode {
    pub fn get_hash(&self) -> Option<u64> {
        match self {
            MarkdownNode::Paragraph { hash, .. } => *hash,
            MarkdownNode::Heading { hash, .. } => *hash,
            MarkdownNode::CodeBlock { hash, .. } => *hash,
            MarkdownNode::Blockquote { hash, .. } => *hash,
            MarkdownNode::List { hash, .. } => *hash,
            MarkdownNode::Table { hash, .. } => *hash,
            MarkdownNode::ThematicBreak => None,
            MarkdownNode::RawHtml { hash, .. } => *hash,
        }
    }

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

    pub fn compute_hash(&self) -> u64 {
        let mut hasher = rustc_hash::FxHasher::default();
        std::hash::Hash::hash(self, &mut hasher);
        std::hash::Hasher::finish(&hasher)
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
    pub fn get_hash(&self) -> Option<u64> {
        match self {
            InlineNode::Text { .. } => None,
            InlineNode::Strong { hash, .. } => *hash,
            InlineNode::Emphasis { hash, .. } => *hash,
            InlineNode::Code { .. } => None,
            InlineNode::Link { hash, .. } => *hash,
            InlineNode::Image { hash, .. } => *hash,
            InlineNode::Break => None,
            InlineNode::InlineMath { hash, .. } => *hash,
            InlineNode::VcpCustom { hash, .. } => *hash,
            InlineNode::Strikethrough { hash, .. } => *hash,
            InlineNode::RawHtmlInline { hash, .. } => *hash,
        }
    }

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

    pub fn inline_math(content: String, display_mode: bool) -> Self {
        Self::InlineMath {
            content,
            display_mode,
            hash: None,
        }
    }

    pub fn vcp_custom(
        kind: String,
        value: Option<String>,
        children: Option<Vec<InlineNode>>,
    ) -> Self {
        Self::VcpCustom {
            kind,
            value,
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

    pub fn r#break() -> Self {
        Self::Break
    }

    pub fn raw_html_inline(content: String) -> Self {
        Self::RawHtmlInline {
            content,
            hash: None,
        }
    }

    pub fn compute_hash(&self) -> u64 {
        let mut hasher = rustc_hash::FxHasher::default();
        std::hash::Hash::hash(self, &mut hasher);
        std::hash::Hasher::finish(&hasher)
    }

    pub fn set_hash(&mut self, h: u64) {
        match self {
            InlineNode::Text { .. } => {}
            InlineNode::Strong { hash, .. } => *hash = Some(h),
            InlineNode::Emphasis { hash, .. } => *hash = Some(h),
            InlineNode::Code { .. } => {}
            InlineNode::Link { hash, .. } => *hash = Some(h),
            InlineNode::Image { hash, .. } => *hash = Some(h),
            InlineNode::Break => {}
            InlineNode::InlineMath { hash, .. } => *hash = Some(h),
            InlineNode::VcpCustom { hash, .. } => *hash = Some(h),
            InlineNode::Strikethrough { hash, .. } => *hash = Some(h),
            InlineNode::RawHtmlInline { hash, .. } => *hash = Some(h),
        }
    }

    pub fn compute_hashes_recursively(&mut self) {
        match self {
            InlineNode::Strong { children, .. }
            | InlineNode::Emphasis { children, .. }
            | InlineNode::Link { children, .. }
            | InlineNode::Strikethrough { children, .. } => {
                for c in children {
                    c.compute_hashes_recursively();
                }
            }
            InlineNode::VcpCustom {
                children: Some(ch), ..
            } => {
                for c in ch {
                    c.compute_hashes_recursively();
                }
            }
            _ => {}
        }
        let h = self.compute_hash();
        self.set_hash(h);
    }
}

impl std::hash::Hash for MarkdownNode {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match self {
            MarkdownNode::Paragraph { children, hash } => {
                state.write_u8(0);
                if let Some(h) = hash {
                    state.write_u64(*h);
                } else {
                    for c in children {
                        c.hash(state);
                    }
                }
            }
            MarkdownNode::Heading {
                level,
                children,
                hash,
            } => {
                state.write_u8(1);
                state.write_u8(*level);
                if let Some(h) = hash {
                    state.write_u64(*h);
                } else {
                    for c in children {
                        c.hash(state);
                    }
                }
            }
            MarkdownNode::CodeBlock {
                lang,
                code,
                highlighted_html,
                theme,
                hash: _,
            } => {
                state.write_u8(2);
                lang.hash(state);
                code.hash(state);
                highlighted_html.hash(state);
                theme.hash(state);
            }
            MarkdownNode::Blockquote { children, hash } => {
                state.write_u8(3);
                if let Some(h) = hash {
                    state.write_u64(*h);
                } else {
                    for n in children {
                        n.hash(state);
                    }
                }
            }
            MarkdownNode::List {
                ordered,
                items,
                hash,
            } => {
                state.write_u8(4);
                ordered.hash(state);
                if let Some(h) = hash {
                    state.write_u64(*h);
                } else {
                    for item in items {
                        for n in item {
                            n.hash(state);
                        }
                    }
                }
            }
            MarkdownNode::Table {
                header,
                rows,
                wrapper_class,
                hash,
            } => {
                state.write_u8(5);
                wrapper_class.hash(state);
                if let Some(h) = hash {
                    state.write_u64(*h);
                } else {
                    for cell in header {
                        for n in cell {
                            n.hash(state);
                        }
                    }
                    for row in rows {
                        for cell in row {
                            for n in cell {
                                n.hash(state);
                            }
                        }
                    }
                }
            }
            MarkdownNode::ThematicBreak => {
                state.write_u8(6);
            }
            MarkdownNode::RawHtml { content, hash: _ } => {
                state.write_u8(7);
                content.hash(state);
            }
        }
    }
}

impl std::hash::Hash for InlineNode {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match self {
            InlineNode::Text { value } => {
                state.write_u8(0);
                value.hash(state);
            }
            InlineNode::Strong { children, hash } => {
                state.write_u8(1);
                if let Some(h) = hash {
                    state.write_u64(*h);
                } else {
                    for c in children {
                        c.hash(state);
                    }
                }
            }
            InlineNode::Emphasis { children, hash } => {
                state.write_u8(2);
                if let Some(h) = hash {
                    state.write_u64(*h);
                } else {
                    for c in children {
                        c.hash(state);
                    }
                }
            }
            InlineNode::Code { value } => {
                state.write_u8(3);
                value.hash(state);
            }
            InlineNode::Link {
                href,
                title,
                children,
                needs_asset_conversion,
                hash,
            } => {
                state.write_u8(4);
                href.hash(state);
                title.hash(state);
                needs_asset_conversion.hash(state);
                if let Some(h) = hash {
                    state.write_u64(*h);
                } else {
                    for c in children {
                        c.hash(state);
                    }
                }
            }
            InlineNode::Image {
                src,
                alt,
                title,
                needs_asset_conversion,
                hash: _,
            } => {
                state.write_u8(5);
                src.hash(state);
                alt.hash(state);
                title.hash(state);
                needs_asset_conversion.hash(state);
            }
            InlineNode::Break => {
                state.write_u8(6);
            }
            InlineNode::InlineMath {
                content,
                display_mode,
                hash: _,
            } => {
                state.write_u8(8);
                content.hash(state);
                display_mode.hash(state);
            }
            InlineNode::VcpCustom {
                kind,
                value,
                children,
                hash,
            } => {
                state.write_u8(14);
                kind.hash(state);
                value.hash(state);
                if let Some(h) = hash {
                    state.write_u64(*h);
                } else if let Some(ch) = children {
                    for c in ch {
                        c.hash(state);
                    }
                }
            }
            InlineNode::Strikethrough { children, hash } => {
                state.write_u8(10);
                if let Some(h) = hash {
                    state.write_u64(*h);
                } else {
                    for c in children {
                        c.hash(state);
                    }
                }
            }

            InlineNode::RawHtmlInline { content, hash: _ } => {
                state.write_u8(13);
                content.hash(state);
            }
        }
    }
}

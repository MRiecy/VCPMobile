use serde::{Deserialize, Serialize};

/// 块级元素
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum MarkdownNode {
    #[serde(rename = "paragraph")]
    Paragraph {
        children: Vec<InlineNode>,
        #[serde(skip_serializing_if = "Option::is_none")]
        hash: Option<String>,
    },

    #[serde(rename = "heading")]
    Heading {
        level: u8,
        children: Vec<InlineNode>,
        #[serde(skip_serializing_if = "Option::is_none")]
        hash: Option<String>,
    },

    #[serde(rename = "code_block")]
    CodeBlock {
        lang: Option<String>,
        code: String,
        highlighted_html: Option<String>, // syntect 预渲染结果
        theme: Option<String>,            // "github-dark" | "github-light"
        #[serde(skip_serializing_if = "Option::is_none")]
        hash: Option<String>,
    },

    #[serde(rename = "blockquote")]
    Blockquote {
        children: Vec<MarkdownNode>,
        #[serde(skip_serializing_if = "Option::is_none")]
        hash: Option<String>,
    },

    #[serde(rename = "list")]
    List {
        ordered: bool,
        items: Vec<Vec<MarkdownNode>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        hash: Option<String>,
    },

    #[serde(rename = "table")]
    Table {
        header: Vec<Vec<InlineNode>>,
        rows: Vec<Vec<Vec<InlineNode>>>,
        wrapper_class: Option<String>, // "vcp-scrollable no-swipe"
        #[serde(skip_serializing_if = "Option::is_none")]
        hash: Option<String>,
    },

    #[serde(rename = "thematic_break")]
    ThematicBreak,

    #[serde(rename = "raw_html")]
    RawHtml {
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        hash: Option<String>,
    },

    #[serde(rename = "mermaid")]
    MermaidPlaceholder {
        code: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        hash: Option<String>,
    },
}

/// 行内元素
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum InlineNode {
    #[serde(rename = "text")]
    Text { value: String },

    #[serde(rename = "strong")]
    Strong {
        children: Vec<InlineNode>,
        #[serde(skip_serializing_if = "Option::is_none")]
        hash: Option<String>,
    },

    #[serde(rename = "emphasis")]
    Emphasis {
        children: Vec<InlineNode>,
        #[serde(skip_serializing_if = "Option::is_none")]
        hash: Option<String>,
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
        hash: Option<String>,
    },

    #[serde(rename = "image")]
    Image {
        src: String,
        alt: String,
        title: Option<String>,
        needs_asset_conversion: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        hash: Option<String>,
    },

    #[serde(rename = "line_break")]
    LineBreak,

    #[serde(rename = "soft_break")]
    SoftBreak,

    #[serde(rename = "inline_math")]
    InlineMath {
        content: String,
        svg: Option<String>,
        display_mode: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        hash: Option<String>,
    },

    // VCP 魔法标记
    #[serde(rename = "quoted_text")]
    QuotedText {
        children: Vec<InlineNode>,
        #[serde(skip_serializing_if = "Option::is_none")]
        hash: Option<String>,
    },

    #[serde(rename = "strikethrough")]
    Strikethrough {
        children: Vec<InlineNode>,
        #[serde(skip_serializing_if = "Option::is_none")]
        hash: Option<String>,
    },

    #[serde(rename = "highlight_tag")]
    HighlightTag { value: String }, // #标签

    #[serde(rename = "alert_tag")]
    AlertTag { value: String }, // !告警

    #[serde(rename = "raw_html_inline")]
    RawHtmlInline {
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        hash: Option<String>,
    },
}

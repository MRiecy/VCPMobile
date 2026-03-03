use scraper::{Html, Node};
use ego_tree::NodeRef;
use lru::LruCache;
use std::sync::Mutex;
use std::num::NonZeroUsize;
use fancy_regex::Regex;
use lazy_static::lazy_static;

lazy_static! {
    static ref THOUGHT_CHAIN_REGEX: Regex = Regex::new(r#"(?s)\[--- VCP元思考链(?::\s*"([^"]*)")?\s*---\].*?\[--- 元思考链结束 ---\]"#).unwrap();
    static ref CONVENTIONAL_THOUGHT_REGEX: Regex = Regex::new(r"(?is)<think>.*?</think>").unwrap();
    static ref HTML_CHECK_REGEX: Regex = Regex::new(r"<[^>]+>").unwrap();
}

pub struct SanitizerState {
    pub cache: Mutex<LruCache<String, String>>,
}

impl SanitizerState {
    pub fn new(capacity: usize) -> Self {
        Self {
            cache: Mutex::new(LruCache::new(NonZeroUsize::new(capacity).unwrap())),
        }
    }
}

/// 清理元思考链（明文形式）
pub fn strip_thought_chains(content: &str) -> String {
    let s = THOUGHT_CHAIN_REGEX.replace_all(content, "").to_string();
    CONVENTIONAL_THOUGHT_REGEX.replace_all(&s, "").to_string()
}

/// 核心逻辑：HTML -> Markdown
pub fn html_to_vcp_markdown(html: &str, keep_thoughts: bool) -> String {
    let fragment = Html::parse_fragment(html);
    let mut result = String::new();
    
    // root() returns the root node of the tree
    for node in fragment.tree.root().children() {
        process_node(node, &mut result, keep_thoughts);
    }
    
    // 清理多余空行，对齐 JS 逻辑
    if let Ok(re) = regex::Regex::new(r"\n{3,}") {
        re.replace_all(result.trim(), "\n\n").to_string()
    } else {
        result.trim().to_string()
    }
}

fn process_node(node: NodeRef<Node>, out: &mut String, keep_thoughts: bool) {
    match node.value() {
        Node::Text(text) => {
            out.push_str(&text.text);
        }
        Node::Element(el) => {
            let tag = el.name();
            
            // 1. 检查 data-raw-content (原味保护)
            if let Some(raw) = el.attr("data-raw-content") {
                out.push_str(raw);
                return;
            }

            // 2. 特殊标签处理
            match tag {
                "img" => {
                    let src = el.attr("src").unwrap_or("");
                    let alt = el.attr("alt").unwrap_or("");
                    if !src.is_empty() {
                        out.push_str(&format!(r#"<img src="{}" alt="{}">"#, src, alt));
                    }
                }
                "audio" | "video" => {
                    let src = el.attr("src").unwrap_or("");
                    if !src.is_empty() {
                        out.push_str(&format!(r#"<{0} src="{1}"></{0}>"#, tag, src));
                    } else {
                        // 尝试查找子节点的 source
                        let mut first_src = "";
                        for child in node.children() {
                            if let Node::Element(cel) = child.value() {
                                if cel.name() == "source" {
                                    if let Some(csrc) = cel.attr("src") {
                                        first_src = csrc;
                                        break;
                                    }
                                }
                            }
                        }
                        if !first_src.is_empty() {
                            out.push_str(&format!(r#"<{0} src="{1}"></{0}>"#, tag, first_src));
                        }
                    }
                }
                "pre" => {
                    // 检查是否包含特殊标记 (vcpRawBlocks 逻辑)
                    let mut text_content = String::new();
                    collect_text(node, &mut text_content);
                    
                    if text_content.contains("<<<[TOOL_REQUEST]>>>") || text_content.contains("<<<DailyNoteStart>>>") {
                        out.push_str(&text_content);
                    } else {
                        // 普通 pre 转为代码块
                        out.push_str("\n```\n");
                        out.push_str(&text_content);
                        out.push_str("\n```\n");
                    }
                }
                "div" if el.has_class("vcp-thought-chain-bubble", scraper::CaseSensitivity::AsciiCaseInsensitive) => {
                    if keep_thoughts {
                        let title = el.attr("data-thought-title").unwrap_or("");
                        let title_part = if !title.is_empty() { format!(r#": "{}""#, title) } else { String::new() };
                        out.push_str(&format!("\n\n[--- VCP元思考链{} ---]\n", title_part));
                        for child in node.children() {
                            process_node(child, out, keep_thoughts);
                        }
                        out.push_str("\n[--- 元思考链结束 ---]\n\n");
                    }
                    // 不保留则直接跳过整个节点
                }
                // 常见标签转 MD
                "p" => {
                    out.push('\n');
                    for child in node.children() {
                        process_node(child, out, keep_thoughts);
                    }
                    out.push('\n');
                }
                "br" => out.push('\n'),
                "strong" | "b" => {
                    out.push_str("**");
                    for child in node.children() {
                        process_node(child, out, keep_thoughts);
                    }
                    out.push_str("**");
                }
                "em" | "i" => {
                    out.push('*');
                    for child in node.children() {
                        process_node(child, out, keep_thoughts);
                    }
                    out.push('*');
                }
                "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                    let level = tag.chars().last().unwrap().to_digit(10).unwrap_or(1);
                    out.push('\n');
                    for _ in 0..level { out.push('#'); }
                    out.push(' ');
                    for child in node.children() {
                        process_node(child, out, keep_thoughts);
                    }
                    out.push('\n');
                }
                "code" => {
                    out.push('`');
                    for child in node.children() {
                        process_node(child, out, keep_thoughts);
                    }
                    out.push('`');
                }
                "ul" => {
                    out.push('\n');
                    for child in node.children() {
                        process_node(child, out, keep_thoughts);
                    }
                    out.push('\n');
                }
                "li" => {
                    out.push_str("- ");
                    for child in node.children() {
                        process_node(child, out, keep_thoughts);
                    }
                    out.push('\n');
                }
                "a" => {
                    let href = el.attr("href").unwrap_or("");
                    out.push('[');
                    for child in node.children() {
                        process_node(child, out, keep_thoughts);
                    }
                    out.push_str(&format!("]({})", href));
                }
                _ => {
                    // 默认递归处理子节点
                    for child in node.children() {
                        process_node(child, out, keep_thoughts);
                    }
                }
            }
        }
        _ => {}
    }
}

fn collect_text(node: NodeRef<Node>, out: &mut String) {
    for child in node.children() {
        match child.value() {
            Node::Text(text) => out.push_str(&text.text),
            Node::Element(_) => collect_text(child, out),
            _ => {}
        }
    }
}

/// 检查内容是否包含 HTML 标签
pub fn contains_html(content: &str) -> bool {
    HTML_CHECK_REGEX.is_match(content).unwrap_or(false)
}

/// 生成缓存键
pub fn generate_cache_key(content: &str, keep_thoughts: bool) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    
    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    keep_thoughts.hash(&mut hasher);
    let hash = hasher.finish();
    format!("sanitized_{}_{}", hash, content.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_thoughts() {
        let input = "Hello [--- VCP元思考链: \"test\" ---] secret [--- 元思考链结束 ---] World <think>internal</think>";
        assert_eq!(strip_thought_chains(input), "Hello  World ");
    }

    #[test]
    fn test_html_to_md_img() {
        let html = r#"<p>Hello <img src="test.png" alt="alt text"> World</p>"#;
        let md = html_to_vcp_markdown(html, false);
        assert!(md.contains(r#"<img src="test.png" alt="alt text">"#));
    }

    #[test]
    fn test_raw_content() {
        let html = r#"<pre data-raw-content="<<<[TOOL_REQUEST]>>>\ncall()"></pre>"#;
        let md = html_to_vcp_markdown(html, false);
        assert_eq!(md, "<<<[TOOL_REQUEST]>>>\ncall()");
    }
}

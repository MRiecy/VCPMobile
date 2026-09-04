//! vcp_modules/infra/html2md/vcp_handlers.rs
//! VCP 自定义元素处理器：对齐桌面端 contextSanitizer.js 的 turndown 规则集
//!
//! 规则映射（桌面 -> 本模块）：
//! - 规则1 preserveImages        -> [`img_handler`]
//! - 规则2 preserveMedia          -> [`media_handler`]
//! - 规则3/4 vcpPrettifiedBlocks / vcpRawBlocks -> [`pre_handler`]
//! - 规则5 vcpThoughtChains       -> [`div_handler`]
//!
//! 未命中的元素一律经 `handlers.fallback(element)` 交还 htmd 内建 handler，
//! 保持通用 HTML -> Markdown 语义不变。

use htmd::element_handler::{HandlerResult, Handlers};
use htmd::Element;
use markup5ever_rcdom::{Node, NodeData};
use std::rc::Rc;

/// 读取元素属性值（html5ever 解析后属性名已小写化）
fn get_attr<'a>(attrs: &'a [html5ever::Attribute], name: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|attr| attr.name.local.as_ref() == name)
        .map(|attr| attr.value.as_ref())
}

/// class 名单匹配：大小写敏感，对齐 JS classList.contains 语义
/// （旧 Rust 实现的 AsciiCaseInsensitive 是与桌面端的偏差，已纠正）
fn has_class(attrs: &[html5ever::Attribute], class: &str) -> bool {
    get_attr(attrs, "class")
        .is_some_and(|value| value.split_ascii_whitespace().any(|item| item == class))
}

/// 收集节点下全部纯文本内容（等价 DOM textContent）
fn collect_text(node: &Rc<Node>, out: &mut String) {
    for child in node.children.borrow().iter() {
        match &child.data {
            NodeData::Text { contents } => out.push_str(contents.borrow().as_ref()),
            NodeData::Element { .. } => collect_text(child, out),
            _ => {}
        }
    }
}

/// 规则1：保留图片。有 src -> 保留 HTML 标签原样；无 src -> 丢弃
pub(super) fn img_handler(_handlers: &dyn Handlers, element: Element) -> Option<HandlerResult> {
    let src = get_attr(element.attrs, "src").unwrap_or("");
    if src.is_empty() {
        return None;
    }
    let alt = get_attr(element.attrs, "alt").unwrap_or("");
    Some(format!(r#"<img src="{src}" alt="{alt}">"#).into())
}

/// 规则2：保留音视频。优先自身 src；否则取第一个 <source> 子元素的 src；都没有 -> 丢弃
pub(super) fn media_handler(_handlers: &dyn Handlers, element: Element) -> Option<HandlerResult> {
    let tag = element.tag;
    if let Some(src) = get_attr(element.attrs, "src").filter(|src| !src.is_empty()) {
        return Some(format!(r#"<{tag} src="{src}"></{tag}>"#).into());
    }

    let children = element.node.children.borrow();
    for child in children.iter() {
        if let NodeData::Element { name, attrs, .. } = &child.data {
            if name.local.as_ref() == "source" {
                // 与桌面一致：只看第一个 <source>，其 src 缺失/为空即视为无源
                return get_attr(&attrs.borrow(), "src")
                    .filter(|src| !src.is_empty())
                    .map(|src| format!(r#"<{tag} src="{src}"></{tag}>"#).into());
            }
        }
    }
    None
}

/// 规则3+4：VCP 特殊 pre 块的「零损提取」
///
/// 检查顺序（与桌面规则3/4 效果等价，沿用旧 Rust 版的简化顺序）：
/// 1. 美化泡泡（vcp-tool-use-bubble / maid-diary-bubble）：返回 data-raw-content 原文；
///    缺失该属性时返回空，避免美化 HTML 污染上下文（桌面规则3 行为）
/// 2. 任意带 data-raw-content 的 pre：原文直通（协议上 data-raw-content 载体只有 pre）
/// 3. 文本含 `<<<[TOOL_REQUEST]>>>` / `<<<DailyNoteStart>>>` 的未美化块：返回 textContent 原文
/// 4. 其余 pre：fallback 交还内建 handler（围栏代码块）
pub(super) fn pre_handler(handlers: &dyn Handlers, element: Element) -> Option<HandlerResult> {
    let is_prettified_bubble = has_class(element.attrs, "vcp-tool-use-bubble")
        || has_class(element.attrs, "maid-diary-bubble");
    if is_prettified_bubble {
        return match get_attr(element.attrs, "data-raw-content") {
            Some(raw) if !raw.is_empty() => Some(raw.to_string().into()),
            _ => {
                log::warn!("[html2md] VCP 美化块缺失 data-raw-content，已丢弃");
                None
            }
        };
    }

    if let Some(raw) = get_attr(element.attrs, "data-raw-content") {
        if !raw.is_empty() {
            return Some(raw.to_string().into());
        }
    }

    let mut text_content = String::new();
    collect_text(element.node, &mut text_content);
    if text_content.contains("<<<[TOOL_REQUEST]>>>")
        || text_content.contains("<<<DailyNoteStart>>>")
    {
        return Some(text_content.into());
    }

    handlers.fallback(element)
}

/// 规则5：VCP 元思考链泡泡（div.vcp-thought-chain-bubble）
///
/// - `keep_thoughts = true`：包成 `[--- VCP元思考链 ---]` 协议块，正文为子树的 Markdown 转换结果
/// - `keep_thoughts = false`：整块丢弃
///
/// 非思维链 div 一律 fallback 交还内建块级 handler
pub(super) fn div_handler(
    handlers: &dyn Handlers,
    element: Element,
    keep_thoughts: bool,
) -> Option<HandlerResult> {
    if !has_class(element.attrs, "vcp-thought-chain-bubble") {
        return handlers.fallback(element);
    }
    if !keep_thoughts {
        return None;
    }

    let title = get_attr(element.attrs, "data-thought-title").unwrap_or("");
    let title_part = if title.is_empty() {
        String::new()
    } else {
        format!(r#": "{title}""#)
    };
    let content = handlers.walk_children(element.node).content;
    Some(
        format!("\n\n[--- VCP元思考链{title_part} ---]\n{content}\n[--- 元思考链结束 ---]\n\n")
            .into(),
    )
}

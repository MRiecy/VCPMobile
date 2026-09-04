//! vcp_modules/infra/html2md/mod.rs
//! 可复用 HTML -> Markdown 转换引擎，基于 htmd（turndown 的 Rust 移植）+ VCP 自定义 handler
//!
//! 架构：htmd 内建 handler 负责通用标签语义（标题/列表/表格/围栏代码块等），
//! VCP 业务规则（图片/音视频保留、VCP 原始块零损提取、元思考链泡泡）以自定义
//! handler 形式注册在其后、优先命中；未命中的元素经 `fallback` 交还内建 handler。
//!
//! Options 对齐桌面端 turndown 配置：Atx 标题、`---` 分割线、`-` 列表标记、
//! 围栏代码块（其余保持 htmd 默认，Pure 模式下未知标签剥标签留内容，
//! 与旧实现「默认透传」语义一致）。
//!
//! 引擎为进程级静态实例（keep_thoughts 两态各一），handler 集合构建期冻结，
//! 转换调用本身不可变共享，可安全并发。

mod vcp_handlers;

use htmd::options::{BulletListMarker, HrStyle, Options};
use htmd::HtmlToMarkdown;
use std::sync::OnceLock;

/// 引擎实例：保留思维链（keep_thoughts = true）
static ENGINE_KEEP_THOUGHTS: OnceLock<HtmlToMarkdown> = OnceLock::new();
/// 引擎实例：丢弃思维链（keep_thoughts = false）
static ENGINE_STRIP_THOUGHTS: OnceLock<HtmlToMarkdown> = OnceLock::new();

/// 将 HTML 内容转换为 Markdown
/// @param html 输入的 HTML 字符串
/// @param keep_thoughts 是否保留 VCP 元思考链泡泡（true 保留 / false 丢弃）
pub fn convert(html: &str, keep_thoughts: bool) -> Result<String, String> {
    let engine = if keep_thoughts {
        ENGINE_KEEP_THOUGHTS.get_or_init(|| build_engine(true))
    } else {
        ENGINE_STRIP_THOUGHTS.get_or_init(|| build_engine(false))
    };
    engine
        .convert(html)
        .map_err(|error| format!("[html2md] HTML -> Markdown 转换失败: {error}"))
}

/// 装配一台引擎：桌面对齐的 Options + VCP handler 集
fn build_engine(keep_thoughts: bool) -> HtmlToMarkdown {
    let options = Options {
        // 对齐桌面 turndown 配置 hr: '---'
        hr_style: HrStyle::Dashes,
        // 对齐桌面 turndown 配置 bulletListMarker: '-'
        bullet_list_marker: BulletListMarker::Dash,
        ..Options::default()
    };
    HtmlToMarkdown::builder()
        .options(options)
        .add_handler(vec!["img"], vcp_handlers::img_handler)
        .add_handler(vec!["audio", "video"], vcp_handlers::media_handler)
        .add_handler(vec!["pre"], vcp_handlers::pre_handler)
        .add_handler(
            vec!["div"],
            move |handlers: &dyn htmd::element_handler::Handlers, element: htmd::Element| {
                vcp_handlers::div_handler(handlers, element, keep_thoughts)
            },
        )
        .build()
}

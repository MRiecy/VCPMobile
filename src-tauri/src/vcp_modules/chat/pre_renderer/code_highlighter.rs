use lazy_static::lazy_static;
use syntect::highlighting::ThemeSet;
use syntect::html::{highlighted_html_for_string, ClassStyle, ClassedHTMLGenerator};
use syntect::parsing::SyntaxSet;

lazy_static! {
    static ref SYNTAX_SET: SyntaxSet = SyntaxSet::load_defaults_newlines();
    static ref THEME_SET: ThemeSet = ThemeSet::load_defaults();
}

/// 专属 HTML 全预览卡片的高性能 Classed Syntect 高亮器
/// 仅输出纯净的带语义类名的 DOM (DoubleMinus 模式，c--tag 等)，绝不硬编码任何 inline style！
pub fn highlight_html_block(code: &str) -> Option<String> {
    let syntax = SYNTAX_SET
        .find_syntax_by_token("html")
        .or_else(|| SYNTAX_SET.find_syntax_by_token("HTML"))
        .unwrap_or_else(|| SYNTAX_SET.find_syntax_plain_text());

    let mut html_generator =
        ClassedHTMLGenerator::new_with_class_style(syntax, &SYNTAX_SET, ClassStyle::Spaced);

    for line in code.split('\n') {
        let mut line_with_nl = line.to_string();
        line_with_nl.push('\n');
        let _ = html_generator.parse_html_for_line_which_includes_newline(&line_with_nl);
    }

    let html = html_generator.finalize();

    Some(format!(
        "<pre class=\"vcp-code-block vcp-html-block vcp-scrollable\"><code>{}</code></pre>",
        html
    ))
}

pub fn highlight_code_block(code: &str, lang: &str) -> Option<String> {
    let lang_lower = lang.to_lowercase();
    let syntax = SYNTAX_SET
        .find_syntax_by_token(&lang_lower)
        .or_else(|| SYNTAX_SET.find_syntax_by_token(lang))
        .or_else(|| SYNTAX_SET.find_syntax_by_extension(lang))
        .unwrap_or_else(|| {
            // 当找不到指定语言时，统一回退到 JavaScript 作为通用的类 C 语法高亮
            SYNTAX_SET
                .find_syntax_by_token("JavaScript")
                .unwrap_or_else(|| SYNTAX_SET.find_syntax_plain_text())
        });

    let theme = &THEME_SET.themes["base16-ocean.dark"];

    let html = highlighted_html_for_string(code, &SYNTAX_SET, syntax, theme).ok()?;

    // 统一添加 vcp-code-block 和 vcp-scrollable 类，并确保单层 pre 结构
    let fixed = if html.starts_with("<pre") {
        html.replacen("<pre", "<pre class=\"vcp-code-block vcp-scrollable\"", 1)
    } else {
        format!(
            "<pre class=\"vcp-code-block vcp-scrollable\">{}</pre>",
            html
        )
    };

    Some(fixed)
}

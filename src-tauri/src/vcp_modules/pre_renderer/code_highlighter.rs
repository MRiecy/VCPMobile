use lazy_static::lazy_static;
use syntect::highlighting::ThemeSet;
use syntect::html::highlighted_html_for_string;
use syntect::parsing::SyntaxSet;

lazy_static! {
    static ref SYNTAX_SET: SyntaxSet = SyntaxSet::load_defaults_newlines();
    static ref THEME_SET: ThemeSet = ThemeSet::load_defaults();
}

pub fn highlight_code_block(code: &str, lang: &str) -> Option<String> {
    let syntax = SYNTAX_SET
        .find_syntax_by_token(lang)
        .or_else(|| SYNTAX_SET.find_syntax_by_extension(lang))
        .unwrap_or_else(|| SYNTAX_SET.find_syntax_plain_text());

    let theme = &THEME_SET.themes["base16-ocean.dark"];

    let html = highlighted_html_for_string(code, &SYNTAX_SET, syntax, theme).ok()?;
    // 将 syntect 的 <pre> 替换为带 class 的 <pre>，单层结构，彻底消除嵌套 <pre> 的可能
    let fixed = if html.starts_with("<pre") {
        html.replacen("<pre", "<pre class=\"vcp-scrollable\"", 1)
    } else {
        html
    };
    Some(fixed)
}

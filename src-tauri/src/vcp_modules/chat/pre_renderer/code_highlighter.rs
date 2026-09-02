use lazy_static::lazy_static;
use std::collections::HashMap;
use syntect::easy::HighlightLines;
use syntect::highlighting::{Color, HighlightState, Theme, ThemeSet};
use syntect::html::{
    append_highlighted_html_for_styled_line, highlighted_html_for_string, ClassStyle,
    ClassedHTMLGenerator, IncludeBackground,
};
use syntect::parsing::{ParseState, SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;

lazy_static! {
    static ref SYNTAX_SET: SyntaxSet = SyntaxSet::load_defaults_newlines();
    static ref THEME_SET: ThemeSet = ThemeSet::load_defaults();
}

const STREAM_CODE_ROOT_ATTR: &str = "data-vcp-stream-code";
const STREAM_CODE_STABLE_ATTR: &str = "data-vcp-code-stable";
const STREAM_CODE_ACTIVE_ATTR: &str = "data-vcp-code-active";

pub struct IncrementalCodePatch {
    pub completed_html: String,
    pub active_html: String,
}

struct CodeHighlightSession {
    language: String,
    committed_len: usize,
    highlight_state: HighlightState,
    parse_state: ParseState,
    seen_generation: u64,
}

/// Aurora 活跃代码块的增量 Syntect 状态。
///
/// 每个节点只保留「最后一个完整换行之后」的解析/高亮状态和一个字节游标；代码原文仍由
/// `prev_tail_ast` 单独持有，已生成的 HTML 只存在于前端 DOM，避免后端再保存一份等长镜像。
#[derive(Default)]
pub struct IncrementalCodeHighlighter {
    sessions: HashMap<String, CodeHighlightSession>,
    generation: u64,
}

impl IncrementalCodeHighlighter {
    pub fn begin_frame(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        if self.generation == 0 {
            self.sessions.clear();
            self.generation = 1;
        }
    }

    pub fn finish_frame(&mut self) {
        let generation = self.generation;
        self.sessions
            .retain(|_, session| session.seen_generation == generation);
    }

    pub fn clear(&mut self) {
        self.sessions.clear();
    }

    pub fn mark_seen(&mut self, id: &str) {
        if let Some(session) = self.sessions.get_mut(id) {
            session.seen_generation = self.generation;
        }
    }

    /// 创建或重建一个代码节点，并返回带稳定区/活跃行锚点的完整 `<pre>`。
    pub fn start(&mut self, id: &str, code: &str, lang: &str) -> Option<String> {
        self.sessions.remove(id);
        let (session, completed_html, active_html) =
            build_session(code, lang, true, self.generation)?;
        self.sessions.insert(id.to_string(), session);
        Some(wrap_incremental_code_html(&completed_html, &active_html))
    }

    /// 仅将状态推进到最后一个完整换行，用于 epoch/reset 后为下一帧建立 checkpoint。
    pub fn prime(&mut self, id: &str, code: &str, lang: &str) -> bool {
        let Some((session, _, _)) = build_session(code, lang, false, self.generation) else {
            self.sessions.remove(id);
            return false;
        };
        self.sessions.insert(id.to_string(), session);
        true
    }

    /// 对严格追加的代码生成「新完成行追加 + 当前未完成行替换」补丁。
    /// 前缀、语言或 session 任一不一致时返回 `None`，调用方应执行一次完整 Replace 重建。
    pub fn append(
        &mut self,
        id: &str,
        old_code: &str,
        new_code: &str,
        lang: &str,
    ) -> Option<IncrementalCodePatch> {
        new_code.strip_prefix(old_code)?;

        let (committed_len, highlight_state, parse_state) = {
            let session = self.sessions.get(id)?;
            if !session.language.eq_ignore_ascii_case(lang)
                || session.committed_len > old_code.len()
            {
                return None;
            }
            (
                session.committed_len,
                session.highlight_state.clone(),
                session.parse_state.clone(),
            )
        };

        let pending = new_code.get(committed_len..)?;
        let completed_len = complete_line_prefix_len(pending);
        let completed = &pending[..completed_len];
        let active = &pending[completed_len..];
        let theme = default_theme()?;

        let (next_highlight_state, next_parse_state, completed_html) =
            advance_complete_lines(highlight_state, parse_state, completed, theme, true)?;
        let active_html = render_active_line(
            next_highlight_state.clone(),
            next_parse_state.clone(),
            active,
            theme,
        )?;

        let session = self.sessions.get_mut(id)?;
        session.committed_len = committed_len.saturating_add(completed_len);
        session.highlight_state = next_highlight_state;
        session.parse_state = next_parse_state;
        session.seen_generation = self.generation;

        Some(IncrementalCodePatch {
            completed_html,
            active_html,
        })
    }
}

fn normalize_language(lang: &str) -> String {
    lang.to_ascii_lowercase()
}

fn syntax_for_language(lang: &str) -> &SyntaxReference {
    let lang_lower = normalize_language(lang);
    SYNTAX_SET
        .find_syntax_by_token(&lang_lower)
        .or_else(|| SYNTAX_SET.find_syntax_by_token(lang))
        .or_else(|| SYNTAX_SET.find_syntax_by_extension(lang))
        .unwrap_or_else(|| {
            SYNTAX_SET
                .find_syntax_by_token("JavaScript")
                .unwrap_or_else(|| SYNTAX_SET.find_syntax_plain_text())
        })
}

fn default_theme() -> Option<&'static Theme> {
    THEME_SET
        .themes
        .get("base16-ocean.dark")
        .or_else(|| THEME_SET.themes.values().next())
}

fn complete_line_prefix_len(code: &str) -> usize {
    code.rfind('\n').map_or(0, |index| index + 1)
}

fn build_session(
    code: &str,
    lang: &str,
    render_html: bool,
    generation: u64,
) -> Option<(CodeHighlightSession, String, String)> {
    let syntax = syntax_for_language(lang);
    let theme = default_theme()?;
    let highlighter = HighlightLines::new(syntax, theme);
    let (highlight_state, parse_state) = highlighter.state();
    let committed_len = complete_line_prefix_len(code);
    let completed = &code[..committed_len];
    let active = &code[committed_len..];
    let (highlight_state, parse_state, completed_html) =
        advance_complete_lines(highlight_state, parse_state, completed, theme, render_html)?;
    let active_html = if render_html {
        render_active_line(highlight_state.clone(), parse_state.clone(), active, theme)?
    } else {
        String::new()
    };

    Some((
        CodeHighlightSession {
            language: normalize_language(lang),
            committed_len,
            highlight_state,
            parse_state,
            seen_generation: generation,
        },
        completed_html,
        active_html,
    ))
}

fn advance_complete_lines(
    highlight_state: HighlightState,
    parse_state: ParseState,
    code: &str,
    theme: &Theme,
    render_html: bool,
) -> Option<(HighlightState, ParseState, String)> {
    let mut highlighter = HighlightLines::from_state(theme, highlight_state, parse_state);
    let mut html = String::new();
    let background = theme.settings.background.unwrap_or(Color::WHITE);

    for line in LinesWithEndings::from(code) {
        let regions = highlighter.highlight_line(line, &SYNTAX_SET).ok()?;
        if render_html {
            append_highlighted_html_for_styled_line(
                &regions,
                IncludeBackground::IfDifferent(background),
                &mut html,
            )
            .ok()?;
        }
    }

    let (highlight_state, parse_state) = highlighter.state();
    Some((highlight_state, parse_state, html))
}

fn render_active_line(
    highlight_state: HighlightState,
    parse_state: ParseState,
    code: &str,
    theme: &Theme,
) -> Option<String> {
    if code.is_empty() {
        return Some(String::new());
    }

    let mut highlighter = HighlightLines::from_state(theme, highlight_state, parse_state);
    let regions = highlighter.highlight_line(code, &SYNTAX_SET).ok()?;
    let background = theme.settings.background.unwrap_or(Color::WHITE);
    let mut html = String::new();
    append_highlighted_html_for_styled_line(
        &regions,
        IncludeBackground::IfDifferent(background),
        &mut html,
    )
    .ok()?;
    Some(html)
}

fn wrap_incremental_code_html(completed_html: &str, active_html: &str) -> String {
    let background = default_theme()
        .and_then(|theme| theme.settings.background)
        .unwrap_or(Color::WHITE);
    format!(
        "<pre class=\"vcp-code-block vcp-scrollable\" style=\"background-color:#{:02x}{:02x}{:02x};\"><code {}><span {}>{}</span><span {}>{}</span></code></pre>",
        background.r,
        background.g,
        background.b,
        STREAM_CODE_ROOT_ATTR,
        STREAM_CODE_STABLE_ATTR,
        completed_html,
        STREAM_CODE_ACTIVE_ATTR,
        active_html,
    )
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
    let syntax = syntax_for_language(lang);
    let theme = default_theme()?;

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

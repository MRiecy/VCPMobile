use lazy_static::lazy_static;
use std::collections::HashMap;
use syntect::html::{ClassStyle, ClassedHTMLGenerator};
use syntect::parsing::{ParseState, Scope, ScopeStack, ScopeStackOp, SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;

lazy_static! {
    static ref SYNTAX_SET: SyntaxSet = SyntaxSet::load_defaults_newlines();
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
    parse_state: ParseState,
    scope_stack: ScopeStack,
    seen_generation: u64,
}

/// Aurora 活跃代码块的增量 Syntect 状态。
///
/// 每个节点只保留「最后一个完整换行之后」的解析状态（含 scope 栈）和一个字节游标；代码原文仍由
/// `prev_tail_ast` 单独持有，已生成的 HTML 只存在于前端 DOM，避免后端再保存一份等长镜像。
/// 输出为平铺的 scope 派生类 span（无内联样式、恒在行内闭合），token 配色由前端 CSS 按亮暗两态定义。
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

        let (committed_len, parse_state, scope_stack) = {
            let session = self.sessions.get(id)?;
            if !session.language.eq_ignore_ascii_case(lang)
                || session.committed_len > old_code.len()
            {
                return None;
            }
            (
                session.committed_len,
                session.parse_state.clone(),
                session.scope_stack.clone(),
            )
        };

        let pending = new_code.get(committed_len..)?;
        let completed_len = complete_line_prefix_len(pending);
        let completed = &pending[..completed_len];
        let active = &pending[completed_len..];

        let (next_parse_state, next_scope_stack, completed_html) =
            advance_complete_lines(parse_state, scope_stack, completed, true)?;
        let active_html =
            render_active_line(next_parse_state.clone(), next_scope_stack.clone(), active)?;

        let session = self.sessions.get_mut(id)?;
        session.committed_len = committed_len.saturating_add(completed_len);
        session.parse_state = next_parse_state;
        session.scope_stack = next_scope_stack;
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
    let parse_state = ParseState::new(syntax);
    let scope_stack = ScopeStack::new();
    let committed_len = complete_line_prefix_len(code);
    let completed = &code[..committed_len];
    let active = &code[committed_len..];
    let (parse_state, scope_stack, completed_html) =
        advance_complete_lines(parse_state, scope_stack, completed, render_html)?;
    let active_html = if render_html {
        render_active_line(parse_state.clone(), scope_stack.clone(), active)?
    } else {
        String::new()
    };

    Some((
        CodeHighlightSession {
            language: normalize_language(lang),
            committed_len,
            parse_state,
            scope_stack,
            seen_generation: generation,
        },
        completed_html,
        active_html,
    ))
}

fn advance_complete_lines(
    mut parse_state: ParseState,
    mut scope_stack: ScopeStack,
    code: &str,
    render_html: bool,
) -> Option<(ParseState, ScopeStack, String)> {
    let mut html = String::new();

    for line in LinesWithEndings::from(code) {
        let ops = parse_state.parse_line(line, &SYNTAX_SET).ok()?;
        if render_html {
            let line_html = classed_line_html(line, &ops, &mut scope_stack)?;
            html.push_str(&line_html);
        } else {
            for (_, op) in &ops {
                scope_stack.apply(op).ok()?;
            }
        }
    }

    Some((parse_state, scope_stack, html))
}

fn render_active_line(
    parse_state: ParseState,
    scope_stack: ScopeStack,
    code: &str,
) -> Option<String> {
    if code.is_empty() {
        return Some(String::new());
    }

    let mut parse_state = parse_state;
    let mut scope_stack = scope_stack;
    let ops = parse_state.parse_line(code, &SYNTAX_SET).ok()?;
    classed_line_html(code, &ops, &mut scope_stack)
}

/// 把一行 token 按「最内层 scope」平铺输出为行内自闭合的类化 span。
///
/// 与 ClassedHTMLGenerator 的跨行嵌套输出刻意不同：平铺 span 恒在本行内闭合，
/// 增量补丁的稳定区/活跃行才能作为两个互相独立的 DOM 片段安全拼接；
/// 类名与 ClassStyle::Spaced 一致（scope atom 空格分隔），与 .vcp-html-block 共享同一套 CSS 调色板。
///
/// 发射层面两个性能不变量：
/// - 相邻同 scope 的 token 合并进同一个 span（op 边界处内层 scope 开关而栈顶不变是常态），
///   压缩 HTML 体积与前端 DOM 节点数；
/// - scope → 类名字符串带单条目缓存，避免每个 token 都锁全局 scope 仓库并重复分配。
fn classed_line_html(
    line: &str,
    ops: &[(usize, ScopeStackOp)],
    stack: &mut ScopeStack,
) -> Option<String> {
    let mut html = String::new();
    let mut cursor = 0;
    let mut open_scope: Option<Scope> = None;
    let mut class_cache: Option<(Scope, String)> = None;

    for (index, op) in ops {
        if *index > cursor {
            append_classed_token(
                &mut html,
                &line[cursor..*index],
                stack,
                &mut open_scope,
                &mut class_cache,
            );
            cursor = *index;
        }
        stack.apply(op).ok()?;
    }
    if cursor < line.len() {
        append_classed_token(
            &mut html,
            &line[cursor..],
            stack,
            &mut open_scope,
            &mut class_cache,
        );
    }
    if open_scope.is_some() {
        html.push_str("</span>");
    }

    Some(html)
}

fn append_classed_token(
    html: &mut String,
    text: &str,
    stack: &ScopeStack,
    open_scope: &mut Option<Scope>,
    class_cache: &mut Option<(Scope, String)>,
) {
    if text.is_empty() {
        return;
    }
    let top = stack.scopes.last().copied();
    if *open_scope == top {
        // 栈顶未变：直接续进当前打开的 span（或继续裸文本），不新开标签。
        escape_html_into(html, text);
        return;
    }
    if open_scope.is_some() {
        html.push_str("</span>");
    }
    *open_scope = top;
    if let Some(scope) = top {
        let class = match class_cache {
            Some((cached_scope, cached_class)) if *cached_scope == scope => cached_class,
            _ => {
                *class_cache = Some((scope, scope.to_string().replace('.', " ")));
                &class_cache.as_ref().expect("cache just filled").1
            }
        };
        html.push_str("<span class=\"");
        html.push_str(class);
        html.push_str("\">");
    }
    escape_html_into(html, text);
}

fn escape_html_into(html: &mut String, text: &str) {
    for ch in text.chars() {
        match ch {
            '&' => html.push_str("&amp;"),
            '<' => html.push_str("&lt;"),
            '>' => html.push_str("&gt;"),
            _ => html.push(ch),
        }
    }
}

/// 仅输出带稳定区/活跃行锚点的纯净外壳。
/// 背景、边框等外壳样式移交前端全局 CSS（pre.vcp-code-block）做亮暗自适应，
/// 不再把 syntect 主题底色硬编码为内联样式（否则前端任何定制都压不过内联）。
fn wrap_incremental_code_html(completed_html: &str, active_html: &str) -> String {
    format!(
        "<pre class=\"vcp-code-block vcp-scrollable\"><code {}><span {}>{}</span><span {}>{}</span></code></pre>",
        STREAM_CODE_ROOT_ATTR,
        STREAM_CODE_STABLE_ATTR,
        completed_html,
        STREAM_CODE_ACTIVE_ATTR,
        active_html,
    )
}

/// 完成态代码块一次性高亮的外壳类别：仅决定 `<pre>` 锚点类名。
/// 前端 CSS 依据两个锚点类分别定制（pre.vcp-code-block / pre.vcp-html-block），不可合并。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CodeBlockShell {
    /// 普通代码块。
    Code,
    /// HTML 全预览卡片。
    Html,
}

/// 完成态代码块的一次性类化高亮（非流式持久化解析路径）。
/// 与增量路径一致输出 ClassStyle::Spaced 类化 span，外壳只挂锚点类，
/// 底色与 token 配色全部移交前端全局 CSS（pre.vcp-code-block / pre.vcp-html-block）按亮暗两态定义。
/// 输入内容来自 pulldown 围栏提取，已是干净文本，本函数不做任何裁剪。
pub fn highlight_code_block(code: &str, lang: &str, shell: CodeBlockShell) -> Option<String> {
    let syntax = syntax_for_language(lang);

    let mut html_generator =
        ClassedHTMLGenerator::new_with_class_style(syntax, &SYNTAX_SET, ClassStyle::Spaced);

    // newline 敏感的语法定义要求每行带换行结尾；原文最后一行若没有换行，
    // 临时补一个参与解析，输出前再摘除（不属于原文，否则块尾会多出空行）。
    let needs_synthetic_newline = !code.is_empty() && !code.ends_with('\n');
    for line in LinesWithEndings::from(code) {
        if line.ends_with('\n') {
            html_generator
                .parse_html_for_line_which_includes_newline(line)
                .ok()?;
        } else {
            let mut owned = line.to_string();
            owned.push('\n');
            html_generator
                .parse_html_for_line_which_includes_newline(&owned)
                .ok()?;
        }
    }

    let mut html = html_generator.finalize();
    if needs_synthetic_newline {
        if html.ends_with("\n</span>") {
            html.truncate(html.len() - "\n</span>".len());
            html.push_str("</span>");
        } else if html.ends_with('\n') {
            html.pop();
        }
    }

    let shell_class = match shell {
        CodeBlockShell::Code => "vcp-code-block",
        CodeBlockShell::Html => "vcp-html-block",
    };
    Some(format!(
        "<pre class=\"{} vcp-scrollable\"><code>{}</code></pre>",
        shell_class, html
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlight_code_block_emits_classed_spans_without_inline_styles() {
        let html = highlight_code_block("fn main() {}\n", "rust", CodeBlockShell::Code)
            .expect("highlight should succeed");
        assert!(html.starts_with("<pre class=\"vcp-code-block vcp-scrollable\"><code>"));
        assert!(html.contains("class=\""));
        assert!(!html.contains("style="));
    }

    #[test]
    fn incremental_start_emits_anchors_with_classed_spans() {
        let mut highlighter = IncrementalCodeHighlighter::default();
        highlighter.begin_frame();
        let html = highlighter
            .start("n1", "let a = 1;\nlet b", "rust")
            .expect("incremental start should succeed");
        assert!(html.contains(STREAM_CODE_ROOT_ATTR));
        assert!(html.contains(STREAM_CODE_STABLE_ATTR));
        assert!(html.contains(STREAM_CODE_ACTIVE_ATTR));
        assert!(html.contains("class=\""));
        assert!(!html.contains("style="));
    }

    #[test]
    fn incremental_append_emits_classed_patch_without_inline_styles() {
        let mut highlighter = IncrementalCodeHighlighter::default();
        highlighter.begin_frame();
        highlighter
            .start("n1", "let a = 1;\nlet b", "rust")
            .expect("incremental start should succeed");
        let patch = highlighter
            .append(
                "n1",
                "let a = 1;\nlet b",
                "let a = 1;\nlet b = 2;\nlet c",
                "rust",
            )
            .expect("append patch should succeed");
        assert!(patch.completed_html.contains("class=\""));
        assert!(patch.completed_html.contains('2'));
        assert!(patch.active_html.contains("let"));
        assert!(patch.active_html.contains('c'));
        assert!(!patch.completed_html.contains("style="));
        assert!(!patch.active_html.contains("style="));
    }

    #[test]
    fn flat_emitter_merges_adjacent_tokens_sharing_top_scope() {
        let scope = Scope::new("source.rust").expect("valid scope");
        // Noop 制造 op 边界但栈顶不变：两段 token 应合并进同一个 span。
        let ops = [(0, ScopeStackOp::Push(scope)), (2, ScopeStackOp::Noop)];
        let mut stack = ScopeStack::new();
        let html = classed_line_html("aabb", &ops, &mut stack).expect("emit should succeed");
        assert_eq!(html, "<span class=\"source rust\">aabb</span>");
    }

    #[test]
    fn flat_emitter_splits_on_top_scope_change_and_escapes_bare_text() {
        let keyword = Scope::new("keyword.control.rust").expect("valid scope");
        // "if" 在 keyword 内，"<" 弹栈后为无 scope 裸文本，必须转义且不包 span。
        let ops = [(0, ScopeStackOp::Push(keyword)), (2, ScopeStackOp::Pop(1))];
        let mut stack = ScopeStack::new();
        let html = classed_line_html("if<", &ops, &mut stack).expect("emit should succeed");
        assert_eq!(html, "<span class=\"keyword control rust\">if</span>&lt;");
    }

    #[test]
    fn flat_emitter_reuses_scope_class_cache_across_tokens() {
        // 两段同 scope 裸文本之间隔一个无 scope 段：第二次开 span 仍走缓存，输出一致。
        let scope = Scope::new("comment.block.rust").expect("valid scope");
        let ops = [
            (0, ScopeStackOp::Push(scope)),
            (1, ScopeStackOp::Pop(1)),
            (2, ScopeStackOp::Push(scope)),
            (3, ScopeStackOp::Pop(1)),
        ];
        let mut stack = ScopeStack::new();
        let html = classed_line_html("a=b", &ops, &mut stack).expect("emit should succeed");
        assert_eq!(
            html,
            "<span class=\"comment block rust\">a</span>=<span class=\"comment block rust\">b</span>"
        );
    }

    #[test]
    fn highlight_code_block_preserves_source_text_exactly() {
        let code =
            "fn main() {\n    let x = 1;\n    if x > 0 {\n        println!(\"{}\", x);\n    }\n}";
        let html = highlight_code_block(code, "rust", CodeBlockShell::Code)
            .expect("highlight should succeed");
        let inner = html
            .strip_prefix("<pre class=\"vcp-code-block vcp-scrollable\"><code>")
            .and_then(|s| s.strip_suffix("</code></pre>"))
            .expect("shell wrapper");
        assert_eq!(strip_tags_for_test(inner), code);
    }

    #[test]
    fn incremental_stream_preserves_source_text_across_patches() {
        let v1 = "fn main() {\n    let x";
        let v2 =
            "fn main() {\n    let x = 1;\n    if x > 0 {\n        println!(\"{}\", x);\n    }\n}";
        let mut highlighter = IncrementalCodeHighlighter::default();
        highlighter.begin_frame();
        let start_html = highlighter.start("n1", v1, "rust").expect("start");
        assert_eq!(strip_tags_for_test(&start_html), v1);

        let patch = highlighter.append("n1", v1, v2, "rust").expect("append");
        let mut dom_text = strip_tags_for_test(&start_html);
        dom_text = format!(
            "{}{}{}",
            // start 的活跃行被 replace：去掉 v1 的活跃部分 "    let x"
            &dom_text[..dom_text.len() - "    let x".len()],
            strip_tags_for_test(&patch.completed_html),
            strip_tags_for_test(&patch.active_html),
        );
        assert_eq!(dom_text, v2);
    }

    #[test]
    fn html_shell_uses_html_block_anchor_class() {
        // Html 外壳挂 vcp-html-block 锚点类（与 vcp-code-block 区分），且不硬编码内联样式
        let html = highlight_code_block("<div>hi</div>\n", "html", CodeBlockShell::Html)
            .expect("highlight");
        assert!(html.starts_with("<pre class=\"vcp-html-block vcp-scrollable\"><code>"));
        assert!(html.contains("class=\""));
        assert!(!html.contains("style="));
    }

    #[test]
    fn html_shell_preserves_content_byte_exactly() {
        // 上游已是 pulldown 提取的干净内容，高亮层不做任何裁剪
        let html = highlight_code_block("<div>hi</div>\n", "html", CodeBlockShell::Html)
            .expect("highlight");
        let inner = html
            .strip_prefix("<pre class=\"vcp-html-block vcp-scrollable\"><code>")
            .and_then(|s| s.strip_suffix("</code></pre>"))
            .expect("shell wrapper");
        assert_eq!(strip_tags_for_test(inner), "<div>hi</div>\n");
    }

    #[test]
    fn html_shell_preserves_blank_edges_verbatim() {
        // 内容自身的首尾空行属于原文，原样保留（不再有围栏工件剥离逻辑）
        let html = highlight_code_block("\n<div>hi</div>\n\n", "html", CodeBlockShell::Html)
            .expect("highlight");
        let inner = html
            .strip_prefix("<pre class=\"vcp-html-block vcp-scrollable\"><code>")
            .and_then(|s| s.strip_suffix("</code></pre>"))
            .expect("shell wrapper");
        assert_eq!(strip_tags_for_test(inner), "\n<div>hi</div>\n\n");
    }

    fn strip_tags_for_test(html: &str) -> String {
        let mut out = String::new();
        let mut in_tag = false;
        for ch in html.chars() {
            match ch {
                '<' => in_tag = true,
                '>' => in_tag = false,
                _ if !in_tag => out.push(ch),
                _ => {}
            }
        }
        out.replace("&amp;", "&")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
    }
}

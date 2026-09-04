//! vcp_modules/context_sanitizer.rs
//! 上下文 HTML -> Markdown 净化器（薄门面）
//!
//! 转换引擎已迁移至 [`crate::vcp_modules::infra::html2md`]（htmd + VCP 自定义 handler）；
//! 本模块保留门面职责：LRU+TTL 缓存、VCP 原始块直通、think 标签字面量保护、
//! 元思考链明文剥离，流程对齐桌面端 contextSanitizer.js。

use lazy_static::lazy_static;
use lru::LruCache;
use regex::Regex;
use std::num::NonZeroUsize;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use crate::vcp_modules::infra::html2md;

lazy_static! {
    /// 剥离 VCP 元思考链（桌面行锚定语义：起止标记必须各自独占一行，
    /// 正文/行内代码里的示例原样保留）
    static ref THOUGHT_CHAIN_REGEX: Regex = Regex::new(r#"(?m)^[ \t]*\[--- VCP元思考链(?::\s*"[^"]*")?\s*---\][ \t]*\r?\n[\s\S]*?^[ \t]*\[--- 元思考链结束 ---\][ \t]*(?:\r?\n|$)"#).unwrap();
    /// 剥离常规 <think>/<thinking> 块（行锚定 + 大小写不敏感）
    static ref CONVENTIONAL_THOUGHT_REGEX: Regex = Regex::new(r"(?im)^[ \t]*<think(?:ing)?>[ \t]*\r?\n[\s\S]*?^[ \t]*</think(?:ing)?>[ \t]*(?:\r?\n|$)").unwrap();
    /// think 标签字面量保护：转换前替换为占位符，防止解析器把协议讲解文本里的
    /// think 标签当元素吞掉；转换后逐个恢复
    static ref THINK_TAG_LITERAL_REGEX: Regex = Regex::new(r"(?i)</?think(?:ing)?>").unwrap();
    /// 简单检查是否包含 HTML 标签的正则表达式
    static ref HTML_CHECK_REGEX: Regex = Regex::new(r"<[^>]+>").unwrap();
    /// 清理多余空行（保留最多2个连续空行）的正则表达式
    static ref MULTI_NEWLINE_REGEX: Regex = Regex::new(r"\n{3,}").unwrap();
}

/// 缓存项结构，支持过期时间
pub struct CacheItem {
    pub value: String,
    pub timestamp: SystemTime,
}

/// 上下文净化器结构体，管理 LRU 缓存与 TTL
pub struct ContextSanitizer {
    /// 线程安全的 LRU 缓存：内容哈希 -> 净化后的内容
    pub cache: Mutex<LruCache<String, CacheItem>>,
    /// 缓存有效期 (Time To Live)
    pub ttl: Duration,
}

impl ContextSanitizer {
    /// 创建新的净化器实例
    /// @param capacity 缓存最大容量
    /// @param ttl_secs 过期时间（秒）
    pub fn new(capacity: usize, ttl_secs: u64) -> Self {
        log::info!(
            "[ContextSanitizer] Initializing Rust ContextSanitizer with capacity {} and TTL {}s",
            capacity,
            ttl_secs
        );
        Self {
            cache: Mutex::new(LruCache::new(NonZeroUsize::new(capacity).unwrap())),
            ttl: Duration::from_secs(ttl_secs),
        }
    }

    /// 从缓存中获取已净化的内容
    /// @param key 缓存键
    pub fn get_cached(&self, key: &str) -> Option<String> {
        let mut cache = self.cache.lock().unwrap();
        if let Some(item) = cache.get(key) {
            // 检查是否过期
            if let Ok(elapsed) = item.timestamp.elapsed() {
                if elapsed < self.ttl {
                    log::debug!("[ContextSanitizer] Cache hit for content");
                    return Some(item.value.clone());
                }
            }
            // 已过期，删除
            cache.pop(key);
        }
        None
    }

    /// 将净化后的内容存入缓存
    /// @param key 缓存键
    /// @param value 净化后的内容
    pub fn set_cached(&self, key: String, value: String) {
        let mut cache = self.cache.lock().unwrap();
        cache.put(
            key,
            CacheItem {
                value,
                timestamp: SystemTime::now(),
            },
        );
        log::debug!("[ContextSanitizer] Sanitized content, cached result");
    }

    /// 净化单条消息内容：HTML -> Markdown (带缓存逻辑)
    /// 流程对齐桌面端：空检查 -> VCP 原始块直通 -> HTML 检查 -> 缓存 -> 转换
    /// @param content 原始内容
    /// @param keep_thoughts 是否保留思考链
    pub fn sanitize_content(&self, content: &str, keep_thoughts: bool) -> String {
        if content.trim().is_empty() {
            return content.to_string();
        }

        // 对原始工具调用块 / DailyNote 块做直通保护，避免被 Markdown 转义污染
        if contains_raw_vcp_blocks(content) {
            return content.to_string();
        }

        // 如果不包含 HTML，直接返回
        if !contains_html(content) {
            return content.to_string();
        }

        // 尝试从缓存获取
        let cache_key = generate_cache_key(content, keep_thoughts);
        if let Some(cached) = self.get_cached(&cache_key) {
            return cached;
        }

        // 核心执行：HTML 转换为 Markdown（出错时回退原始内容，且不进缓存）
        let Some(result) = sanitize_core(content, keep_thoughts) else {
            return content.to_string();
        };

        // 存入缓存
        self.set_cached(cache_key, result.clone());
        result
    }
}

/// 默认配置：最大 100 条缓存，1 小时过期
impl Default for ContextSanitizer {
    fn default() -> Self {
        Self::new(100, 3600)
    }
}

/// 清理元思考链（明文形式，桌面行锚定语义）
/// 只有起止标记分别独占一行时才视为思维链协议；
/// 正文中的 `<think>...</think>` 示例、行内代码或标签说明必须原样保留。
/// @param content 原始内容
/// @returns 清理后的内容
pub fn strip_thought_chains(content: &str) -> String {
    let s = THOUGHT_CHAIN_REGEX.replace_all(content, "");
    CONVENTIONAL_THOUGHT_REGEX.replace_all(&s, "").to_string()
}

/// 检查内容是否包含原始 VCP 特殊块（成对标记），避免进入 HTML -> Markdown 后被额外转义
/// @param content 要检查的内容
pub fn contains_raw_vcp_blocks(content: &str) -> bool {
    (content.contains("<<<[TOOL_REQUEST]>>>") && content.contains("<<<[END_TOOL_REQUEST]>>>"))
        || (content.contains("<<<DailyNoteStart>>>") && content.contains("<<<DailyNoteEnd>>>"))
}

/// 核心转换管线：think 字面量保护 -> html2md 引擎转换 -> 字面量恢复 -> 空行收敛 + trim
/// 返回 None 表示转换失败（调用方按桌面语义回退原始内容）
fn sanitize_core(content: &str, keep_thoughts: bool) -> Option<String> {
    // 解析器会把用于讲解协议的字面量 <think>/<thinking> 标签当成未知 HTML 元素并吞掉标签本身，
    // 先保护所有标签字面量；真正需要清理的完整思维链块由 strip_thought_chains 按独占行规则处理
    let mut thought_tag_literals: Vec<String> = Vec::new();
    let protected = THINK_TAG_LITERAL_REGEX
        .replace_all(content, |caps: &regex::Captures| {
            let placeholder = format!("VCPTHOUGHTTAGLITERAL{}TOKEN", thought_tag_literals.len());
            thought_tag_literals.push(caps[0].to_string());
            placeholder
        })
        .to_string();

    let mut markdown = match html2md::convert(&protected, keep_thoughts) {
        Ok(markdown) => markdown,
        Err(error) => {
            log::error!("[ContextSanitizer] Error sanitizing content: {}", error);
            return None;
        }
    };

    for (index, tag) in thought_tag_literals.iter().enumerate() {
        markdown = markdown.replace(&format!("VCPTHOUGHTTAGLITERAL{index}TOKEN"), tag);
    }

    // 清理多余空行（保留最多2个连续空行）
    Some(
        MULTI_NEWLINE_REGEX
            .replace_all(markdown.trim(), "\n\n")
            .to_string(),
    )
}

/// 核心转换管线的薄包装：HTML -> VCP 风格 Markdown（不含缓存与直通检查）
/// 供测试与未来导出复用；完整净化流程请走 [`ContextSanitizer::sanitize_content`]
/// @param html 输入的 HTML 字符串
/// @param keep_thoughts 是否保留思考链
#[allow(dead_code)]
pub fn html_to_vcp_markdown(html: &str, keep_thoughts: bool) -> String {
    sanitize_core(html, keep_thoughts).unwrap_or_else(|| html.to_string())
}

/// 检查内容是否包含 HTML 标签
pub fn contains_html(content: &str) -> bool {
    HTML_CHECK_REGEX.is_match(content)
}

/// 生成缓存键（使用哈希与长度组合）
pub fn generate_cache_key(content: &str, keep_thoughts: bool) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    keep_thoughts.hash(&mut hasher);
    let hash = hasher.finish();
    format!("sanitized_{}_{}", hash, content.len())
}

// ---------------------------------------------------------
// Tauri Commands
// ---------------------------------------------------------

/// 净化单条消息内容：HTML -> Markdown（含 LRU+TTL 缓存与 VCP 直通保护）
#[tauri::command]
pub async fn chat_sanitize_content(
    sanitizer: tauri::State<'_, ContextSanitizer>,
    content: String,
    keep_thoughts: bool,
) -> Result<String, String> {
    Ok(sanitizer.sanitize_content(&content, keep_thoughts))
}

/// 明文剥离元思考链（桌面行锚定语义）
#[tauri::command]
pub async fn chat_strip_thought_chains(content: String) -> Result<String, String> {
    Ok(strip_thought_chains(&content))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── strip_thought_chains：桌面行锚定语义 ──

    #[test]
    fn test_strip_thoughts_line_anchored_blocks_removed() {
        // 起止标记各自独占一行 -> 整块剥离（含 <think> 与 <thinking> 变体）
        let input = "前言\n\
                     [--- VCP元思考链: \"计划\" ---]\n\
                     秘密推理\n\
                     [--- 元思考链结束 ---]\n\
                     正文\n\
                     <think>\n\
                     内部独白\n\
                     </think>\n\
                     <thinking>\n\
                     变体独白\n\
                     </thinking>\n\
                     结尾";
        assert_eq!(strip_thought_chains(input), "前言\n正文\n结尾");
    }

    #[test]
    fn test_strip_thoughts_inline_mentions_preserved() {
        // 行内提及（未独占行）必须原样保留
        let input = "这是 [--- VCP元思考链: \"x\" ---] 示例 [--- 元思考链结束 ---] 与 `<think>a</think>` 讲解";
        assert_eq!(strip_thought_chains(input), input);
    }

    #[test]
    fn test_strip_thoughts_allows_leading_whitespace_on_marker_lines() {
        let input = "前文\n  <think>\n内容\n\t</thinking>\n后文";
        assert_eq!(strip_thought_chains(input), "前文\n后文");
    }

    // ── VCP 特殊块零损提取 ──

    #[test]
    fn test_prettified_bubble_returns_raw_content_verbatim() {
        let html = r#"<pre class="vcp-tool-use-bubble" data-raw-content="<<<[TOOL_REQUEST]>>> call(**kwargs**) <<<[END_TOOL_REQUEST]>>>"><code>美化后的HTML不应出现</code></pre>"#;
        let md = html_to_vcp_markdown(html, false);
        assert_eq!(
            md,
            "<<<[TOOL_REQUEST]>>> call(**kwargs**) <<<[END_TOOL_REQUEST]>>>"
        );

        // maid-diary-bubble 变体
        let html = r#"<pre class="maid-diary-bubble" data-raw-content="<<<DailyNoteStart>>> 日记 <<<DailyNoteEnd>>>">美化</pre>"#;
        let md = html_to_vcp_markdown(html, false);
        assert_eq!(md, "<<<DailyNoteStart>>> 日记 <<<DailyNoteEnd>>>");
    }

    #[test]
    fn test_raw_content() {
        let html = "<pre data-raw-content=\"<<<[TOOL_REQUEST]>>>\ncall()\"></pre>";
        let md = html_to_vcp_markdown(html, false);
        assert_eq!(md, "<<<[TOOL_REQUEST]>>>\ncall()");
    }

    #[test]
    fn test_raw_marker_pre_passthrough() {
        // 未美化但含特殊标记的 pre：textContent 原文直通
        let html = "<pre><<<[TOOL_REQUEST]>>>\nfn call()\n<<<[END_TOOL_REQUEST]>>></pre>";
        let md = html_to_vcp_markdown(html, false);
        assert_eq!(
            md,
            "<<<[TOOL_REQUEST]>>>\nfn call()\n<<<[END_TOOL_REQUEST]>>>"
        );

        // 注意：`<<<DailyNoteStart>>>` 不带方括号，`<DailyNoteStart>` 会被 HTML5 解析器
        // 当成未知元素吞掉（桌面 jsdom 行为相同），因此完整 DailyNote 块依赖
        // contains_raw_vcp_blocks 前置直通保护（见 test_contains_raw_vcp_blocks_passthrough），
        // pre handler 里的 DailyNote 标记检查仅作为与桌面规则4 的镜像兜底。
    }

    // ── 元思考链泡泡：keep / drop 两态 ──

    #[test]
    fn test_thought_chain_bubble_keep_with_title() {
        let html = r#"<div class="vcp-thought-chain-bubble" data-thought-title="计划"><p>思考内容</p></div>"#;
        let md = html_to_vcp_markdown(html, true);
        assert!(md.contains("[--- VCP元思考链: \"计划\" ---]"), "md={md}");
        assert!(md.contains("思考内容"), "md={md}");
        assert!(md.contains("[--- 元思考链结束 ---]"), "md={md}");
    }

    #[test]
    fn test_thought_chain_bubble_keep_without_title() {
        let html = r#"<div class="vcp-thought-chain-bubble"><p>思考内容</p></div>"#;
        let md = html_to_vcp_markdown(html, true);
        assert!(md.contains("[--- VCP元思考链 ---]"), "md={md}");
        assert!(md.contains("思考内容"), "md={md}");
    }

    #[test]
    fn test_thought_chain_bubble_dropped_when_not_keeping() {
        let html = r#"<p>前</p><div class="vcp-thought-chain-bubble" data-thought-title="计划"><p>思考内容</p></div><p>后</p>"#;
        let md = html_to_vcp_markdown(html, false);
        assert!(!md.contains("思考内容"), "md={md}");
        assert!(!md.contains("VCP元思考链"), "md={md}");
        assert!(md.contains('前'), "md={md}");
        assert!(md.contains('后'), "md={md}");
    }

    #[test]
    fn test_thought_chain_bubble_class_match_is_case_sensitive() {
        // 对齐 JS classList 语义：大小写敏感（纠正旧实现的 AsciiCaseInsensitive 偏差）
        let html = r#"<div class="VCP-THOUGHT-CHAIN-BUBBLE">内容保留</div>"#;
        let md = html_to_vcp_markdown(html, false);
        assert!(md.contains("内容保留"), "md={md}");
    }

    // ── 多媒体保留 ──

    #[test]
    fn test_html_to_md_img() {
        let html = r#"<p>Hello <img src="test.png" alt="alt text"> World</p>"#;
        let md = html_to_vcp_markdown(html, false);
        assert!(md.contains(r#"<img src="test.png" alt="alt text">"#));
    }

    #[test]
    fn test_img_without_src_dropped() {
        let html = r#"<p>前<img alt="no src">后</p>"#;
        let md = html_to_vcp_markdown(html, false);
        assert!(!md.contains("<img"), "md={md}");
        assert!(md.contains("前"), "md={md}");
    }

    #[test]
    fn test_audio_video_preserved() {
        let html = r#"<p><audio src="a.mp3"></audio><video src="v.mp4"></video></p>"#;
        let md = html_to_vcp_markdown(html, false);
        assert!(md.contains(r#"<audio src="a.mp3"></audio>"#), "md={md}");
        assert!(md.contains(r#"<video src="v.mp4"></video>"#), "md={md}");
    }

    #[test]
    fn test_media_source_child_fallback() {
        // 无 src 时取第一个 <source> 子元素
        let html = r#"<video><source src="fallback.mp4"></video>"#;
        let md = html_to_vcp_markdown(html, false);
        assert!(
            md.contains(r#"<video src="fallback.mp4"></video>"#),
            "md={md}"
        );

        // 第一个 <source> 无 src -> 丢弃（对齐桌面 querySelectorAll('source')[0] 语义）
        let html = r#"<audio><source><source src="second.mp3"></audio>"#;
        let md = html_to_vcp_markdown(html, false);
        assert!(!md.contains("<audio"), "md={md}");
    }

    // ── sanitize_content 门面流程 ──

    #[test]
    fn test_contains_raw_vcp_blocks_passthrough() {
        let sanitizer = ContextSanitizer::default();
        // 成对标记存在时整条原样返回，即使内容里含 HTML 形态文本
        let raw = "<<<[TOOL_REQUEST]>>>\n<think>伪标签</think>\n<<<[END_TOOL_REQUEST]>>>";
        assert_eq!(sanitizer.sanitize_content(raw, false), raw);

        let raw = "<p>说明</p>\n<<<DailyNoteStart>>>\nx\n<<<DailyNoteEnd>>>";
        assert_eq!(sanitizer.sanitize_content(raw, false), raw);

        // 不成对则正常走转换
        assert!(!contains_raw_vcp_blocks("<<<[TOOL_REQUEST]>>> 单独的"));
    }

    #[test]
    fn test_think_tag_literals_protected() {
        let sanitizer = ContextSanitizer::default();
        // 协议讲解文本里的 think 标签字面量：转换后必须原样还在
        let input = "协议讲解：<think> 与 </think> 是字面量<strong>加粗</strong>";
        let md = sanitizer.sanitize_content(input, false);
        assert!(md.contains("<think>"), "md={md}");
        assert!(md.contains("</think>"), "md={md}");
        assert!(md.contains("**加粗**"), "md={md}");

        // <thinking> 变体 + 大小写不敏感
        let input = "变体 <thinking> 和 </THINKING> 讲解";
        let md = sanitizer.sanitize_content(input, false);
        assert!(md.contains("<thinking>"), "md={md}");
        assert!(md.contains("</THINKING>"), "md={md}");
    }

    // ── 通用标签语义（不断言精确空白） ──

    #[test]
    fn test_general_html_semantics() {
        let html = "<h1>标题一</h1><h3>标题三</h3>\
                    <p><strong>粗</strong>与<em>斜</em>与<code>x=1</code></p>\
                    <ul><li>甲</li><li>乙</li></ul>\
                    <blockquote><p>引用</p></blockquote>\
                    <pre><code>let x = 1;</code></pre>\
                    <table><thead><tr><th>H</th></tr></thead><tbody><tr><td>C</td></tr></tbody></table>\
                    <hr>";
        let md = html_to_vcp_markdown(html, false);
        assert!(md.contains("# 标题一"), "md={md}");
        assert!(md.contains("### 标题三"), "md={md}");
        assert!(md.contains("**粗**"), "md={md}");
        assert!(md.contains("*斜*"), "md={md}");
        assert!(md.contains("`x=1`"), "md={md}");
        assert!(
            md.lines()
                .any(|line| line.starts_with("- ") && line.contains('甲')),
            "md={md}"
        );
        assert!(
            md.lines()
                .any(|line| line.starts_with("- ") && line.contains('乙')),
            "md={md}"
        );
        assert!(md.contains("> 引用"), "md={md}");
        assert!(md.contains("```"), "md={md}");
        assert!(md.contains("let x = 1;"), "md={md}");
        assert!(
            md.contains('|') && md.contains('H') && md.contains('C'),
            "md={md}"
        );
        // HrStyle::Dashes 渲染为 `- - -`（桌面 turndown hr:'---' 意图的对齐项）
        assert!(md.contains("- - -"), "md={md}");
    }

    // ── 缓存 ──

    #[test]
    fn test_cache_capacity_evicts_least_recent_item() {
        let sanitizer = ContextSanitizer::new(1, 60);
        sanitizer.set_cached("first".to_string(), "one".to_string());
        assert_eq!(sanitizer.get_cached("first"), Some("one".to_string()));

        sanitizer.set_cached("second".to_string(), "two".to_string());

        assert_eq!(sanitizer.get_cached("first"), None);
        assert_eq!(sanitizer.get_cached("second"), Some("two".to_string()));
    }

    #[test]
    fn test_sanitize_content_uses_cache() {
        let sanitizer = ContextSanitizer::default();
        let html = "<p>Hello <strong>World</strong></p>";
        let first = sanitizer.sanitize_content(html, false);
        let second = sanitizer.sanitize_content(html, false);
        assert_eq!(first, second);
        assert!(first.contains("**World**"), "md={first}");
    }
}

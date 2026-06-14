

const injectedStyles = new Map<string, string>();
const rawCssCache = new Map<string, string>();

/**
 * Composable that provides scoped style injection helpers for message bubbles.
 * Converts global-like selectors into scoped selectors targeting a specific message ID
 * to prevent user/agent-generated HTML preview styles from polluting the global application theme.
 */
export function useMessageStyleInjector() {
  /**
   * Scopes and injects raw CSS scoped to a specific message ID.
   * Modifies selectors (except keyframe definitions) to be nested under `[data-message-id="..."]`.
   */
  const injectScopedCss = (css: string, messageId: string) => {
    if (!css || !messageId) return;
    
    // 提前去重校验：若原始 CSS 无变化，直接拦截，完全跳过后面重型 selector scoping 的正则运算
    if (rawCssCache.get(messageId) === css) return;
    rawCssCache.set(messageId, css);

    const scopeSelector = `[data-message-id="${messageId}"]`;
    const scopedCss = css.replace(
      /(^|\}|\{)\s*([^{]+)\s*\{/g,
      (_, prefix, selectors) => {
        const scopedSelectors = selectors
          .split(",")
          .map((s: string) => {
            const sel = s.trim();
            // Allow keyframe percentages and prefixes directly
            if (sel.match(/^(@|from|to|\d+%)/)) return sel;
            // Prevent html, body, :root from polluting the entire global tree
            if (sel.match(/^(html|body|:root)/i)) return `${scopeSelector}`;
            if (sel === "#vcp-root") return `${scopeSelector} ${sel}`;
            if (sel.startsWith("::") || sel.startsWith(":"))
              return `${scopeSelector}${sel}`;
            return `${scopeSelector} ${sel}`;
          })
          .join(", ");
        return `${prefix} ${scopedSelectors} {`;
      },
    );

    if (injectedStyles.get(messageId) === scopedCss) return;
    injectedStyles.set(messageId, scopedCss);

    let styleEl = document.getElementById(`style-${messageId}`);
    if (!styleEl) {
      styleEl = document.createElement("style");
      styleEl.id = `style-${messageId}`;
      styleEl.setAttribute("data-vcp-scope-id", messageId);
      document.head.appendChild(styleEl);
    }
    styleEl.textContent = scopedCss;
  };

  /**
   * Removes the scoped style element associated with a specific message ID.
   * Uses a setTimeout delay to prevent the style sheet from being instantly removed
   * and re-injected during the streaming-to-stable transition tick, which causes layout flicker.
   */
  const removeScopedCss = (messageId: string) => {
    if (!messageId) return;
    
    // 立即注销 rawCss 活跃状态。如果后面有新静态块接管，它会同步重新执行 injectScopedCss 重新 set 写入
    rawCssCache.delete(messageId);
    
    // 延迟 50ms 物理清理，给新静态块挂载和样式接管留出时间差
    setTimeout(() => {
      // 核心门禁：如果在这 50ms 期间有新块重新写入并接管了该 messageId，说明样式依然活跃，保留它
      if (rawCssCache.has(messageId)) {
        return;
      }
      
      const styleEl = document.getElementById(`style-${messageId}`);
      if (styleEl) {
        styleEl.remove();
      }
      injectedStyles.delete(messageId);
    }, 50);
  };

  return {
    injectScopedCss,
    removeScopedCss,
  };
}

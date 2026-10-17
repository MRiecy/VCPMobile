import { onScopeDispose } from 'vue';

/**
 * Composable that provides scoped style injection helpers for message bubbles.
 * Converts global-like selectors into scoped selectors targeting a specific message ID
 * to prevent user/agent-generated HTML preview styles from polluting the global application theme.
 */
export function useMessageStyleInjector() {
  // Each mounted message owns its actual style element, including during overlapping remounts.
  let styleEl: HTMLStyleElement | null = null;
  let currentMessageId = '';
  let currentCss = '';

  const removeScopedCss = () => {
    styleEl?.remove();
    styleEl = null;
    currentMessageId = '';
    currentCss = '';
  };
  onScopeDispose(removeScopedCss);
  /**
   * Scopes and injects raw CSS scoped to a specific message ID.
   * Modifies selectors (except keyframe definitions) to be nested under `[data-message-id="..."]`.
   */
  const injectScopedCss = (css: string, messageId: string) => {
    if (!css.trim() || !messageId) {
      removeScopedCss();
      return;
    }
    
    // 提前去重校验：若原始 CSS 无变化，直接拦截，完全跳过后面重型 selector scoping 的正则运算
    if (currentMessageId === messageId && currentCss === css && styleEl?.isConnected) return;

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

    if (!styleEl) {
      styleEl = document.createElement("style");
    }
    styleEl.setAttribute("data-vcp-scope-id", messageId);
    styleEl.textContent = scopedCss;
    if (!styleEl.isConnected) document.head.appendChild(styleEl);
    currentMessageId = messageId;
    currentCss = css;
  };

  return {
    injectScopedCss,
  };
}

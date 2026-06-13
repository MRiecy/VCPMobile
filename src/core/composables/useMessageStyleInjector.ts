

const injectedStyles = new Map<string, string>();

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
   */
  const removeScopedCss = (messageId: string) => {
    if (!messageId) return;
    const styleEl = document.getElementById(`style-${messageId}`);
    if (styleEl) {
      styleEl.remove();
    }
    injectedStyles.delete(messageId);
  };

  return {
    injectScopedCss,
    removeScopedCss,
  };
}

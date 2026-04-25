import { invoke } from "@tauri-apps/api/core";

export interface ContentBlock {
  type:
  | "markdown"
  | "tool-use"
  | "tool-result"
  | "diary"
  | "thought"
  | "button-click"
  | "html-preview"
  | "role-divider"
  | "style";
  content: string;
  tool_name?: string;
  is_complete?: boolean;
  status?: string;
  details?: Array<{ key: string; value: string }>;
  footer?: string;
  maid?: string;
  date?: string;
  theme?: string;
  role?: string;
  is_end?: boolean;
}

const injectedStyles = new Map<string, string>();

export function useContentProcessor() {
  /**
   * Escape HTML special characters
   */
  const escapeHtml = (text: string) => {
    return text
      .replace(/&/g, "&")
      .replace(/</g, "<")
      .replace(/>/g, ">")
      .replace(/"/g, "&quot;")
      .replace(/'/g, "&#039;");
  };

  const injectScopedCss = (css: string, messageId: string) => {
    if (!css || !messageId) return;
    const scopeSelector = `[data-message-id="${messageId}"]`;
    const scopedCss = css.replace(
      /(^|\})\s*([^{]+)\s*\{/g,
      (_, prefix, selectors) => {
        const scopedSelectors = selectors
          .split(",")
          .map((s: string) => {
            const sel = s.trim();
            // 允许动画关键帧直接通过
            if (sel.match(/^(@|from|to|\d+%)/)) return sel;
            // 核心修复：绝对禁止 html, body 污染全局，强行降维到当前气泡
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
      document.head.appendChild(styleEl);
    }
    styleEl.textContent = scopedCss;
  };

  const removeScopedCss = (messageId: string) => {
    if (!messageId) return;
    const styleEl = document.getElementById(`style-${messageId}`);
    if (styleEl) {
      styleEl.remove();
    }
    injectedStyles.delete(messageId);
  };

  const transformSpecialBlocksForStream = (text: string) => {
    let processed = text;
    // THOUGHT
    processed = processed.replace(
      /\[--- VCP元思考链(?::\s*([^\]]*?))?\s*---\]([\s\S]*?)(?:\[--- 元思考链结束 ---\]|$)/gs,
      (_, theme, content) => {
        const displayTheme = theme
          ? theme.trim().replace(/"/g, "")
          : "元思考链";
        return `\n<div class="my-2 p-3 bg-black/5 dark:bg-white/5 rounded-xl border border-black/10 dark:border-white/10 text-sm"><div class="flex items-center gap-2 mb-2 opacity-70 font-bold"><span class="grayscale">🧠</span> <span>${displayTheme}</span><span class="i-lucide-loader-2 animate-spin text-[12px]">...</span></div><div class="italic opacity-80">${content}</div></div>\n`;
      },
    );
    // THINK (Standard)
    processed = processed.replace(
      /<think(?:ing)?>([\s\S]*?)(?:<\/think(?:ing)?>|$)/gi,
      (_, content) => {
        return `\n<div class="my-2 p-3 bg-black/5 dark:bg-white/5 rounded-xl border border-black/10 dark:border-white/10 text-sm"><div class="flex items-center gap-2 mb-2 opacity-70 font-bold"><span class="grayscale">🧠</span> <span>思维链</span><span class="i-lucide-loader-2 animate-spin text-[12px]">...</span></div><div class="italic opacity-80">${content}</div></div>\n`;
      },
    );
    // TOOL
    processed = processed.replace(
      /<<<\[TOOL_REQUEST\]>>>(.*?)(?:<<<\[END_TOOL_REQUEST\]>>>|$)/gs,
      (_, content) => {
        const nameMatch = content.match(
          /<tool_name>([\s\S]*?)<\/tool_name>|tool_name:\s*「始(?:exp)?」([^「」]*)「末(?:exp)?」/,
        );
        let toolName = "Processing...";
        if (nameMatch) {
          toolName = (nameMatch[1] || nameMatch[2] || "")
            .trim()
            .replace(/「始」|「末」|「始exp」|「末exp」/g, "")
            .replace(/,$/, "")
            .trim();
        }
        if (content.includes("DailyNote") && content.includes("create")) {
          return `\n<div class="my-2 p-3 bg-amber-500/10 border border-amber-500/20 rounded-xl text-sm"><div class="font-bold text-amber-600 dark:text-amber-400 mb-1">📖 日记撰写中...</div><pre class="opacity-70 text-xs whitespace-pre-wrap">${escapeHtml(content)}</pre></div>\n`;
        }
        return `\n<div class="my-2 p-3 bg-blue-500/5 border border-blue-500/20 rounded-xl text-sm"><div class="font-bold text-blue-500 mb-1">🛠️ 工具调用: ${toolName}</div><pre class="opacity-70 text-xs whitespace-pre-wrap">${escapeHtml(content)}</pre></div>\n`;
      },
    );
    return processed;
  };

  /**
   * Main entry point for processing content (Async, calls Rust backend if static)
   */
  const processMessageContent = async (
    text: string,
    options: {
      role: string;
      depth: number;
      messageId?: string;
      isStreaming?: boolean;
    },
  ): Promise<ContentBlock[]> => {
    if (!text) return [];

    // 1. Hybrid Pipeline branching
    if (options.isStreaming) {
      let processed = transformSpecialBlocksForStream(text);

      // Streaming mode still needs basic role divider placeholder for better UX
      const ROLE_DIVIDER_REGEX = /<<<\[(END_)?ROLE_DIVIDE_(SYSTEM|ASSISTANT|USER)\]>>>/g;
      processed = processed.replace(ROLE_DIVIDER_REGEX, (_match, endMarker, role) => {
        const isEnd = !!endMarker;
        const roleLower = role.toLowerCase();
        const actionText = isEnd ? "[结束]" : "[起始]";
        return `\n<div class="vcp-role-divider role-${roleLower}"><span class="divider-text">角色分界: ${role} ${actionText}</span></div>\n`;
      });

      return [{ type: "markdown", content: processed }];
    }

    // 2. Call Rust backend to parse the text into AST blocks (Static Mode)
    let blocks: ContentBlock[] = [];
    try {
      blocks = await invoke("process_message_content", { content: text });

      // 3. Post-process blocks for side-effects (CSS injection)
      if (options.messageId) {
        for (const block of blocks) {
          if (block.type === "style") {
            injectScopedCss(block.content, options.messageId);
          }
        }
      }
    } catch (error) {
      console.error(
        "[useContentProcessor] Failed to parse content via Rust:",
        error,
      );
      blocks = [{ type: "markdown", content: text }];
    }

    return blocks;
  };

  return {
    processMessageContent,
    transformSpecialBlocksForStream,
    escapeHtml,
    removeScopedCss,
  };
}

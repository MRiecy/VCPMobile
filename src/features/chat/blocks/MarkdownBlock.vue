<script setup lang="ts">
import { computed, ref, watch, nextTick, onMounted, onUnmounted } from 'vue';
import { Marked } from 'marked';
import { markedHighlight } from 'marked-highlight';
import hljs from 'highlight.js';
import DOMPurify from 'dompurify';
import morphdom from 'morphdom';
import { useDebounceFn } from '@vueuse/core';
// 移除同步导入，改为动态导入
// import mermaid from 'mermaid';
// import katex from 'katex';
// import 'katex/dist/katex.min.css';
import { useVcpMagic } from '../../../core/composables/useVcpMagic';
import { useChatManagerStore } from '../../../core/stores/chatManager';

const props = defineProps<{
  content: string;
  isStreaming?: boolean;
}>();

const markdownContainer = ref<HTMLElement | null>(null);
const innerContentRef = ref<HTMLElement | null>(null);
const isVisible = ref(false);

const { processMagic, cleanupMagic } = useVcpMagic();
const chatStore = useChatManagerStore();

const marked = new Marked(
  markedHighlight({
    emptyLangClass: 'hljs',
    langPrefix: 'hljs language-',
    highlight(code, lang) {
      const language = hljs.getLanguage(lang) ? lang : 'plaintext';
      return hljs.highlight(code, { language }).value;
    }
  })
);

marked.setOptions({
  gfm: true,
  breaks: true,
});

import { convertFileSrc } from '@tauri-apps/api/core';

// Custom renderer for Mermaid and Images
const renderer = {
  code({ text, lang }: { text: string; lang?: string; escaped?: boolean }) {
    if (lang === 'mermaid' || lang === 'flowchart' || lang === 'graph') {
      const encoded = btoa(encodeURIComponent(text));
      return `<div class="mermaid-placeholder" data-code="${encoded}">解析渲染中...</div>`;
    }
    return false; // use default
  },
  image({ href, title, text }: { href: string; title: string | null; text: string }) {
    let finalHref = href;
    // 拦截本地绝对路径，转换为 Tauri asset 协议
    if (href && (href.startsWith('/') || href.match(/^[a-zA-Z]:\\/))) {
      try {
        finalHref = convertFileSrc(href);
      } catch (e) {
        console.warn('Failed to convert image path to asset protocol:', href, e);
      }
    }

    let out = `<img src="${finalHref}" alt="${text}"`;
    if (title) {
      out += ` title="${title}"`;
    }
    out += ' loading="lazy" class="vcp-markdown-image" />';
    return out;
  }
};

// VCP Math Extension for Marked
const mathExtension = {
  extensions: [
    {
      name: 'inlineMath',
      level: 'inline',
      start(src: string) {
        const match = src.match(/\$|\\\(/);
        return match ? match.index : -1;
      },
      tokenizer(src: string) {
        // 匹配 $...$ (非换行，支持 \$ 转义)
        const dollarMatch = src.match(/^\$((?:[^\$\n]|\\\$)+?)\$/);
        if (dollarMatch) return { type: 'inlineMath', raw: dollarMatch[0], text: dollarMatch[1].trim() };

        // 匹配 \(...\)
        const parenMatch = src.match(/^\\\(([\s\S]+?)\\\)/);
        if (parenMatch) return { type: 'inlineMath', raw: parenMatch[0], text: parenMatch[1].trim() };
      },
      renderer(token: any) {
        return `<span class="math-inline">${token.text}</span>`;
      }
    },
    {
      name: 'blockMath',
      level: 'block',
      start(src: string) {
        const match = src.match(/\$\$|\\\[|\\begin/);
        return match ? match.index : -1;
      },
      tokenizer(src: string) {
        // 匹配 $$...$$ (允许前置空格)
        const dollarMatch = src.match(/^ *\$\$([\s\S]+?)\$\$/);
        if (dollarMatch) return { type: 'blockMath', raw: dollarMatch[0], text: dollarMatch[1].trim() };

        // 匹配 \[...\]
        const bracketMatch = src.match(/^ *\\\[([\s\S]+?)\\\]/);
        if (bracketMatch) return { type: 'blockMath', raw: bracketMatch[0], text: bracketMatch[1].trim() };

        // 匹配 \begin{...}...\end{...}
        const envMatch = src.match(/^ *\\begin\{([a-z]*\*?)\}([\s\S]+?)\\end\{\1\}/);
        if (envMatch) return { type: 'blockMath', raw: envMatch[0], text: envMatch[0].trim() };
      },
      renderer(token: any) {
        return `<div class="language-math">${token.text}</div>`;
      }
    }
  ]
};

marked.use({ renderer });
marked.use(mathExtension);

// Sanitize HTML with DOMPurify
const renderedHtml = computed(() => {
  const rawHtml = marked.parse(props.content) as string;
  // 保持安全过滤，但由于纠错已在 Rust 完成，不再需要放行 onerror
  return DOMPurify.sanitize(rawHtml, {
    ADD_TAGS: [
      'iframe', 'canvas', 'script', 'style', 'button', 'img',
      'svg', 'circle', 'line', 'text', 'animate', 'defs', 'linearGradient', 
      'stop', 'filter', 'feDropShadow', 'path', 'g', 'polyline', 'polygon', 'rect',
      'table', 'thead', 'tbody', 'tr', 'th', 'td'
    ],
    ADD_ATTR: [
      'allow', 'allowfullscreen', 'frameborder', 'scrolling',
      'data-send', 'data-vcp-interactive', 'data-vcp-scoped',
      'class', 'width', 'height',
      'viewBox', 'fill', 'stroke', 'stroke-width', 'cx', 'cy', 'r', 'x', 'y', 
      'x1', 'y1', 'x2', 'y2', 'd', 'filter', 'attributeName', 'from', 'to', 
      'begin', 'dur', 'dx', 'dy', 'stdDeviation', 'flood-color', 'flood-opacity', 
      'offset', 'stop-color', 'text-anchor', 'opacity', 'style', 'id'
    ],
    FORCE_BODY: true
  });
});

const updateDOM = () => {
  if (innerContentRef.value) {
    morphdom(innerContentRef.value, `<div class="vcp-markdown-inner">${renderedHtml.value}</div>`, {
      childrenOnly: false,
      onBeforeElUpdated: function (fromEl, toEl) {
        if (fromEl.isEqualNode(toEl)) return false;

        // Preserve VCP injected things
        if (fromEl.classList && fromEl.classList.contains('mermaid') && fromEl.tagName === 'DIV') {
          return false; // Don't overwrite rendered mermaid
        }
        if (fromEl.classList && (fromEl.classList.contains('language-math') || fromEl.classList.contains('math-inline')) && fromEl.querySelector('.katex')) {
          return false; // Don't overwrite rendered katex
        }

        return true;
      }
    });
  }
};

// 节流处理复杂的 DOM 操作（Magic 和 重度渲染）
const debouncedProcessMagic = useDebounceFn(() => {
  if (innerContentRef.value) {
    const scopeId = (markdownContainer.value?.closest('.vcp-message-item') as HTMLElement)?.dataset.messageId
      || Math.random().toString(36).substring(2, 9);
    processMagic(innerContentRef.value, scopeId);
    if (isVisible.value) {
      renderHeavyContent();
    }
  }
}, 400);

// 监听渲染内容变化
watch(() => renderedHtml.value, async () => {
  updateDOM();

  if (props.isStreaming) {
    debouncedProcessMagic();
  } else {
    await nextTick();
    const scopeId = (markdownContainer.value?.closest('.vcp-message-item') as HTMLElement)?.dataset.messageId
      || Math.random().toString(36).substring(2, 9);
    if (innerContentRef.value) {
      processMagic(innerContentRef.value, scopeId);
      if (isVisible.value) {
        renderHeavyContent();
      }
    }
  }
});

onMounted(async () => {
  updateDOM();
  if (!props.isStreaming && innerContentRef.value) {
    await nextTick();
    const scopeId = (markdownContainer.value?.closest('.vcp-message-item') as HTMLElement)?.dataset.messageId
      || Math.random().toString(36).substring(2, 9);
    processMagic(innerContentRef.value, scopeId);
  }
});

onUnmounted(() => {
  if (innerContentRef.value) {
    cleanupMagic(innerContentRef.value);
  }
});

const renderHeavyContent = async () => {
  if (!innerContentRef.value || !isVisible.value) return;

  await nextTick();

  // 1. Render KaTeX (Lazy Load)
  const texElements = innerContentRef.value.querySelectorAll('.language-math, .math-inline');
  if (texElements.length > 0) {
    try {
      // 动态导入 KaTeX 及其样式
      const [katexModule] = await Promise.all([
        import('katex'),
        import('katex/dist/katex.min.css')
      ]);
      const katex = katexModule.default;
      (window as any).katex = katex; // 挂载到全局以便调试和兼容性

      texElements.forEach(el => {
        if (el.querySelector('.katex')) return; // Already rendered
        const isBlock = el.classList.contains('language-math');
        try {
          katex.render(el.textContent || '', el as HTMLElement, {
            throwOnError: false,
            displayMode: isBlock
          });
        } catch (e) {
          console.error('KaTeX error:', e);
        }
      });
    } catch (e) {
      console.error('Failed to load KaTeX:', e);
    }
  }

  // 2. Render Mermaid (Lazy Load)
  const placeholders = innerContentRef.value.querySelectorAll('.mermaid-placeholder');
  if (placeholders.length > 0) {
    try {
      // 动态导入 Mermaid
      const mermaidModule = await import('mermaid');
      const mermaid = mermaidModule.default;

      mermaid.initialize({ startOnLoad: false, theme: 'dark' });
      for (const el of Array.from(placeholders)) {
        const placeholder = el as HTMLElement;
        const encoded = placeholder.dataset.code;
        if (!encoded) continue;

        try {
          const code = decodeURIComponent(atob(encoded));
          placeholder.innerHTML = code;
          placeholder.classList.remove('mermaid-placeholder');
          placeholder.classList.add('mermaid');
          await mermaid.run({ nodes: [placeholder] });
        } catch (e) {
          console.error('Mermaid error:', e);
          placeholder.innerHTML = '<div class="text-red-500 text-[10px]">图表渲染失败</div>';
        }
      }
    } catch (e) {
      console.error('Failed to load Mermaid:', e);
    }
  }
};

const handleContainerClick = (e: MouseEvent) => {
  const target = e.target as HTMLElement;
  const button = target.closest('button');

  if (!button || button.dataset.vcpInteractive !== 'true') return;

  e.preventDefault();
  e.stopPropagation();

  if (button.disabled) return;

  const sendText = button.dataset.send || button.textContent?.trim() || '';
  if (!sendText) return;

  let finalSendText = `[[点击按钮:${sendText}]]`;
  if (finalSendText.length > 500) {
    const truncated = sendText.substring(0, 500 - '[[点击按钮:]]'.length);
    finalSendText = `[[点击按钮:${truncated}]]`;
  }

  // Visual feedback
  button.disabled = true;
  button.style.opacity = '0.6';
  button.style.cursor = 'not-allowed';
  const originalText = button.textContent;
  button.textContent = `${originalText} ✓`;

  // Send message
  chatStore.sendMessage(finalSendText);
};

const handleIntersect = () => {
  isVisible.value = true;
  if (!props.isStreaming) {
    renderHeavyContent();
  }
};

watch(() => [props.content, props.isStreaming], () => {
  if (isVisible.value && !props.isStreaming) {
    renderHeavyContent();
  }
});
</script>

<template>
  <div ref="markdownContainer" class="vcp-markdown-block prose prose-sm dark:prose-invert max-w-none"
    v-intersection-observer.once @intersect="handleIntersect" @click="handleContainerClick">
    <div ref="innerContentRef" class="vcp-markdown-inner"></div>
  </div>
</template>

<style>
/* hljs styles should be imported globally or here */
@import 'highlight.js/styles/github-dark.css';

.vcp-markdown-block {
  /* Fix layout thrashing and horizontal overflow */
  word-break: break-word;
  overflow-wrap: break-word;
  min-width: 0;
  max-width: 100%;
}

.vcp-markdown-inner {
  max-width: 100%;
}

/* VCP Role Divide Styles (Ported from VChat) */
.vcp-role-divider {
  display: flex;
  align-items: center;
  justify-content: center;
  margin: 15px 0;
  font-size: 0.85em;
  color: var(--primary-text);
  opacity: 0.7;
  user-select: none;
  clear: both;
}

.vcp-role-divider::before,
.vcp-role-divider::after {
  content: "";
  flex: 1;
  border-bottom: 1px dashed var(--border-color, #ccc);
  margin: 0 15px;
}

.vcp-role-divider.role-system {
  color: #e67e22;
}

.vcp-role-divider.role-assistant {
  color: #3498db;
}

.vcp-role-divider.role-user {
  color: #2ecc71;
}

.vcp-role-divider.type-end {
  opacity: 0.5;
}

.vcp-markdown-block pre {
  @apply rounded-lg bg-gray-900/50 p-3 overflow-x-auto border border-white/10 my-2;
  max-width: 100%;
}

.vcp-markdown-block table {
  display: block;
  max-width: 100%;
  overflow-x: auto;
  white-space: nowrap;
}

.vcp-markdown-block img {
  max-width: 100%;
  height: auto;
}

/* 表情包专属尺寸约束：使用 Rust 注入的 .vcp-emoticon 类名 */
.vcp-markdown-block .vcp-emoticon {
  max-width: 110px;
  max-height: 110px;
  display: inline-block;
  vertical-align: middle;
  margin: 4px;
  border-radius: 8px;
  border: 1px solid rgba(255, 255, 255, 0.05);
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.1);
  /* 平滑显示效果 */
  transition: all 0.3s ease;
}

.vcp-markdown-block .vcp-emoticon:hover {
  transform: scale(1.05);
  border-color: rgba(52, 152, 219, 0.3);
}

.vcp-markdown-block code {
  @apply font-mono text-sm;
  word-break: break-word;
}

/* 修复：Tailwind Prose 默认会给 code 加上多余的字符，必须强行去除 */
.vcp-markdown-block code::before,
.vcp-markdown-block code::after {
  content: none !important;
}

/* 修复：恢复经典的 MD 行内代码样式，并为亮色模式注入半透明质感底色 */
.vcp-markdown-block code:not(pre code) {
  color: #c7254e;
  /* 亮色模式下使用带 8% 透明度的专属粉红底色，避免纯白背景带来的突兀感 */
  background-color: rgba(199, 37, 78, 0.08);
  padding: 0.2em 0.4em;
  border-radius: 4px;
  font-size: 0.9em;
  border: none;
}

.dark .vcp-markdown-block code:not(pre code) {
  color: #ff8b8b;
  /* 暗黑模式下使用高级的透灰底色 */
  background-color: rgba(255, 255, 255, 0.1);
}

/* 修复：VCP 专属引号高亮样式 (移除固定的 font-weight，允许与加粗 ** 完美嵌套) */
.highlighted-quote {
  color: var(--quoted-text, #ff7f50) !important;
  display: inline !important;
  word-break: break-all;
}

/* VCP 专属 Markdown 原生标签色彩劫持 (仅限标题，加粗保持原色以维持视觉克制) */
.vcp-markdown-block h1,
.vcp-markdown-block h2,
.vcp-markdown-block h3,
.vcp-markdown-block h4,
.vcp-markdown-block h5,
.vcp-markdown-block h6 {
  color: var(--highlight-text, #3498db) !important;
}

/* VCP 专属标签高亮样式 (@标签 和 @!警告) */
.highlighted-tag {
  color: var(--highlight-text, #3498db) !important;
  font-weight: 600;
  display: inline !important;
}

.highlighted-alert-tag {
  color: var(--danger-color, #e74c3c) !important;
  font-weight: bold;
  display: inline !important;
}

.vcp-markdown-block p {
  @apply mb-2 last:mb-0;
}

/* 优化嵌套时的边距累加问题 */
.vcp-markdown-block>.vcp-markdown-inner> :first-child {
  margin-top: 0 !important;
}

.vcp-markdown-block>.vcp-markdown-inner> :last-child {
  margin-bottom: 0 !important;
}

.vcp-markdown-block ul,
.vcp-markdown-block ol {
  padding-left: 1.2em !important;
  margin-top: 0.5em !important;
  margin-bottom: 0.5em !important;
}

/* 修复：超长公式截断问题（为 KaTeX 公式容器分配独立的横向滚动上下文） */
.vcp-markdown-block .vcp-math-block,
.vcp-markdown-block .katex-display {
  max-width: 100%;
  overflow-x: auto;
  overflow-y: hidden;
  -webkit-overflow-scrolling: touch;
}

.vcp-markdown-block .katex-display {
  padding-bottom: 0.5em;
  /* 防止垂直截断遮挡下标或滚动条 */
}

.vcp-markdown-block .vcp-math-inline {
  max-width: 100%;
  overflow-x: auto;
  overflow-y: hidden;
  display: inline-block;
  vertical-align: middle;
}

/* 匹配整体美学的细长公式滚动条 */
.vcp-markdown-block .vcp-math-block::-webkit-scrollbar,
.vcp-markdown-block .katex-display::-webkit-scrollbar,
.vcp-markdown-block .vcp-math-inline::-webkit-scrollbar {
  height: 4px;
}

.vcp-markdown-block .vcp-math-block::-webkit-scrollbar-thumb,
.vcp-markdown-block .katex-display::-webkit-scrollbar-thumb,
.vcp-markdown-block .vcp-math-inline::-webkit-scrollbar-thumb {
  background: rgba(150, 150, 150, 0.3);
  border-radius: 4px;
}

/* 强化 Emoji 字体栈，强制手机端渲染更精美的原生彩色表情 */
.vcp-markdown-block {
  font-family: inherit;
}
.vcp-markdown-block,
.vcp-markdown-block * {
  font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif, "Apple Color Emoji", "Segoe UI Emoji", "Segoe UI Symbol", "Noto Color Emoji";
}

/* 移动端强力防护：防止硬编码的内联样式撑爆屏幕或导致排版面条化 */
@media (max-width: 768px) {
  /* 强制覆盖固定宽度，确保不会超出屏幕 */
  .vcp-markdown-block [style*="width"] {
    max-width: 100% !important;
    min-width: 0 !important;
  }
  
  /* 强制将硬编码的 grid 转为单列或允许 flex 换行 */
  .vcp-markdown-block [style*="display: grid"],
  .vcp-markdown-block [style*="display:grid"] {
    grid-template-columns: 1fr !important;
  }
  .vcp-markdown-block [style*="display: flex"],
  .vcp-markdown-block [style*="display:flex"] {
    flex-wrap: wrap !important;
  }

  /* 防止 SVG 因为硬编码的宽高过大而超出容器 */
  .vcp-markdown-block svg {
    max-width: 100% !important;
    height: auto !important;
  }

  /* 为带有固定宽度的内联元素提供一个横向滚动安全网 */
  .vcp-markdown-block > .vcp-markdown-inner > div[style] {
    overflow-x: auto;
    -webkit-overflow-scrolling: touch;
  }
}
</style>
p-markdown-block svg {
    max-width: 100% !important;
    height: auto !important;
  }

  /* 为带有固定宽度的内联元素提供一个横向滚动安全网 */
  .vcp-markdown-block > .vcp-markdown-inner > div[style] {
    overflow-x: auto;
    -webkit-overflow-scrolling: touch;
  }
}
</style>

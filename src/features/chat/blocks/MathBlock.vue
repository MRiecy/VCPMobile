<script setup lang="ts">
import { ref, watch, computed, nextTick } from 'vue';
import type { ContentBlock } from '../../../core/composables/useContentProcessor';

const props = defineProps<{
  content: string;
  block: ContentBlock;
  isStreaming?: boolean;
}>();

const katexEl = ref<HTMLElement | null>(null);
const isRendered = ref(false);

// 检测公式内容是否看起来完整（有闭合标记），不完整时跳过 KaTeX 渲染
const looksComplete = computed(() => {
  const c = props.content.trim();
  if (!c) return false;
  // block math 需要闭合标记
  return c.endsWith('$$') || c.endsWith('\\]') || /\\end\{[a-z]+\*?\}$/.test(c);
});

const renderKatex = async () => {
  if (!katexEl.value || isRendered.value || !props.content) return;
  // 流式模式下，不完整的公式暂不做 KaTeX 渲染，避免报错和闪烁
  if (props.isStreaming && !looksComplete.value) return;

  try {
    const [katexModule] = await Promise.all([
      import('katex'),
      import('katex/dist/katex.min.css')
    ]);
    const katex = katexModule.default;

    katex.render(props.content, katexEl.value, {
      displayMode: props.block.display_mode !== false,
      throwOnError: false,
      strict: false,
    });
    isRendered.value = true;
  } catch (e) {
    console.error('MathBlock KaTeX error:', e);
  }
};

// 监听内容变化：重置渲染状态并尝试重新渲染（用于流式更新）
watch(() => props.content, async () => {
  isRendered.value = false;
  await nextTick();
  renderKatex();
});
</script>

<template>
  <div
    class="math-block-container my-3 overflow-x-auto"
    v-intersection-observer.once
    @intersect="renderKatex"
  >
    <div ref="katexEl" class="vcp-math-block text-xs opacity-40 font-mono">{{ content }}</div>
  </div>
</template>

<style>
.math-block-container {
  max-width: 100%;
  -webkit-overflow-scrolling: touch;
}

.math-block-container .katex-display {
  margin: 0.5em 0;
}

.math-block-container::-webkit-scrollbar {
  height: 4px;
}

.math-block-container::-webkit-scrollbar-thumb {
  background: rgba(150, 150, 150, 0.3);
  border-radius: 4px;
}

/* 从 MarkdownBlock 迁移：block math 的溢出保护与滚动条样式 */
.vcp-math-block {
  max-width: 100%;
  overflow-x: auto;
  overflow-y: hidden;
  -webkit-overflow-scrolling: touch;
}

.vcp-math-block .katex-display {
  padding-bottom: 0.5em;
  /* 防止垂直截断遮挡下标或滚动条 */
}

.vcp-math-block::-webkit-scrollbar,
.vcp-math-block .katex-display::-webkit-scrollbar {
  height: 4px;
}

.vcp-math-block::-webkit-scrollbar-thumb,
.vcp-math-block .katex-display::-webkit-scrollbar-thumb {
  background: rgba(150, 150, 150, 0.3);
  border-radius: 4px;
}
</style>

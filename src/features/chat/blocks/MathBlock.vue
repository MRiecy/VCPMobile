<script setup lang="ts">
import { ref } from 'vue';
import type { ContentBlock } from '../../../core/composables/useContentProcessor';

const props = defineProps<{
  content: string;
  block: ContentBlock;
}>();

const katexEl = ref<HTMLElement | null>(null);
const isRendered = ref(false);

const renderKatex = async () => {
  if (!katexEl.value || isRendered.value || !props.content) return;

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
</script>

<template>
  <div
    class="math-block-container my-3 overflow-x-auto"
    v-intersection-observer.once
    @intersect="renderKatex"
  >
    <div ref="katexEl" class="text-xs opacity-40 font-mono">{{ content }}</div>
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
</style>

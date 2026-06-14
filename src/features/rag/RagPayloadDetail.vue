<script setup lang="ts">
import { computed } from 'vue';
import { marked } from 'marked';

// Configure marked to support Github Flavored Markdown & breaks
marked.setOptions({
  gfm: true,
  breaks: true,
});

interface Props {
  text: string;
  isQuery?: boolean;
}

const props = withDefaults(defineProps<Props>(), {
  isQuery: false,
});

const renderedHtml = computed(() => {
  let rawText = props.text || '';
  
  if (props.isQuery) {
    // 转义特殊 HTML 字符，防止在 v-html 渲染 query/response 文本时因浏览器误判 <Tauri> 等标签而吞字
    rawText = rawText.replace(/</g, '&lt;').replace(/>/g, '&gt;');
  }

  try {
    // 修复 Markdown 引擎将 "[AI]:" 或 "[USER]:" 识别为隐藏链接定义（Link Reference Definition）从而吞字的 Bug
    const safeText = rawText.replace(/^(\s*)\[([^\]]+)\]:/gm, '$1\\[$2\\]:');
    return marked.parse(safeText) as string;
  } catch (e) {
    console.error('[RagPayloadDetail] marked parse failed:', e);
    return rawText;
  }
});
</script>

<template>
  <div v-html="renderedHtml"></div>
</template>

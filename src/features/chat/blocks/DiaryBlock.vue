<script setup lang="ts">
import MarkdownBlock from './MarkdownBlock.vue';
import type { ContentBlock } from '../../../core/composables/useContentProcessor';

defineProps<{
  content: string;
  block: ContentBlock;
}>();
</script>

<template>
  <div class="maid-diary-bubble">
    <div class="diary-header">
      <span class="diary-title">Maid's Diary</span>
      <span v-if="block.date" class="diary-date">{{ block.date }}</span>
    </div>

    <div v-if="block.maid" class="diary-maid-info">
      <span class="diary-maid-label">Maid:</span>
      <span class="diary-maid-name">{{ block.maid }}</span>
    </div>

    <div class="diary-content">
      <MarkdownBlock :content="content" />
    </div>
  </div>
</template>

<style scoped>
.maid-diary-bubble {
    background: #fdfaf6 !important; /* A very light, warm parchment color */
    border: 1px solid #eaddd0; /* A soft, paper-like border */
    border-radius: 8px !important;
    padding: 12px 14px 15px 36px !important; /* Adjusted padding for new icon placement */
    color: #5d4037 !important; /* A warm, dark brown for text */
    box-shadow: 0 3px 8px rgba(0, 0, 0, 0.08);
    margin-top: 10px !important;
    margin-bottom: 12px !important;
    position: relative;
    overflow: visible; /* Allow for potential pseudo-elements to peek out */
    line-height: 1.7;
    font-family: 'Georgia', 'Times New Roman', serif !important;
}

/* The decorative icon on the top left, changed to a quill */
.maid-diary-bubble::before {
    content: "✒️";
    position: absolute;
    top: 14px;
    left: 12px;
    font-size: 18px;
    opacity: 0.6;
    z-index: 2;
    transform: rotate(0deg); /* Fixed tilt */
}

.diary-header {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    border-bottom: 1px solid #d7ccc8;
    padding-bottom: 6px;
    margin-bottom: 10px;
}

.diary-title {
    font-weight: bold;
    font-size: 1.1em;
    color: #6d4c41;
    font-family: 'Georgia', 'Times New Roman', serif !important;
}

.diary-date {
    font-size: 0.85em;
    color: #a1887f;
    font-style: italic;
}

.diary-maid-info {
    margin-bottom: 12px;
    font-size: 0.9em;
    color: #8d6e63;
}

.diary-maid-name {
    font-style: italic;
    color: #a1887f;
    background: rgba(161, 136, 127, 0.08);
    padding: 1px 5px;
    border-radius: 4px;
}

.diary-content {
    font-size: 0.95em;
    color: #4e342e;
    white-space: normal;
    word-break: break-word;
}

/* --- Maid Diary Dark Theme Overrides --- */
html.dark .maid-diary-bubble {
    background: #2c2a3a !important; /* A deep, slightly magical purple-gray */
    border-color: #4a485f;
    color: #d8d2c9 !important; /* A soft, parchment-like off-white */
    box-shadow: 0 3px 10px rgba(0, 0, 0, 0.25);
}

html.dark .diary-header {
    border-bottom-color: #4a485f;
}

html.dark .diary-title {
    color: #ffb74d; /* A warm, golden-orange for the title */
}

html.dark .diary-date {
    color: #a0a0b0;
}

html.dark .diary-maid-info {
    color: #b0bec5;
}

html.dark .diary-maid-name {
    color: #ffcc80;
    background: rgba(255, 183, 77, 0.1);
}

html.dark .diary-content {
    color: #c5c8c6;
}
</style>

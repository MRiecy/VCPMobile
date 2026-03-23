<script setup lang="ts">
import { ref } from 'vue';
import { ChevronDown, ChevronUp, Loader2 } from 'lucide-vue-next';
import MarkdownBlock from './MarkdownBlock.vue';
import type { ContentBlock } from '../../composables/useContentProcessor';

defineProps<{
  content: string;
  block: ContentBlock;
}>();

const isExpanded = ref(false);

const toggleExpand = () => {
  isExpanded.value = !isExpanded.value;
};
</script>

<template>
  <div class="vcp-thought-block">
    <div class="thought-header" @click="toggleExpand">
      <span class="thought-icon">🧠</span>
      <span class="thought-label flex items-center gap-1">
        {{ block.theme || '元思考链' }}
        <Loader2 v-if="!block.is_complete" :size="10" class="animate-spin" />
      </span>
      <component :is="isExpanded ? ChevronUp : ChevronDown" :size="14" class="opacity-40 ml-auto" />
    </div>

    <div v-show="isExpanded" class="thought-content animate-slide-down">
      <div class="thought-body">
        <MarkdownBlock :content="content" />
      </div>
    </div>
  </div>
</template>

<style scoped>
.vcp-thought-block {
    background: rgba(0, 0, 0, 0.03) !important;
    border-radius: 12px !important;
    border: 1px solid rgba(0, 0, 0, 0.1);
    margin: 10px 0 !important;
    position: relative;
    font-size: 0.92em !important;
    line-height: 1.6;
    width: fit-content;
    max-width: 98%;
    transition: all 0.3s ease;
}

html.dark .vcp-thought-block {
    background: rgba(120, 120, 128, 0.05) !important;
    border-color: rgba(120, 120, 128, 0.2);
}

.thought-header {
    display: flex;
    align-items: center;
    gap: 8px;
    cursor: pointer;
    user-select: none;
    opacity: 0.8;
    transition: opacity 0.2s;
    padding: 10px 15px !important;
}

.thought-header:hover {
    opacity: 1;
}

.thought-icon {
    font-size: 1.1em;
    filter: grayscale(0.5);
}

.thought-label {
    font-weight: 600;
    font-size: 0.95em;
}

.thought-content {
    padding: 0 15px 10px 15px;
    border-top: 1px dashed rgba(120, 120, 128, 0.2);
    margin-top: 5px;
    padding-top: 10px;
}

.thought-body {
    font-style: italic;
    opacity: 0.8;
}

.animate-slide-down {
  animation: slideDown 0.3s cubic-bezier(0.4, 0, 0.2, 1);
}
@keyframes slideDown {
  from { opacity: 0; transform: translateY(-10px); }
  to { opacity: 1; transform: translateY(0); }
}
</style>

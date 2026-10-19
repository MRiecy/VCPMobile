<script setup lang="ts">
import { ref, useId } from "vue";
import { ChevronDown, AudioLines } from "lucide-vue-next";
import type { ContentBlock } from "../../../core/types/chat";

defineProps<{
  block: Extract<ContentBlock, { type: "voice-guidance" }>;
}>();

const expanded = ref(false);
const contentId = useId();
</script>

<template>
  <div class="voice-guidance-block">
    <button
      type="button"
      class="voice-guidance-toggle"
      data-longpress-native
      :aria-expanded="expanded"
      :aria-controls="contentId"
      @click.stop="expanded = !expanded"
    >
      <AudioLines :size="15" aria-hidden="true" />
      <span>语音指导</span>
      <span v-if="!block.is_complete" class="voice-guidance-status">未完成</span>
      <ChevronDown
        :size="14"
        class="voice-guidance-chevron"
        :class="{ 'is-expanded': expanded }"
        aria-hidden="true"
      />
    </button>
    <div
      v-show="expanded"
      :id="contentId"
      class="voice-guidance-content"
      data-message-content
      data-longpress-native
    >{{ block.content || '未生成指导内容' }}</div>
  </div>
</template>

<style scoped>
.voice-guidance-block {
  color: color-mix(in srgb, currentColor 75%, transparent);
  font-size: 12px;
}

.voice-guidance-toggle {
  display: flex;
  align-items: center;
  gap: 7px;
  width: 100%;
  min-height: 44px;
  padding: 0 2px;
  color: inherit;
  text-align: left;
}

.voice-guidance-toggle:active {
  opacity: 0.65;
}

.voice-guidance-status {
  font-size: 11px;
  opacity: 0.65;
}

.voice-guidance-chevron {
  margin-left: auto;
  opacity: 0.5;
  transition: transform 160ms ease;
}

.voice-guidance-chevron.is-expanded {
  transform: rotate(180deg);
}

.voice-guidance-content {
  padding: 0 2px 8px 24px;
  line-height: 1.65;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
  -webkit-user-select: text;
  user-select: text;
}

@media (prefers-reduced-motion: reduce) {
  .voice-guidance-chevron {
    transition: none;
  }
}
</style>

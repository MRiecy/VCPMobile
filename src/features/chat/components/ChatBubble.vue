<script setup lang="ts">
import { computed } from 'vue';

const props = defineProps<{
  isUser: boolean;
  isStreaming: boolean;
  bubbleStyle?: Record<string, string>;
}>();

const mergedStyle = computed(() => {
  return props.bubbleStyle || {};
});
</script>

<template>
  <div class="vcp-chat-bubble w-full min-w-0 flex flex-col" :data-message-role="isUser ? 'user' : 'agent'" :class="[
    isUser ? 'items-end' : 'items-start',
    isStreaming ? 'streaming' : '',
  ]">
    <div
      class="vcp-bubble-container message-bubble rounded-2xl transition-all duration-300 relative min-w-[60px] min-h-[36px]"
      :class="[
        isUser ? 'p-3 w-fit max-w-[85%] vcp-bubble-user' : 'p-1.5 w-fit max-w-[100%] min-w-[1rem] vcp-bubble-agent'
      ]" :style="mergedStyle">
      <slot />
    </div>

    <slot name="footer" />
  </div>
</template>

<style scoped>
.vcp-bubble-container {
  position: relative;
  word-break: break-word;
}

.vcp-bubble-container::after {
  content: "";
  position: absolute;
  inset: 0;
  border-radius: inherit;
  /* 优化：减小阴影模糊半径，降低渲染复杂度 */
  box-shadow: 0 2px 8px -4px var(--dynamic-color, transparent);
  opacity: 0.15;
  pointer-events: none;
}

.streaming .vcp-bubble-container {
  border-color: var(--vcp-highlight-border-40, var(--highlight-text, #3b82f6));
}
</style>

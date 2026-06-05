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
  <div class="w-full min-w-0 flex flex-col" :class="[
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

@keyframes vcp-border-flow {
  0% {
    background-position: 0% 50%;
  }

  100% {
    background-position: 200% 50%;
  }
}

.streaming .vcp-bubble-container::before {
  content: "";
  position: absolute;
  inset: -1px; /* 减薄边框 */
  padding: 1px;
  border-radius: inherit;
  /* 优化：使用更简单的渐变，减少插值计算 */
  background: linear-gradient(90deg,
      transparent 25%,
      var(--highlight-text, #3b82f6) 50%,
      transparent 75%);
  background-size: 200% 100%;
  -webkit-mask:
    linear-gradient(#fff 0 0) content-box,
    linear-gradient(#fff 0 0);
  -webkit-mask-composite: xor;
  mask-composite: exclude;
  animation: vcp-border-flow 4s linear infinite; /* 减慢动画速度 */
  pointer-events: none;
  z-index: 1;
  opacity: 0.6; /* 降低透明度 */
  /* 强制提升为独立的 GPU 合成层，阻断重绘污染 */
  will-change: transform, opacity;
  transform: translate3d(0, 0, 0);
}
</style>

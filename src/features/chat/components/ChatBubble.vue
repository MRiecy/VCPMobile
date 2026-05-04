<script setup lang="ts">
import { computed } from 'vue';

const props = defineProps<{
  isUser: boolean;
  isStreaming: boolean;
  bubbleStyle?: Record<string, string>;
}>();

const mergedStyle = computed(() => {
  const base = props.bubbleStyle || {};
  if (props.isStreaming) {
    return {
      ...base,
      backdropFilter: 'none',
      WebkitBackdropFilter: 'none',
    };
  }
  return base;
});
</script>

<template>
  <div class="w-full min-w-0 flex flex-col" :class="[
    isUser ? 'items-end' : 'items-start',
    isStreaming ? 'streaming' : '',
  ]">
    <div
      class="vcp-bubble-container rounded-2xl transition-all duration-300 relative backdrop-blur-md min-w-[60px] min-h-[36px]"
      :class="isUser
          ? 'p-3 w-fit max-w-[85%] shadow-sm'
          : 'p-1.5 w-fit max-w-[100%] min-w-0 shadow-sm'
        " :style="mergedStyle">
      <slot />
    </div>

    <slot name="footer" />
  </div>
</template>

<style scoped>
.vcp-bubble-container {
  word-break: break-word;
  backdrop-filter: blur(8px) saturate(110%);
  -webkit-backdrop-filter: blur(8px) saturate(110%);
  transform: translateZ(0);
}

.vcp-bubble-container::after {
  content: "";
  position: absolute;
  inset: 0;
  border-radius: inherit;
  box-shadow: 0 4px 15px -5px var(--dynamic-color, transparent);
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
  inset: -2px;
  padding: 2px;
  border-radius: inherit;
  background: linear-gradient(90deg,
      transparent 0%,
      var(--highlight-text, #3b82f6) 50%,
      transparent 100%);
  background-size: 200% 100%;
  -webkit-mask:
    linear-gradient(#fff 0 0) content-box,
    linear-gradient(#fff 0 0);
  -webkit-mask-composite: xor;
  mask-composite: exclude;
  animation: vcp-border-flow 3s linear infinite;
  pointer-events: none;
  z-index: 1;
  opacity: 0.8;
}
</style>

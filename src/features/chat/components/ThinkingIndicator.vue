<template>
  <div class="flex items-center gap-2 py-1 px-2.5">
    <div class="thinking-dots flex gap-1">
      <span></span>
      <span></span>
      <span></span>
    </div>
    <span class="text-xs italic opacity-70">正在思考...</span>
  </div>
</template>

<style scoped>
.thinking-dots span {
  width: 4px;
  height: 4px;
  border-radius: 50%;
  background-color: currentColor;
  opacity: 0.2;
  animation: vcp-dot-pulse 1.4s infinite ease-in-out;
  /* 提升至 GPU 独立合成层 */
  will-change: transform, opacity;
  transform: translate3d(0, 0, 0);
}

.thinking-dots span:nth-child(2) {
  animation-delay: 0.2s;
}

.thinking-dots span:nth-child(3) {
  animation-delay: 0.4s;
}

@keyframes vcp-dot-pulse {

  0%,
  100% {
    transform: scale(0.8) translate3d(0, 0, 0);
    opacity: 0.2;
  }

  50% {
    transform: scale(1.2) translate3d(0, 0, 0);
    opacity: 1;
  }
}
</style>

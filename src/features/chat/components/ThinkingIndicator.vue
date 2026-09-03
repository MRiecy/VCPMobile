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
  opacity: 0.25;
  animation: vcp-dot-pulse 900ms ease-in-out 1;
}

.thinking-dots span:nth-child(2) {
  animation-delay: 120ms;
}

.thinking-dots span:nth-child(3) {
  animation-delay: 240ms;
}

@keyframes vcp-dot-pulse {

  0%,
  100% {
    transform: scale(1);
    opacity: 0.25;
  }

  50% {
    transform: scale(1.25);
    opacity: 1;
  }
}
</style>

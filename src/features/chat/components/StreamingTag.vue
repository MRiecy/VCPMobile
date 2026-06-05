<template>
  <div
    class="absolute -bottom-2 right-4 bg-white dark:bg-gray-800 rounded-full px-2 py-0.5 shadow-sm border border-black/5 dark:border-white/10 flex items-center gap-1.5 z-10">
    <div class="w-1 h-1 bg-blue-500 rounded-full custom-ping"></div>
    <span class="text-[9px] font-bold opacity-50 uppercase text-gray-500">Streaming</span>
  </div>
</template>

<style scoped>
.custom-ping {
  animation: vcp-ping 1s cubic-bezier(0, 0, 0.2, 1) infinite;
  /* 强行隔离重绘并提升到合成层 */
  will-change: transform, opacity;
  transform: translate3d(0, 0, 0);
}

@keyframes vcp-ping {
  75%, 100% {
    transform: scale(2) translate3d(0, 0, 0);
    opacity: 0;
  }
}
</style>

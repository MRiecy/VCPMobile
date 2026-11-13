<script setup lang="ts">
import { useNotificationStore } from '../../core/stores/notification';
import ToastItem from './ToastItem.vue';

const store = useNotificationStore();
</script>

<template>
  <div class="vcp-toast-stack fixed left-0 right-0 z-toast pointer-events-none px-4 flex flex-col items-center gap-2.5">
    <TransitionGroup name="toast">
      <ToastItem v-for="toast in store.activeToasts" :key="toast.id" :toast="toast" />
    </TransitionGroup>
  </div>
</template>

<style scoped>
.vcp-toast-stack {
  top: calc(var(--vcp-viewport-top, 0px) + var(--vcp-safe-top, 0px) + 16px);
  left: var(--vcp-viewport-left, 0px);
  width: var(--vcp-viewport-width, 100%);
}

@media (pointer: coarse) {
  .vcp-toast-stack {
    /* Android WindowInsets owns the safe-area value; keep a small visual gap below it. */
    top: calc(var(--vcp-viewport-top, 0px) + var(--vcp-safe-top, 0px) + 12px);
  }
}

.toast-enter-active {
  transition: all 0.5s cubic-bezier(0.18, 0.89, 0.32, 1.28);
}

.toast-leave-active {
  /* 立即释放显示位置，避免退场气泡与新气泡同时堆叠超过上限。 */
  display: none;
}

.toast-enter-from {
  opacity: 0;
  transform: translateY(-40px) scale(0.8);
}

.toast-move {
  transition: transform 0.4s ease;
}
</style>

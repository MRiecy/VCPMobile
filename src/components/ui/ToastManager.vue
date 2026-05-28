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
  top: calc(var(--vcp-safe-top, env(safe-area-inset-top, 0px)) + 16px);
}

@media (pointer: coarse) {
  .vcp-toast-stack {
    /* Android edge-to-edge WebView often reports safe-area as 0, so keep toasts below the status bar. */
    top: calc(max(var(--vcp-safe-top, env(safe-area-inset-top, 0px)), 24px) + 12px);
  }
}

.toast-enter-active {
  transition: all 0.5s cubic-bezier(0.18, 0.89, 0.32, 1.28);
}

.toast-leave-active {
  transition: all 0.4s ease-in;
}

.toast-enter-from {
  opacity: 0;
  transform: translateY(-40px) scale(0.8);
}

.toast-leave-to {
  opacity: 0;
  transform: translateY(-20px) scale(0.9);
}

.toast-move {
  transition: transform 0.4s ease;
}
</style>

<script setup lang="ts">
import { useNotificationStore } from '../../core/stores/notification';
import ToastItem from './ToastItem.vue';

const store = useNotificationStore();
</script>

<template>
  <div class="fixed top-safe left-0 right-0 z-toast pointer-events-none px-4 pt-4 flex flex-col items-center gap-2.5">
    <TransitionGroup name="toast">
      <ToastItem v-for="toast in store.activeToasts" :key="toast.id" :toast="toast" />
    </TransitionGroup>
  </div>
</template>

<style scoped>
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

<script setup lang="ts">
import { X } from 'lucide-vue-next';
import SyncLogBrowserCore from './SyncLogBrowserCore.vue';

defineProps<{
  isOpen: boolean;
}>();

const emit = defineEmits<{
  close: [];
}>();
</script>

<template>
  <Transition name="slide">
    <div v-if="isOpen" class="fixed inset-0 z-[60] flex flex-col bg-[#0a0f14] text-white">
      <!-- Header -->
      <div class="flex items-center justify-between px-4 pt-[calc(env(safe-area-inset-top)+8px)] pb-3 border-b border-white/10">
        <h2 class="text-sm font-bold uppercase tracking-widest">同步日志</h2>
        <button @click="emit('close')" class="p-2 -mr-2 text-gray-400 hover:text-white">
          <X :size="20" />
        </button>
      </div>

      <SyncLogBrowserCore />
    </div>
  </Transition>
</template>

<style scoped>
.slide-enter-active,
.slide-leave-active {
  transition: transform 0.25s ease;
}
.slide-enter-from,
.slide-leave-to {
  transform: translateX(100%);
}
</style>

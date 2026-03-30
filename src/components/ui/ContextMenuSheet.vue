<script setup lang="ts">
import type { OverlayActionItem } from '../../core/types/overlay';

defineProps<{
  isOpen: boolean;
  title?: string;
  actions: OverlayActionItem[];
}>();

const emit = defineEmits(['close', 'action-click']);

const handleBackdropClick = () => {
  emit('close');
};

const handleAction = (action: OverlayActionItem) => {
  if (action.disabled) return;
  action.handler();
  emit('action-click', action);
};
</script>

<template>
  <Transition name="fade">
    <div v-if="isOpen" 
         class="fixed inset-0 bg-black/20 backdrop-blur-[1px] pointer-events-auto z-[100]" 
         @click="handleBackdropClick">
      <div class="absolute left-1/2 bottom-6 -translate-x-1/2 w-[calc(100%-24px)] max-w-sm rounded-3xl border border-black/5 dark:border-white/10 bg-white/92 dark:bg-[#111827]/92 backdrop-blur-xl shadow-2xl overflow-hidden"
           @click.stop>
        <div v-if="title" class="px-5 pt-5 pb-3 border-b border-black/5 dark:border-white/10">
          <h3 class="text-sm font-black tracking-wide">{{ title }}</h3>
        </div>
        <div class="p-2">
          <button v-for="action in actions" :key="action.label"
                  @click="handleAction(action)"
                  :disabled="action.disabled"
                  class="w-full flex items-center gap-3 px-4 py-3 rounded-2xl text-left transition-all"
                  :class="[
                    action.danger ? 'text-red-500 hover:bg-red-500/10' : 'hover:bg-black/5 dark:hover:bg-white/5',
                    action.disabled ? 'opacity-40 cursor-not-allowed' : ''
                  ]">
            <component v-if="action.icon" :is="action.icon" class="w-4 h-4 shrink-0" />
            <span class="text-sm font-semibold">{{ action.label }}</span>
          </button>
        </div>
      </div>
    </div>
  </Transition>
</template>

<style scoped>
.fade-enter-active,
.fade-leave-active {
  transition: opacity 0.25s ease;
}
.fade-enter-from,
.fade-leave-to {
  opacity: 0;
}
</style>

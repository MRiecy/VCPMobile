<script setup lang="ts">
import { computed } from 'vue';
import type { OverlayActionItem } from '../../core/types/overlay';

const props = defineProps<{
  isOpen: boolean;
  title?: string;
  actions: OverlayActionItem[];
}>();

const isSelectionMenu = computed(() => props.actions.some((action) => action.selected !== undefined));

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
  <Teleport to="body">
    <Transition name="fade">
      <div v-if="isOpen" class="fixed inset-0 bg-black/20 pointer-events-auto z-dialog"
        @click="handleBackdropClick">
        <div
          v-guide="'context-menu-sheet'"
          class="absolute left-1/2 -translate-x-1/2 w-[calc(100%-24px)] max-w-sm rounded-3xl border border-black/5 dark:border-white/10 bg-white/92 dark:bg-[#111827]/92 shadow-2xl overflow-hidden"
          :style="{ bottom: 'calc(var(--vcp-safe-bottom, 48px) + 24px)' }"
          role="dialog"
          aria-modal="true"
          :aria-label="title || '操作菜单'"
          @click.stop>
          <div v-if="title" class="px-5 pt-5 pb-3 border-b border-black/5 dark:border-white/10">
            <h3 class="text-sm font-black tracking-wide">{{ title }}</h3>
          </div>
          <div class="p-2" :role="isSelectionMenu ? 'radiogroup' : undefined" :aria-label="isSelectionMenu ? title : undefined">
            <button v-for="action in actions" :key="action.label" @click="handleAction(action)"
              :disabled="action.disabled"
              :role="isSelectionMenu ? 'radio' : undefined"
              :aria-checked="isSelectionMenu ? action.selected === true : undefined"
              :data-selected="isSelectionMenu ? String(action.selected === true) : undefined"
              class="relative min-h-12 w-full flex items-center gap-3 px-4 py-3 rounded-2xl text-left transition-colors" :class="[
                action.danger ? 'text-red-500 hover:bg-red-500/10' : 'hover:bg-black/5 dark:hover:bg-white/5',
                action.disabled ? 'opacity-40 cursor-not-allowed' : '',
                action.selected ? 'bg-black/5 dark:bg-white/5' : ''
              ]">
              <span v-if="action.selected" class="absolute left-0 top-2 bottom-2 w-0.5 rounded-full bg-[var(--highlight-text)]" aria-hidden="true"></span>
              <component v-if="action.icon" :is="action.icon" class="w-4 h-4 shrink-0" />
              <span class="text-sm font-semibold flex-1">{{ action.label }}</span>
              <svg v-if="action.selected" class="w-4 h-4 shrink-0 text-[var(--highlight-text)]" viewBox="0 0 24 24" fill="none"
                stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                <polyline points="20 6 9 17 4 12"></polyline>
              </svg>
            </button>
          </div>
        </div>
      </div>
    </Transition>
  </Teleport>
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

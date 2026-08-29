<script setup lang="ts">
import { computed } from 'vue';
import type { OverlayActionItem } from '../../core/types/overlay';

const props = defineProps<{
  isOpen: boolean;
  title?: string;
  actions: OverlayActionItem[];
  headerAction?: OverlayActionItem;
}>();

const isSelectionMenu = computed(() => props.actions.some((action) => action.selected !== undefined));

const emit = defineEmits(['close', 'action-click', 'header-action-click']);

const handleBackdropClick = () => {
  emit('close');
};

const handleAction = (action: OverlayActionItem) => {
  if (action.disabled) return;
  action.handler();
  emit('action-click', action);
};

const handleHeaderAction = () => {
  const action = props.headerAction;
  if (!action || action.disabled) return;
  action.handler();
  emit('header-action-click', action);
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
          <div v-if="title" class="min-h-14 px-5 py-3 border-b border-black/5 dark:border-white/10 flex items-center justify-between gap-3">
            <h3 class="text-sm font-black tracking-wide">{{ title }}</h3>
            <button
              v-if="headerAction"
              type="button"
              class="min-h-9 px-3 flex items-center gap-1.5 rounded-lg border text-xs font-bold transition-colors"
              :class="[
                headerAction.selected
                  ? 'border-[var(--highlight-text)] text-[var(--highlight-text)] bg-[var(--vcp-highlight-bg-10)]'
                  : 'border-black/10 dark:border-white/10 hover:bg-black/5 dark:hover:bg-white/5',
                headerAction.disabled ? 'opacity-40 cursor-not-allowed' : '',
              ]"
              :disabled="headerAction.disabled"
              :aria-pressed="headerAction.selected === undefined ? undefined : headerAction.selected"
              @click="handleHeaderAction"
            >
              <component v-if="headerAction.icon" :is="headerAction.icon" class="w-3.5 h-3.5 shrink-0" />
              <span>{{ headerAction.label }}</span>
            </button>
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

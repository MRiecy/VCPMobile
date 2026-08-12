<script setup lang="ts">
import { ChevronRight, FilePenLine, FolderInput, Trash2, X } from "lucide-vue-next";

export interface DiarySheetAction {
  id: string;
  label: string;
  detail?: string;
  danger?: boolean;
  disabled?: boolean;
}

defineProps<{
  open: boolean;
  title: string;
  actions: DiarySheetAction[];
}>();

const emit = defineEmits<{
  close: [];
  action: [id: string];
}>();
</script>

<template>
  <Transition name="diary-sheet">
    <div v-if="open" class="fixed inset-0 z-sheet no-swipe">
      <button type="button" class="absolute inset-0 w-full h-full border-0 bg-black/45" aria-label="关闭操作面板" @click="emit('close')" />
      <section
        class="diary-action-sheet absolute inset-x-0 bottom-0 text-[var(--primary-text)] pb-[var(--vcp-safe-bottom,48px)]"
        style="left: var(--vcp-safe-left, 0px); right: var(--vcp-safe-right, 0px)"
        role="dialog"
        aria-modal="true"
        :aria-label="title"
      >
        <span class="diary-sheet-grabber" aria-hidden="true" />
        <header class="diary-action-header">
          <div class="min-w-0 flex-1">
            <span class="diary-action-eyebrow">VCP MEMO · ACTIONS</span>
            <h2>{{ title }}</h2>
          </div>
          <button type="button" class="diary-action-close" aria-label="关闭" @click="emit('close')">
            <X :size="19" />
          </button>
        </header>
        <div class="diary-action-list">
          <button
            v-for="action in actions"
            :key="action.id"
            type="button"
            class="diary-action-row"
            :class="action.danger ? 'danger' : ''"
            :disabled="action.disabled"
            @click="emit('action', action.id)"
          >
            <span class="diary-action-icon" aria-hidden="true">
              <FilePenLine v-if="action.id === 'rename'" :size="17" />
              <FolderInput v-else-if="action.id === 'move'" :size="17" />
              <Trash2 v-else-if="action.id === 'delete'" :size="17" />
              <ChevronRight v-else :size="17" />
            </span>
            <span class="diary-action-copy">
              <strong>{{ action.label }}</strong>
              <span v-if="action.detail">{{ action.detail }}</span>
            </span>
            <ChevronRight :size="16" class="shrink-0 opacity-45" />
          </button>
        </div>
      </section>
    </div>
  </Transition>
</template>

<style scoped>
.diary-action-sheet {
  overflow: hidden;
  border: 1px solid var(--diary-line);
  border-bottom: 0;
  border-radius: 18px 18px 0 0;
  background: var(--primary-bg);
}

.diary-sheet-grabber {
  width: 34px;
  height: 4px;
  margin: 8px auto 0;
  display: block;
  border-radius: 999px;
  background: var(--diary-muted-surface);
}

.diary-action-header {
  min-height: 60px;
  box-sizing: border-box;
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 6px 10px 6px 16px;
  border-bottom: 1px solid var(--diary-line);
}

.diary-action-header h2 {
  margin: 2px 0 0;
  font-size: 15px;
  font-weight: 700;
}

.diary-action-eyebrow {
  display: block;
  color: var(--diary-muted-text);
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 9px;
  font-weight: 700;
  letter-spacing: 0.14em;
}

.diary-action-close {
  width: 48px;
  height: 48px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border: 0;
  border-radius: 12px;
  background: transparent;
  color: var(--primary-text);
}

.diary-action-list {
  padding: 6px 10px 10px;
}

.diary-action-row {
  width: 100%;
  min-height: 64px;
  box-sizing: border-box;
  display: flex;
  align-items: center;
  gap: 11px;
  padding: 8px 10px;
  border: 0;
  border-bottom: 1px solid var(--diary-line);
  background: transparent;
  color: var(--primary-text);
  text-align: left;
}

.diary-action-row:active {
  background: var(--diary-surface-soft);
}

.diary-action-icon {
  width: 38px;
  height: 38px;
  flex: 0 0 auto;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border: 1px solid var(--diary-line);
  border-radius: 11px;
  background: var(--diary-surface-soft);
  color: var(--diary-muted-text);
}

.diary-action-copy {
  min-width: 0;
  flex: 1;
  display: flex;
  flex-direction: column;
  gap: 3px;
}

.diary-action-copy strong {
  font-size: 13px;
}

.diary-action-copy > span {
  color: var(--diary-muted-text);
  font-size: 10px;
}

.diary-action-row.danger,
.diary-action-row.danger .diary-action-icon {
  color: var(--danger-color);
}

button:disabled {
  opacity: 0.35;
}

.diary-sheet-enter-active,
.diary-sheet-leave-active {
  transition: opacity 200ms ease;
}

.diary-sheet-enter-from,
.diary-sheet-leave-to {
  opacity: 0;
}

@media (prefers-reduced-motion: reduce) {
  .diary-sheet-enter-active,
  .diary-sheet-leave-active {
    transition: none;
  }
}
</style>

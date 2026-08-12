<script setup lang="ts">
import { computed, nextTick, ref } from "vue";
import { useVirtualList } from "@vueuse/core";
import { Check, FileText } from "lucide-vue-next";
import type { DiaryNoteKey, DiaryNoteSummary, DiarySearchMode } from "../types";
import {
  formatDiaryTimestamp,
  noteKeyId,
  parseDiaryFileName,
} from "../types";

const props = defineProps<{
  notes: DiaryNoteSummary[];
  loading: boolean;
  refreshing?: boolean;
  searchMode: DiarySearchMode;
  selectionMode: boolean;
  selectedIds: string[];
}>();

const emit = defineEmits<{
  open: [key: DiaryNoteKey];
  select: [key: DiaryNoteKey];
  longpress: [key: DiaryNoteKey];
}>();

const source = computed(() => props.notes.map((note) => {
  const presentation = parseDiaryFileName(note.file);
  return {
    note,
    id: noteKeyId(note),
    title: presentation.title,
    extension: presentation.extension || "FILE",
    recordedAt: presentation.structured
      ? `${presentation.date} ${presentation.time?.slice(0, 5)}`
      : "",
    updatedAt: formatDiaryTimestamp(note.lastModified),
  };
}));
const { list, containerProps, wrapperProps } = useVirtualList(source, {
  itemHeight: 96,
  overscan: 10,
});

const container = ref<HTMLElement | null>(null);
let swallowNextClick = false;

function bindContainerRef(element: unknown): void {
  const htmlElement = element as HTMLElement | null;
  containerProps.ref.value = htmlElement;
  container.value = htmlElement;
}

function handleLongPress(key: DiaryNoteKey): void {
  swallowNextClick = true;
  emit("longpress", key);
}

function handleClick(key: DiaryNoteKey): void {
  if (swallowNextClick) {
    swallowNextClick = false;
    return;
  }
  if (props.selectionMode) emit("select", key);
  else emit("open", key);
}

function getScrollTop(): number {
  return container.value?.scrollTop ?? 0;
}

async function restoreScrollTop(value: number): Promise<void> {
  await nextTick();
  if (!container.value) return;
  container.value.scrollTop = Math.max(0, value);
  containerProps.onScroll();
}

defineExpose({ getScrollTop, restoreScrollTop });
</script>

<template>
  <div
    v-if="loading && notes.length === 0"
    class="diary-list-state"
    data-diary-role="list-loading"
    aria-label="正在读取日记目录"
  >
    <div class="diary-loading-stack" aria-hidden="true">
      <div v-for="index in 5" :key="index" class="diary-loading-row">
        <span class="diary-loading-tag" />
        <span class="diary-loading-copy">
          <span />
          <span />
          <span />
        </span>
      </div>
    </div>
  </div>

  <div v-else-if="notes.length === 0" class="diary-list-state" data-diary-role="list-empty">
    <div class="diary-empty-state">
      <span class="diary-empty-icon"><FileText :size="22" /></span>
      <span class="diary-empty-kicker">VCP MEMO</span>
      <p class="m-0 text-sm font-semibold">没有可显示的记忆</p>
      <p class="mt-1.5 mb-0 text-xs text-[var(--diary-muted-text)]">
        {{ searchMode === "none" ? "当前文件夹为空" : "换个关键词或搜索范围试试" }}
      </p>
    </div>
  </div>

  <div
    v-else
    :ref="bindContainerRef"
    :style="containerProps.style"
    class="diary-note-scroll relative flex-1 overflow-y-auto vcp-scrollable no-rubber-band no-swipe"
    data-diary-role="note-list"
    @scroll="containerProps.onScroll"
  >
    <div
      v-if="refreshing"
      class="sticky top-0 z-local h-0 text-center pointer-events-none"
      aria-live="polite"
    >
      <span class="diary-refresh-pill">
        正在刷新
      </span>
    </div>

    <div v-bind="wrapperProps">
      <button
        v-for="item in list"
        :key="item.data.id"
        v-longpress="() => handleLongPress(item.data.note)"
        type="button"
        class="diary-note-slot"
        :class="selectedIds.includes(item.data.id) ? 'diary-row-selected' : ''"
        :aria-pressed="selectionMode ? selectedIds.includes(item.data.id) : undefined"
        @click="handleClick(item.data.note)"
      >
        <span class="diary-note-surface" data-diary-role="note-row">
          <span class="diary-format-tag">{{ item.data.extension }}</span>
          <span class="diary-note-copy">
            <span class="diary-note-heading">
              <strong>{{ item.data.title }}</strong>
              <time v-if="item.data.recordedAt">{{ item.data.recordedAt }}</time>
            </span>
            <span class="diary-note-preview">{{ item.data.note.preview || "暂无摘要" }}</span>
            <span class="diary-note-meta">
              <span v-if="searchMode !== 'none'" class="truncate">{{ item.data.note.folder }}</span>
              <span>{{ item.data.updatedAt }}</span>
            </span>
          </span>
          <span
            v-if="selectedIds.includes(item.data.id)"
            class="diary-selection-mark"
            aria-hidden="true"
          >
            <Check :size="15" />
          </span>
        </span>
      </button>
    </div>
  </div>
</template>

<style scoped>
.diary-list-state {
  min-height: 0;
  flex: 1;
  display: grid;
  place-items: center;
  padding: 20px 10px;
  text-align: center;
}

.diary-loading-stack {
  width: 100%;
  align-self: start;
  display: grid;
  gap: 8px;
}

.diary-loading-row {
  height: 88px;
  box-sizing: border-box;
  display: flex;
  align-items: flex-start;
  gap: 11px;
  padding: 13px 12px;
  border: 1px solid var(--diary-line);
  border-radius: 12px;
  background: var(--diary-surface-soft);
}

.diary-loading-tag,
.diary-loading-copy span {
  display: block;
  border-radius: 999px;
  background: var(--diary-loading-surface);
  animation: diary-loading-pulse 1.2s ease-in-out infinite alternate;
}

.diary-loading-tag {
  width: 38px;
  height: 22px;
  flex: 0 0 auto;
}

.diary-loading-copy {
  min-width: 0;
  flex: 1;
  display: grid;
  gap: 8px;
}

.diary-loading-copy span:nth-child(1) {
  width: 62%;
  height: 12px;
}

.diary-loading-copy span:nth-child(2) {
  width: 94%;
  height: 9px;
}

.diary-loading-copy span:nth-child(3) {
  width: 42%;
  height: 8px;
}

.diary-empty-state {
  max-width: 260px;
}

.diary-empty-icon {
  width: 48px;
  height: 48px;
  margin: 0 auto 12px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border: 1px solid var(--diary-line);
  border-radius: 14px;
  background: var(--diary-surface-soft);
  color: var(--diary-muted-text);
}

.diary-empty-kicker {
  display: block;
  margin-bottom: 5px;
  color: var(--diary-muted-text);
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 9px;
  font-weight: 700;
  letter-spacing: 0.16em;
}

.diary-note-scroll {
  padding: 4px 0 calc(var(--vcp-safe-bottom, 48px) + 4px);
}

.diary-refresh-pill {
  display: inline-block;
  margin-top: 7px;
  padding: 5px 10px;
  border: 1px solid var(--diary-line);
  border-radius: 999px;
  background: var(--diary-surface);
  color: var(--diary-muted-text);
  font-size: 10px;
}

.diary-note-slot {
  position: relative;
  width: 100%;
  height: 96px;
  box-sizing: border-box;
  display: block;
  padding: 4px 10px;
  border: 0;
  background: transparent;
  color: var(--primary-text);
  text-align: left;
}

.diary-note-slot:focus-visible {
  outline: 2px solid var(--diary-focus-outline);
  outline-offset: -3px;
}

.diary-note-surface {
  position: relative;
  height: 88px;
  box-sizing: border-box;
  display: flex;
  align-items: flex-start;
  gap: 11px;
  padding: 11px 12px;
  overflow: hidden;
  border: 1px solid var(--diary-line);
  border-radius: 12px;
  background: var(--diary-surface-soft);
}

.diary-note-slot:active .diary-note-surface {
  background: var(--diary-surface);
}

.diary-format-tag {
  min-width: 38px;
  height: 22px;
  box-sizing: border-box;
  flex: 0 0 auto;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  padding: 0 7px;
  border: 1px solid var(--diary-highlight-line-subtle);
  border-radius: 999px;
  background: var(--diary-highlight-surface-transparent);
  color: var(--highlight-text);
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 9px;
  font-weight: 800;
  letter-spacing: 0.05em;
}

.diary-note-copy {
  min-width: 0;
  flex: 1;
  display: block;
}

.diary-note-heading {
  min-width: 0;
  height: 18px;
  display: flex;
  align-items: baseline;
  gap: 8px;
}

.diary-note-heading strong {
  min-width: 0;
  flex: 1;
  overflow: hidden;
  color: var(--primary-text);
  font-size: 13px;
  font-weight: 700;
  line-height: 18px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.diary-note-heading time {
  flex: 0 0 auto;
  color: var(--diary-muted-text);
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 9px;
  line-height: 12px;
}

.diary-note-preview {
  height: 30px;
  margin-top: 2px;
  display: -webkit-box;
  overflow: hidden;
  color: var(--diary-muted-text);
  font-size: 11px;
  line-height: 15px;
  -webkit-box-orient: vertical;
  -webkit-line-clamp: 2;
}

.diary-note-meta {
  height: 12px;
  margin-top: 2px;
  display: flex;
  align-items: center;
  gap: 8px;
  overflow: hidden;
  color: var(--diary-muted-text);
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 9px;
  line-height: 12px;
}

.diary-selection-mark {
  position: absolute;
  right: 9px;
  bottom: 9px;
  width: 26px;
  height: 26px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border: 1px solid var(--highlight-text);
  border-radius: 999px;
  background: var(--diary-surface);
  color: var(--highlight-text);
}

.diary-row-selected {
  background: transparent;
}

.diary-row-selected .diary-note-surface {
  border-color: var(--diary-highlight-line-strong);
  background: var(--diary-highlight-surface);
}

.diary-row-selected .diary-note-surface::before {
  content: "";
  position: absolute;
  inset: 0 auto 0 0;
  width: 2px;
  background: var(--highlight-text);
}

@keyframes diary-loading-pulse {
  from { opacity: 0.42; }
  to { opacity: 0.9; }
}

@media (prefers-reduced-motion: reduce) {
  .diary-loading-tag,
  .diary-loading-copy span {
    animation: none;
  }
}
</style>

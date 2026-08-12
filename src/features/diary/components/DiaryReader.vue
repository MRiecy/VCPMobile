<script setup lang="ts">
import { computed } from "vue";
import { ChevronLeft, MoreHorizontal, Pencil, RefreshCw } from "lucide-vue-next";
import type { DiaryDocument, DiaryUiError } from "../types";
import { parseDiaryFileName } from "../types";
import { renderDiaryMarkdownWithHighlight } from "../diaryMarkdown";

const props = defineProps<{
  document: DiaryDocument | null;
  loading: boolean;
  refreshing: boolean;
  error: DiaryUiError | null;
  highlightTerm?: string;
}>();

const emit = defineEmits<{
  back: [];
  refresh: [];
  edit: [];
  more: [];
}>();

const presentation = computed(() =>
  props.document ? parseDiaryFileName(props.document.key.file) : null,
);
const renderedContent = computed(() =>
  props.document
    ? renderDiaryMarkdownWithHighlight(props.document.content, props.highlightTerm)
    : "",
);
</script>

<template>
  <section class="h-full min-h-0 flex flex-col bg-[var(--primary-bg)] text-[var(--primary-text)]">
    <header class="diary-page-header shrink-0" data-diary-role="reader-header">
      <div class="diary-page-toolbar">
        <button type="button" class="diary-icon-button" aria-label="返回日记列表" @click="emit('back')">
          <ChevronLeft :size="22" />
        </button>
        <div class="diary-page-title">
          <span class="diary-page-eyebrow">VCP MEMO · {{ document?.key.folder || "DOCUMENT" }}</span>
          <h2>{{ presentation?.title || "日记正文" }}</h2>
        </div>
        <button
          type="button"
          class="diary-icon-button"
          :disabled="loading || refreshing"
          aria-label="刷新正文"
          @click="emit('refresh')"
        >
          <RefreshCw :size="17" :class="refreshing ? 'animate-spin' : ''" />
        </button>
        <button
          type="button"
          class="diary-icon-button"
          :disabled="!document"
          aria-label="日记操作"
          @click="emit('more')"
        >
          <MoreHorizontal :size="20" />
        </button>
      </div>
    </header>

    <div v-if="loading && !document" class="diary-reader-state">
      <span class="diary-reader-state-mark">MEMO</span>
      <strong>正在读取正文…</strong>
    </div>

    <div v-else-if="error && !document" class="flex-1 grid place-items-center px-6 text-center">
      <div>
        <p class="m-0 text-sm font-semibold text-[var(--danger-color)]">正文读取失败</p>
        <p class="mt-2 mb-4 text-xs text-[var(--diary-muted-text)]">{{ error.message }}</p>
        <button type="button" class="diary-text-button" @click="emit('refresh')">重新读取</button>
      </div>
    </div>

    <div
      v-else-if="document"
      class="diary-reader-scroll flex-1 min-h-0 overflow-y-auto vcp-scrollable no-rubber-band no-swipe select-text"
    >
      <div
        v-if="error"
        class="mx-4 mt-3 px-3 py-2 text-xs border-l-2 border-[var(--danger-color)] bg-[var(--secondary-bg)]"
        role="status"
      >
        刷新失败，继续显示已有正文：{{ error.message }}
      </div>
      <div class="diary-document-shell mx-auto w-full max-w-[720px]" data-diary-role="reader-document">
        <div class="diary-document-meta">
          <span class="diary-document-format">{{ presentation?.extension || "FILE" }}</span>
          <span v-if="presentation?.structured" class="diary-document-time">
            {{ presentation.date }} · {{ presentation.time?.slice(0, 5) }}
          </span>
          <span class="diary-document-file">{{ document.key.file }}</span>
        </div>
        <article
          class="diary-reader-content vcp-markdown-block text-[16px] leading-[1.68]"
          v-html="renderedContent"
        />
      </div>
    </div>

    <footer
      v-if="document"
      class="diary-reader-footer"
    >
      <div class="min-w-0 flex-1">
        <span class="diary-page-eyebrow">READ MODE</span>
        <span class="block mt-0.5 truncate text-[10px] text-[var(--diary-muted-text)]">{{ document.key.folder }}</span>
      </div>
      <button type="button" class="diary-primary-button" @click="emit('edit')">
        <Pencil :size="16" />
        编辑
      </button>
    </footer>
  </section>
</template>

<style scoped>
.diary-page-header {
  padding-top: var(--diary-header-safe-top);
  border-bottom: 1px solid var(--diary-line);
  background: var(--primary-bg);
}

.diary-page-toolbar {
  min-height: 64px;
  padding: 0 4px;
  display: flex;
  align-items: center;
  gap: 0;
}

.diary-icon-button {
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

.diary-icon-button:active {
  background: var(--diary-surface);
}

.diary-icon-button:disabled {
  opacity: 0.3;
}

.diary-page-title {
  min-width: 0;
  flex: 1;
  padding: 6px 8px;
}

.diary-page-title h2 {
  margin: 2px 0 0;
  overflow: hidden;
  font-size: 15px;
  font-weight: 700;
  line-height: 20px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.diary-page-eyebrow {
  display: block;
  overflow: hidden;
  color: var(--diary-muted-text);
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 9px;
  font-weight: 700;
  line-height: 12px;
  letter-spacing: 0.12em;
  text-overflow: ellipsis;
  text-transform: uppercase;
  white-space: nowrap;
}

.diary-reader-state {
  flex: 1;
  display: grid;
  place-content: center;
  gap: 6px;
  padding: 24px;
  color: var(--diary-muted-text);
  text-align: center;
}

.diary-reader-state-mark {
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 9px;
  font-weight: 800;
  letter-spacing: 0.18em;
}

.diary-reader-scroll {
  padding: 0 0 16px;
}

.diary-document-shell {
  box-sizing: border-box;
  border: 0;
  border-radius: 0;
  background: transparent;
}

.diary-document-meta {
  min-height: 42px;
  box-sizing: border-box;
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 9px 16px;
  border-bottom: 1px solid var(--diary-line);
  color: var(--diary-muted-text);
}

.diary-document-format {
  min-width: 38px;
  height: 22px;
  box-sizing: border-box;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  padding: 0 7px;
  border: 1px solid color-mix(in srgb, var(--highlight-text) 36%, var(--diary-line));
  border-radius: 999px;
  color: var(--highlight-text);
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 9px;
  font-weight: 800;
}

.diary-document-time,
.diary-document-file {
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 9px;
}

.diary-document-time {
  flex: 0 0 auto;
}

.diary-document-file {
  min-width: 0;
  flex: 1;
  overflow: hidden;
  text-align: right;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.diary-reader-content {
  padding: 20px 16px 28px;
}

.diary-reader-footer {
  min-height: 64px;
  box-sizing: border-box;
  flex: 0 0 auto;
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 8px 14px calc(var(--vcp-safe-bottom, 48px) + 8px);
  border-top: 1px solid var(--diary-line);
  background: var(--primary-bg);
}

.diary-text-button,
.diary-primary-button {
  min-height: 48px;
  border: 1px solid var(--diary-line);
  border-radius: 12px;
  background: var(--diary-surface);
  color: var(--primary-text);
  font-weight: 600;
}

.diary-text-button {
  padding: 0 18px;
}

.diary-primary-button {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  padding: 0 20px;
  border-color: color-mix(in srgb, var(--highlight-text) 55%, var(--diary-line));
  color: var(--highlight-text);
}

.diary-reader-content :deep(pre),
.diary-reader-content :deep(table) {
  max-width: 100%;
  overflow-x: auto;
}

.diary-reader-content :deep(img) {
  max-width: 100%;
  height: auto;
}

.diary-reader-content :deep(mark.diary-search-mark) {
  padding: 0 1px;
  background: color-mix(in srgb, var(--warning-color, #eab308) 34%, transparent);
  color: inherit;
}

@media (prefers-reduced-motion: reduce) {
  .animate-spin {
    animation: none;
  }
}
</style>

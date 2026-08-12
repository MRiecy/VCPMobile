<script setup lang="ts">
import { computed } from "vue";
import { ChevronLeft, Copy, Eye, FileEdit, RefreshCw, Save } from "lucide-vue-next";
import type { DiaryDocument, DiarySaveState, DiaryUiError } from "../types";
import { parseDiaryFileName } from "../types";
import { renderDiaryMarkdown } from "../diaryMarkdown";

const props = defineProps<{
  document: DiaryDocument;
  draft: string;
  dirty: boolean;
  preview: boolean;
  saveState: DiarySaveState;
  error: DiaryUiError | null;
  keyboardHeight: number;
}>();

const emit = defineEmits<{
  back: [];
  update: [value: string];
  preview: [];
  edit: [];
  save: [];
  copy: [];
  reload: [];
  force: [];
}>();

const renderedDraft = computed(() => renderDiaryMarkdown(props.draft));
const presentation = computed(() => parseDiaryFileName(props.document.key.file));
const statusLabel = computed(() => {
  switch (props.saveState) {
    case "dirty": return "有未保存修改";
    case "saving": return "正在保存并核验";
    case "saved": return "已保存 · 索引稍后追平";
    case "conflict": return "远端内容已变化";
    case "uncertain": return "保存结果无法确认";
    case "error": return "保存失败";
    default: return props.dirty ? "有未保存修改" : "尚未修改";
  }
});

function handleInput(event: Event): void {
  emit("update", (event.target as HTMLTextAreaElement).value);
}
</script>

<template>
  <section class="h-full min-h-0 flex flex-col bg-[var(--primary-bg)] text-[var(--primary-text)]">
    <header class="diary-page-header shrink-0" data-diary-role="editor-header">
      <div class="diary-page-toolbar">
        <button type="button" class="diary-icon-button" aria-label="退出编辑" @click="emit('back')">
          <ChevronLeft :size="22" />
        </button>
        <div class="diary-page-title">
          <span class="diary-page-eyebrow">VCP MEMO · {{ preview ? "PREVIEW" : "EDITOR" }}</span>
          <h2>{{ presentation.title }}</h2>
        </div>
      </div>
    </header>

    <div
      v-if="preview"
      class="diary-editor-stage flex-1 min-h-0 overflow-y-auto vcp-scrollable no-rubber-band no-swipe select-text"
    >
      <div class="diary-editor-sheet mx-auto w-full max-w-[720px]" data-diary-role="editor-preview">
        <div class="diary-editor-meta">
          <span>{{ presentation.extension || "FILE" }}</span>
          <span class="truncate">{{ document.key.folder }} / {{ document.key.file }}</span>
        </div>
        <article
          class="diary-editor-preview vcp-markdown-block text-[16px] leading-[1.68]"
          v-html="renderedDraft"
        />
      </div>
    </div>
    <div v-else class="diary-editor-stage flex-1 min-h-0">
      <textarea
        :value="draft"
        class="diary-editor-input vcp-scrollable no-swipe select-text"
        spellcheck="false"
        aria-label="日记 Markdown 原文"
        @input="handleInput"
      />
    </div>

    <div
      v-if="error"
      class="diary-editor-error shrink-0 text-xs"
      role="status"
    >
      <p class="m-0 text-[var(--danger-color)]">{{ error.message }}</p>
      <div v-if="saveState === 'conflict' || saveState === 'uncertain'" class="flex flex-wrap gap-1 mt-2">
        <button type="button" class="diary-small-button" @click="emit('copy')">
          <Copy :size="13" />复制草稿
        </button>
        <button type="button" class="diary-small-button" @click="emit('reload')">
          <RefreshCw :size="13" />加载远端
        </button>
        <button type="button" class="diary-small-button danger" @click="emit('force')">
          明确覆盖
        </button>
      </div>
    </div>

    <footer
      class="diary-editor-footer shrink-0"
      :style="{ paddingBottom: `calc(var(--vcp-safe-bottom, 48px) + ${keyboardHeight}px + 8px)` }"
    >
      <div class="min-h-12 flex items-center gap-2">
        <div
          class="min-w-0 flex-1 text-[11px]"
          :class="saveState === 'conflict' || saveState === 'uncertain' || saveState === 'error'
            ? 'text-[var(--danger-color)]'
            : 'text-[var(--diary-muted-text)]'"
          aria-live="polite"
        >
          {{ statusLabel }}
        </div>
        <button
          type="button"
          class="diary-footer-button"
          :disabled="saveState === 'saving'"
          @click="preview ? emit('edit') : emit('preview')"
        >
          <FileEdit v-if="preview" :size="15" />
          <Eye v-else :size="15" />
          {{ preview ? "继续编辑" : "预览" }}
        </button>
        <button
          type="button"
          class="diary-footer-button primary"
          :disabled="saveState === 'saving' || !dirty"
          @click="emit('save')"
        >
          <Save :size="15" />
          {{ saveState === "saving" ? "保存中" : "保存" }}
        </button>
      </div>
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
  color: var(--diary-muted-text);
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 9px;
  font-weight: 700;
  line-height: 12px;
  letter-spacing: 0.14em;
}

.diary-editor-stage {
  box-sizing: border-box;
  padding: 10px;
}

.diary-editor-sheet {
  box-sizing: border-box;
  overflow: hidden;
  border: 1px solid var(--diary-line);
  border-radius: 14px;
  background: var(--diary-surface-soft);
}

.diary-editor-meta {
  min-height: 40px;
  box-sizing: border-box;
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 8px 12px;
  border-bottom: 1px solid var(--diary-line);
  color: var(--diary-muted-text);
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 9px;
}

.diary-editor-meta > span:first-child {
  padding: 3px 7px;
  border: 1px solid color-mix(in srgb, var(--highlight-text) 36%, var(--diary-line));
  border-radius: 999px;
  color: var(--highlight-text);
  font-weight: 800;
}

.diary-editor-preview {
  padding: 18px 16px 24px;
}

.diary-editor-input {
  width: 100%;
  height: 100%;
  box-sizing: border-box;
  resize: none;
  border: 1px solid var(--diary-line);
  border-radius: 14px;
  background: var(--diary-surface-soft);
  color: var(--primary-text);
  padding: 16px;
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 14px;
  line-height: 1.65;
  outline: none;
}

.diary-editor-input:focus {
  border-color: color-mix(in srgb, var(--highlight-text) 58%, var(--diary-line));
}

.diary-editor-error {
  padding: 10px 14px;
  border-top: 1px solid var(--diary-line);
  border-left: 2px solid var(--danger-color);
  background: var(--diary-surface);
}

.diary-editor-footer {
  padding: 8px 12px 0;
  border-top: 1px solid var(--diary-line);
  background: var(--primary-bg);
}

.diary-footer-button,
.diary-small-button {
  min-height: 48px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 6px;
  padding: 0 14px;
  border: 1px solid var(--diary-line);
  border-radius: 12px;
  background: var(--diary-surface);
  color: var(--primary-text);
  font-weight: 600;
}

.diary-small-button {
  padding: 0 10px;
  font-size: 11px;
}

.diary-footer-button.primary {
  border-color: color-mix(in srgb, var(--highlight-text) 55%, var(--diary-line));
  color: var(--highlight-text);
}

.diary-small-button.danger {
  border-color: var(--danger-color);
  color: var(--danger-color);
}

.diary-footer-button:disabled,
.diary-small-button:disabled {
  opacity: 0.35;
}

.diary-editor-preview :deep(pre),
.diary-editor-preview :deep(table) {
  max-width: 100%;
  overflow-x: auto;
}
</style>

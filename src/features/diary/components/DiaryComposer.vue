<script setup lang="ts">
import { computed } from "vue";
import { ChevronLeft, Send } from "lucide-vue-next";
import type { DiaryComposerDraft, DiaryUiError } from "../types";

const props = defineProps<{
  draft: DiaryComposerDraft;
  folders: string[];
  submitting: boolean;
  error: DiaryUiError | null;
  keyboardHeight: number;
}>();

const emit = defineEmits<{
  back: [];
  update: [patch: Partial<DiaryComposerDraft>];
  submit: [];
}>();

const canSubmit = computed(() =>
  Boolean(
    props.draft.maid.trim()
      && props.draft.date.trim()
      && props.draft.content.trim()
      && props.error?.code !== "DIARY_CREATE_UNCERTAIN",
  ),
);

function updateField<K extends keyof DiaryComposerDraft>(field: K, event: Event): void {
  emit("update", { [field]: (event.target as HTMLInputElement | HTMLTextAreaElement).value });
}
</script>

<template>
  <section class="h-full min-h-0 flex flex-col bg-[var(--primary-bg)] text-[var(--primary-text)]">
    <header class="diary-page-header shrink-0" data-diary-role="composer-header">
      <div class="diary-page-toolbar">
        <button type="button" class="diary-icon-button" aria-label="退出新建日记" @click="emit('back')">
          <ChevronLeft :size="22" />
        </button>
        <div class="diary-page-title">
          <span class="diary-page-eyebrow">VCP MEMO · NEW ENTRY</span>
          <h2>新建 DailyNote</h2>
        </div>
      </div>
    </header>

    <form class="flex-1 min-h-0 overflow-y-auto vcp-scrollable no-rubber-band no-swipe" @submit.prevent="emit('submit')">
      <div class="mx-auto max-w-[720px] px-4 py-4 space-y-4">
        <div class="diary-compose-intro">
          <span class="diary-page-eyebrow">CREATE MEMORY</span>
          <p>内容将写入 VCP 记忆库；实际文件名由服务端生成并返回。</p>
        </div>

        <section class="diary-form-section" aria-label="日记属性">
          <div class="diary-section-heading">
            <strong>记录信息</strong>
            <span>必填项标记 *</span>
          </div>
          <div class="grid grid-cols-2 gap-3">
            <label class="diary-field">
              <span>署名 *</span>
              <input
                :value="draft.maid"
                maxlength="200"
                autocomplete="off"
                placeholder="Maid / 用户名"
                @input="updateField('maid', $event)"
              />
            </label>
            <label class="diary-field">
              <span>日期 *</span>
              <input :value="draft.date" type="date" @input="updateField('date', $event)" />
            </label>
          </div>

          <label class="diary-field">
            <span>文件夹</span>
            <input
              :value="draft.folder"
              list="diary-folder-options"
              autocomplete="off"
              placeholder="留空则使用服务端默认文件夹"
              @input="updateField('folder', $event)"
            />
            <datalist id="diary-folder-options">
              <option v-for="folder in folders" :key="folder" :value="folder" />
            </datalist>
          </label>

          <label class="diary-field">
            <span>文件名后缀</span>
            <input
              :value="draft.fileNameSuffix"
              autocomplete="off"
              placeholder="可选；不是完整目标文件名"
              @input="updateField('fileNameSuffix', $event)"
            />
          </label>

          <label class="diary-field">
            <span>Tag</span>
            <input
              :value="draft.tag"
              maxlength="2000"
              autocomplete="off"
              placeholder="可选"
              @input="updateField('tag', $event)"
            />
          </label>
        </section>

        <section class="diary-form-section" aria-label="日记正文">
          <label class="diary-field">
            <span>正文 *</span>
            <textarea
              :value="draft.content"
              rows="12"
              placeholder="输入日记正文…"
              @input="updateField('content', $event)"
            />
          </label>
        </section>

        <div v-if="error" class="diary-compose-error" role="status">
          {{ error.message }}
          <p v-if="error.code === 'DIARY_CREATE_UNCERTAIN'" class="m-0 mt-1 font-semibold">
            为避免生成重复日记，本页已禁止直接重试。
          </p>
        </div>
      </div>
    </form>

    <footer
      class="diary-compose-footer shrink-0"
      :style="{ paddingBottom: `calc(var(--vcp-safe-bottom, 48px) + ${keyboardHeight}px + 8px)` }"
    >
      <button
        type="button"
        class="diary-submit-button"
        :disabled="!canSubmit || submitting"
        @click="emit('submit')"
      >
        <Send :size="16" />
        {{ submitting ? "正在创建…" : "创建并打开" }}
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
  font-size: 15px;
  font-weight: 700;
  line-height: 20px;
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

.diary-compose-intro {
  padding: 2px 2px 0;
}

.diary-compose-intro p {
  margin: 5px 0 0;
  color: var(--diary-muted-text);
  font-size: 12px;
  line-height: 1.55;
}

.diary-form-section {
  display: grid;
  gap: 14px;
  padding: 14px;
  border: 1px solid var(--diary-line);
  border-radius: 14px;
  background: var(--diary-surface-soft);
}

.diary-section-heading {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 12px;
  padding-bottom: 10px;
  border-bottom: 1px solid var(--diary-line);
}

.diary-section-heading strong {
  font-size: 13px;
}

.diary-section-heading span {
  color: var(--diary-muted-text);
  font-size: 9px;
}

.diary-field {
  display: flex;
  flex-direction: column;
  gap: 6px;
  color: var(--diary-muted-text);
  font-size: 11px;
  font-weight: 600;
}

.diary-field input,
.diary-field textarea {
  width: 100%;
  box-sizing: border-box;
  border: 1px solid var(--diary-line);
  border-radius: 11px;
  background: var(--diary-surface);
  color: var(--primary-text);
  padding: 11px 12px;
  font: inherit;
  font-size: 14px;
  font-weight: 400;
  outline: none;
}

.diary-field input {
  min-height: 48px;
}

.diary-field textarea {
  min-height: 240px;
  resize: vertical;
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  line-height: 1.55;
}

.diary-field input:focus,
.diary-field textarea:focus {
  border-color: var(--diary-highlight-line-strong);
}

.diary-compose-error {
  padding: 10px 12px;
  border: 1px solid var(--diary-line);
  border-left: 2px solid var(--danger-color);
  border-radius: 10px;
  background: var(--diary-surface);
  color: var(--danger-color);
  font-size: 12px;
}

.diary-compose-footer {
  padding: 8px 14px 0;
  border-top: 1px solid var(--diary-line);
  background: var(--primary-bg);
}

.diary-submit-button {
  width: 100%;
  min-height: 48px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 8px;
  border: 1px solid var(--diary-highlight-line-strong);
  border-radius: 12px;
  background: var(--diary-surface);
  color: var(--highlight-text);
  font-weight: 700;
}

.diary-submit-button:disabled {
  opacity: 0.35;
}
</style>

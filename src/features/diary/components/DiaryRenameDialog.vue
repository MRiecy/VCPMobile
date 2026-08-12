<script setup lang="ts">
import { computed, nextTick, ref, watch } from "vue";
import { FilePenLine } from "lucide-vue-next";
import { isValidDiaryFileName } from "../types";

const props = defineProps<{
  open: boolean;
  currentFile: string;
  busy: boolean;
  serverError?: string;
}>();

const emit = defineEmits<{
  close: [];
  confirm: [file: string];
}>();

const value = ref("");
const input = ref<HTMLInputElement | null>(null);
const localError = computed(() => {
  if (!value.value.trim()) return "文件名不能为空";
  if (!isValidDiaryFileName(value.value)) return "文件名不能包含路径分隔符或路径语义";
  if (value.value.trim() === props.currentFile) return "请输入不同的文件名";
  return "";
});

watch(() => props.open, async (open) => {
  if (!open) return;
  value.value = props.currentFile;
  await nextTick();
  const dot = props.currentFile.lastIndexOf(".");
  input.value?.focus();
  input.value?.setSelectionRange(0, dot > 0 ? dot : props.currentFile.length);
});

function submit(): void {
  if (localError.value || props.busy) return;
  emit("confirm", value.value.trim());
}
</script>

<template>
  <div v-if="open" class="fixed inset-0 z-dialog flex items-start justify-center pt-[15vh] pl-[calc(var(--vcp-safe-left,0px)+1rem)] pr-[calc(var(--vcp-safe-right,0px)+1rem)] bg-black/45 no-swipe" role="presentation" @click.self="emit('close')">
    <section class="diary-rename-dialog" role="dialog" aria-modal="true" aria-label="重命名文件">
      <header class="diary-rename-header">
        <span class="diary-rename-icon"><FilePenLine :size="18" /></span>
        <div class="min-w-0 flex-1">
          <span class="diary-rename-eyebrow">VCP MEMO · RENAME</span>
          <h2>重命名文件</h2>
        </div>
      </header>
      <p class="diary-rename-hint">保留扩展名；目标已存在时不会覆盖。</p>
      <input
        ref="input"
        v-model="value"
        class="diary-rename-input"
        autocomplete="off"
        aria-label="新文件名"
        @keydown.enter="submit"
        @keydown.esc="emit('close')"
      />
      <p v-if="localError || serverError" class="min-h-4 mt-2 mb-0 text-xs text-[var(--danger-color)]" role="status">
        {{ localError || serverError }}
      </p>
      <div class="flex justify-end gap-2 mt-4">
        <button type="button" class="diary-dialog-button" :disabled="busy" @click="emit('close')">取消</button>
        <button type="button" class="diary-dialog-button primary" :disabled="Boolean(localError) || busy" @click="submit">
          {{ busy ? "处理中…" : "重命名" }}
        </button>
      </div>
    </section>
  </div>
</template>

<style scoped>
.diary-rename-dialog {
  width: 100%;
  max-width: 384px;
  box-sizing: border-box;
  padding: 16px;
  overflow: hidden;
  border: 1px solid var(--diary-line);
  border-radius: 16px;
  background: var(--primary-bg);
  color: var(--primary-text);
}

.diary-rename-header {
  display: flex;
  align-items: center;
  gap: 11px;
}

.diary-rename-icon {
  width: 40px;
  height: 40px;
  flex: 0 0 auto;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border: 1px solid var(--diary-line);
  border-radius: 12px;
  background: var(--diary-surface-soft);
  color: var(--highlight-text);
}

.diary-rename-header h2 {
  margin: 2px 0 0;
  font-size: 15px;
  font-weight: 700;
}

.diary-rename-eyebrow {
  display: block;
  color: var(--diary-muted-text);
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 9px;
  font-weight: 700;
  letter-spacing: 0.14em;
}

.diary-rename-hint {
  margin: 14px 0 8px;
  color: var(--diary-muted-text);
  font-size: 11px;
  line-height: 1.5;
}

.diary-rename-input {
  width: 100%;
  height: 48px;
  box-sizing: border-box;
  padding: 0 12px;
  border: 1px solid var(--diary-line);
  border-radius: 11px;
  background: var(--diary-surface);
  color: var(--primary-text);
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 13px;
  outline: none;
}

.diary-rename-input:focus {
  border-color: var(--diary-highlight-line-strong);
}

.diary-dialog-button {
  min-width: 72px;
  min-height: 48px;
  border: 1px solid var(--diary-line);
  border-radius: 11px;
  background: var(--diary-surface-soft);
  color: var(--primary-text);
  font-weight: 600;
}

.diary-dialog-button.primary {
  border-color: var(--diary-highlight-line-strong);
  color: var(--highlight-text);
}

.diary-dialog-button:disabled {
  opacity: 0.35;
}
</style>

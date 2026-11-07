<script setup lang="ts">
/**
 * V工程「源码」编辑器（对齐桌面源码侧栏的编辑/保存）。
 *
 * - textarea + 行号沟槽 + 等宽字体；脏标记；不做自动保存（只有点「保存」/Ctrl+S 才写盘）；
 * - 保存：reason 必填 + 强确认弹层 → ProjectForge `EditCode` 整文件替换 → 写后 ReadCode 回读校验；
 * - 新建文件：路径 + 内容 + reason + 强确认 → `CreateFile` → 回读校验；
 * - 删除 / 移动入口 emit 给 PfSourcePanel 统一弹层确认（写操作强确认集中管理）；
 * - 未保存离开由 PfSourcePanel 通过 `defineExpose` 的 isDirty()/resetDraft() 协调。
 */
import { computed, nextTick, ref, watch } from 'vue';
import { ArrowLeft, Check, FilePlus2, FolderInput, Loader2, Trash2, X } from "@lucide/vue";
import { useProjectForgeSource } from '../useProjectForgeSource';

const props = withDefaults(defineProps<{ mode?: 'edit' | 'create' }>(), { mode: 'edit' });

const emit = defineEmits<{
  back: [];
  saved: [path: string];
  created: [path: string];
  delete: [];
  move: [];
  'dirty-change': [dirty: boolean];
}>();

const source = useProjectForgeSource();
const { currentFile, saving, actionBusy, actionError, saveFile, createFile } = source;

const draft = ref('');
const createPath = ref('');
const reason = ref('');
const localError = ref('');
const confirmOpen = ref(false);
const confirmKind = ref<'save' | 'create'>('save');
const textareaRef = ref<HTMLTextAreaElement | null>(null);
const gutterRef = ref<HTMLElement | null>(null);

const isCreate = computed(() => props.mode === 'create');
const title = computed(() => (isCreate.value ? '新建文件' : currentFile.value?.path || '编辑文件'));
const dirty = computed(() => {
  if (isCreate.value) return draft.value !== '' || createPath.value.trim() !== '';
  return draft.value !== (currentFile.value?.text ?? '');
});
const lineNumbers = computed(() => {
  const count = draft.value.split('\n').length;
  return Array.from({ length: Math.max(1, count) }, (_, index) => index + 1);
});
const busy = computed(() => saving.value || actionBusy.value);
const canSave = computed(() => {
  if (busy.value) return false;
  // 新建模式的路径校验放在点击时给出中文提示（按钮保持可点）
  if (isCreate.value) return true;
  return dirty.value;
});

/** 切换到另一个文件 / 切换编辑模式时，从磁盘内容重置编辑器。 */
function resetFromDisk(): void {
  localError.value = '';
  reason.value = '';
  confirmOpen.value = false;
  if (props.mode === 'create') {
    draft.value = '';
    createPath.value = '';
    return;
  }
  draft.value = currentFile.value?.text ?? '';
  void nextTick(() => textareaRef.value?.scrollTo?.({ top: 0, left: 0 }));
}

watch([() => currentFile.value?.path, () => props.mode], resetFromDisk, { immediate: true });

// 同文件磁盘内容变化（保存成功 / 回读不一致重新载入）：未修改时同步草稿；
// 有本地修改时保留用户草稿（例如保存回读失败，用户可以继续修正后重试）。
watch(
  () => currentFile.value?.text,
  (text) => {
    if (isCreate.value || typeof text !== 'string') return;
    if (!dirty.value) draft.value = text;
  },
);

watch(
  dirty,
  (value) => {
    emit('dirty-change', value);
  },
  { immediate: true },
);

function syncGutterScroll(): void {
  const textarea = textareaRef.value;
  const gutter = gutterRef.value;
  if (!textarea || !gutter) return;
  gutter.scrollTop = textarea.scrollTop;
}

function onEditorKeydown(event: KeyboardEvent): void {
  if (!(event.ctrlKey || event.metaKey) || event.altKey) return;
  if (event.key.toLowerCase() !== 's') return;
  event.preventDefault();
  openSaveConfirm();
}

function openSaveConfirm(): void {
  localError.value = '';
  if (isCreate.value) {
    if (!createPath.value.trim()) {
      localError.value = '请填写新文件路径';
      return;
    }
    confirmKind.value = 'create';
    confirmOpen.value = true;
    return;
  }
  if (!currentFile.value) {
    localError.value = '当前没有打开的文件';
    return;
  }
  if (!dirty.value) return;
  confirmKind.value = 'save';
  confirmOpen.value = true;
}

function closeConfirm(): void {
  if (busy.value) return;
  confirmOpen.value = false;
  localError.value = '';
}

async function confirmWrite(): Promise<void> {
  localError.value = '';
  if (!reason.value.trim()) {
    localError.value = '请填写本次改动原因（reason 必填，会写入桌面施工记录）';
    return;
  }
  try {
    if (confirmKind.value === 'create') {
      const file = await createFile({
        path: createPath.value,
        content: draft.value,
        reason: reason.value,
      });
      confirmOpen.value = false;
      reason.value = '';
      createPath.value = '';
      draft.value = '';
      emit('created', file.path);
      return;
    }
    const result = await saveFile(draft.value, reason.value);
    confirmOpen.value = false;
    reason.value = '';
    draft.value = currentFile.value?.text ?? '';
    emit('saved', result.path);
  } catch (error) {
    localError.value = error instanceof Error ? error.message : String(error);
  }
}

function onBack(): void {
  emit('back');
}

function requestDelete(): void {
  emit('delete');
}

function requestMove(): void {
  emit('move');
}

defineExpose({
  isDirty: (): boolean => dirty.value,
  resetDraft: (): void => {
    if (isCreate.value) {
      draft.value = '';
      createPath.value = '';
    } else {
      draft.value = currentFile.value?.text ?? '';
    }
    reason.value = '';
    localError.value = '';
    confirmOpen.value = false;
  },
});
</script>

<template>
  <section class="pf-editor" aria-label="源码编辑器">
    <!-- 顶部 -->
    <header class="pf-editor-head">
      <button type="button" class="pf-icon-btn" title="返回" aria-label="返回" @click="onBack">
        <ArrowLeft :size="20" />
      </button>
      <div class="pf-editor-title">
        <code class="pf-editor-path" :title="title">{{ title }}</code>
        <span class="pf-editor-state">
          <template v-if="isCreate">新文件（保存后写入桌面工程）</template>
          <template v-else-if="saving">保存中…</template>
          <template v-else-if="dirty"><span class="pf-dot" />未保存修改</template>
          <template v-else>已保存</template>
          <template v-if="!isCreate && currentFile"> · {{ lineNumbers.length }} 行 · {{ currentFile.eol }}</template>
        </span>
      </div>
      <button
        type="button"
        class="pf-save-btn"
        :disabled="!canSave"
        :title="isCreate ? '新建文件' : '保存（Ctrl+S）'"
        @click="openSaveConfirm"
      >
        <Loader2 v-if="saving" :size="15" class="pf-spin" />
        <FilePlus2 v-else-if="isCreate" :size="15" />
        <Check v-else :size="15" />
        {{ isCreate ? '新建' : '保存' }}
      </button>
    </header>

    <!-- 新建模式：路径输入 -->
    <div v-if="isCreate" class="pf-create-path">
      <label class="pf-create-label" for="pf-create-path-input">新文件路径（工程内相对路径）</label>
      <input
        id="pf-create-path-input"
        v-model="createPath"
        class="pf-create-input"
        type="text"
        placeholder="例如 src/components/Demo.vue"
        autocomplete="off"
        autocapitalize="off"
        spellcheck="false"
      />
    </div>

    <!-- 错误提示 -->
    <div v-if="actionError && !confirmOpen" class="pf-editor-banner pf-editor-error">{{ actionError }}</div>
    <div v-else-if="localError && !confirmOpen" class="pf-editor-banner pf-editor-error">{{ localError }}</div>

    <!-- 编辑器主体：行号沟槽 + textarea -->
    <div class="pf-editor-body">
      <div ref="gutterRef" class="pf-editor-gutter" aria-hidden="true">
        <div v-for="no in lineNumbers" :key="no" class="pf-gutter-line">{{ no }}</div>
      </div>
      <textarea
        ref="textareaRef"
        v-model="draft"
        class="pf-editor-textarea"
        wrap="off"
        spellcheck="false"
        autocapitalize="off"
        autocomplete="off"
        autocorrect="off"
        :placeholder="isCreate ? '在这里输入新文件内容…' : '文件内容'"
        @scroll="syncGutterScroll"
        @keydown="onEditorKeydown"
      ></textarea>
    </div>

    <!-- 底部操作 -->
    <footer class="pf-editor-actions">
      <template v-if="!isCreate">
        <button type="button" class="pf-action-btn" :disabled="busy" @click="requestMove">
          <FolderInput :size="15" />
          移动
        </button>
        <button type="button" class="pf-action-btn pf-action-danger" :disabled="busy" @click="requestDelete">
          <Trash2 :size="15" />
          删除
        </button>
      </template>
      <span class="pf-editor-hint">
        写入前会二次确认并填写原因；保存后自动回读校验。不做自动保存。
      </span>
    </footer>

    <!-- 强确认弹层 -->
    <div v-if="confirmOpen" class="pf-confirm-mask" role="dialog" aria-modal="true" aria-label="确认写入">
      <div class="pf-confirm-card">
        <header class="pf-confirm-head">
          <strong>{{ confirmKind === 'create' ? '确认新建文件' : '确认保存文件' }}</strong>
          <button type="button" class="pf-icon-btn" aria-label="关闭" :disabled="busy" @click="closeConfirm">
            <X :size="18" />
          </button>
        </header>
        <div class="pf-confirm-body">
          <p class="pf-confirm-line">
            <span class="pf-confirm-label">文件</span>
            <code class="pf-confirm-path">{{ confirmKind === 'create' ? createPath : currentFile?.path }}</code>
          </p>
          <p class="pf-confirm-warn">
            将通过桌面 ProjectForge
            <code>{{ confirmKind === 'create' ? 'CreateFile' : 'EditCode' }}</code>
            写入桌面工程（写入前自动快照，可在桌面端回退）；{{ confirmKind === 'create' ? '文件已存在时会被拒绝。' : '将整文件覆盖。' }}
          </p>
          <label class="pf-confirm-label" for="pf-write-reason">改动原因（必填，写入施工记录）</label>
          <textarea
            id="pf-write-reason"
            v-model="reason"
            class="pf-confirm-reason"
            rows="2"
            maxlength="500"
            placeholder="例如：修正登录校验的空值判断"
          ></textarea>
          <p v-if="localError" class="pf-confirm-error">{{ localError }}</p>
        </div>
        <footer class="pf-confirm-actions">
          <button type="button" class="pf-action-btn" :disabled="busy" @click="closeConfirm">取消</button>
          <button
            type="button"
            class="pf-action-btn pf-action-primary"
            :disabled="busy || !reason.trim()"
            @click="confirmWrite"
          >
            <Loader2 v-if="busy" :size="15" class="pf-spin" />
            {{ confirmKind === 'create' ? '确认新建' : '确认保存' }}
          </button>
        </footer>
      </div>
    </div>
  </section>
</template>

<style scoped>
.pf-editor {
  position: relative;
  display: flex;
  flex-direction: column;
  min-height: 0;
  height: 100%;
  color: var(--primary-text);
  background: var(--primary-bg, transparent);
}

.pf-editor-head {
  display: flex;
  align-items: center;
  gap: 4px;
  padding: 4px 6px;
  border-bottom: 1px solid var(--vcp-border-subtle, rgba(255, 255, 255, 0.08));
  flex: 0 0 auto;
}

.pf-editor-title {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
  padding-left: 2px;
}

.pf-editor-path {
  font-family: ui-monospace, 'SFMono-Regular', Menlo, Consolas, monospace;
  font-size: 11.5px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  direction: rtl;
  text-align: left;
}

.pf-editor-state {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  font-size: 9.5px;
  opacity: 0.5;
  font-family: ui-monospace, 'SFMono-Regular', Menlo, Consolas, monospace;
}

.pf-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--pf-warn, #f5a623);
  display: inline-block;
}

.pf-icon-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 44px;
  height: 44px;
  flex: 0 0 44px;
  border: 0;
  border-radius: 10px;
  background: transparent;
  color: inherit;
  opacity: 0.8;
  cursor: pointer;
}

.pf-save-btn {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  min-height: 40px;
  padding: 0 14px;
  margin-right: 2px;
  border: 1px solid var(--highlight-text, #4f8cff);
  border-radius: 10px;
  background: color-mix(in srgb, var(--highlight-text, #4f8cff) 20%, transparent);
  color: var(--highlight-text, #4f8cff);
  font-size: 12px;
  font-weight: 700;
  cursor: pointer;
}

.pf-save-btn:disabled {
  opacity: 0.4;
  cursor: default;
}

.pf-create-path {
  display: flex;
  flex-direction: column;
  gap: 4px;
  padding: 8px 12px 6px;
  flex: 0 0 auto;
}

.pf-create-label {
  font-size: 10.5px;
  opacity: 0.55;
}

.pf-create-input {
  min-height: 44px;
  padding: 0 12px;
  border-radius: 10px;
  border: 1px solid var(--vcp-border-subtle, rgba(255, 255, 255, 0.12));
  background: var(--secondary-bg, rgba(255, 255, 255, 0.05));
  color: inherit;
  font-size: 12.5px;
  font-family: ui-monospace, 'SFMono-Regular', Menlo, Consolas, monospace;
  outline: none;
}

.pf-editor-banner {
  flex: 0 0 auto;
  margin: 0 12px 6px;
  padding: 7px 10px;
  border-radius: 8px;
  font-size: 11px;
  word-break: break-word;
}

.pf-editor-error {
  background: color-mix(in srgb, var(--pf-danger, #ff6b6b) 15%, transparent);
  color: var(--pf-danger, #ff6b6b);
}

.pf-editor-body {
  flex: 1;
  min-height: 0;
  display: flex;
  align-items: stretch;
  border-top: 1px solid var(--vcp-border-subtle, rgba(255, 255, 255, 0.06));
}

.pf-editor-gutter {
  flex: 0 0 3.2em;
  overflow: hidden;
  padding: 8px 6px 8px 0;
  text-align: right;
  font-family: ui-monospace, 'SFMono-Regular', Menlo, Consolas, monospace;
  font-size: 12px;
  line-height: 19px;
  opacity: 0.32;
  user-select: none;
}

.pf-gutter-line {
  height: 19px;
}

.pf-editor-textarea {
  flex: 1;
  min-width: 0;
  min-height: 0;
  padding: 8px 10px;
  border: 0;
  background: transparent;
  color: inherit;
  font-family: ui-monospace, 'SFMono-Regular', Menlo, Consolas, 'Liberation Mono', monospace;
  font-size: 12px;
  line-height: 19px;
  resize: none;
  outline: none;
  white-space: pre;
  overflow: auto;
  tab-size: 4;
  -webkit-text-size-adjust: none;
}

.pf-editor-actions {
  flex: 0 0 auto;
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 10px calc(8px + var(--vcp-safe-bottom, 0px));
  border-top: 1px solid var(--vcp-border-subtle, rgba(255, 255, 255, 0.08));
}

.pf-editor-hint {
  flex: 1;
  font-size: 9.5px;
  opacity: 0.4;
  line-height: 1.4;
}

.pf-action-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 6px;
  min-height: 44px;
  padding: 0 16px;
  border-radius: 10px;
  border: 1px solid var(--vcp-border-subtle, rgba(255, 255, 255, 0.12));
  background: var(--secondary-bg, rgba(255, 255, 255, 0.06));
  color: inherit;
  font-size: 12px;
  font-weight: 600;
  cursor: pointer;
}

.pf-action-btn:disabled {
  opacity: 0.4;
}

.pf-action-primary {
  flex: 1;
  border-color: var(--highlight-text, #4f8cff);
  background: color-mix(in srgb, var(--highlight-text, #4f8cff) 22%, transparent);
  color: var(--highlight-text, #4f8cff);
}

.pf-action-danger {
  color: var(--pf-danger, #ff6b6b);
  border-color: color-mix(in srgb, var(--pf-danger, #ff6b6b) 40%, transparent);
}

.pf-confirm-mask {
  position: absolute;
  inset: 0;
  z-index: 20;
  display: flex;
  align-items: flex-end;
  justify-content: center;
  padding: 12px;
  padding-bottom: calc(12px + var(--vcp-safe-bottom, 0px));
  background: rgba(0, 0, 0, 0.55);
}

.pf-confirm-card {
  width: 100%;
  max-width: 560px;
  max-height: 88%;
  display: flex;
  flex-direction: column;
  border-radius: 14px;
  border: 1px solid var(--vcp-border-subtle, rgba(255, 255, 255, 0.12));
  background: var(--secondary-bg, #242427);
  box-shadow: 0 10px 32px rgba(0, 0, 0, 0.45);
  overflow: hidden;
}

.pf-confirm-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 10px 8px 6px 14px;
  font-size: 13px;
}

.pf-confirm-body {
  padding: 0 14px 6px;
  overflow-y: auto;
}

.pf-confirm-line {
  display: flex;
  align-items: center;
  gap: 8px;
  margin: 0 0 8px;
  font-size: 11.5px;
}

.pf-confirm-label {
  font-size: 10.5px;
  opacity: 0.55;
  display: block;
  margin-bottom: 4px;
}

.pf-confirm-path {
  font-family: ui-monospace, 'SFMono-Regular', Menlo, Consolas, monospace;
  font-size: 11px;
  word-break: break-all;
}

.pf-confirm-warn {
  margin: 0 0 10px;
  padding: 8px 10px;
  border-radius: 8px;
  background: color-mix(in srgb, var(--pf-warn, #f5a623) 12%, transparent);
  color: var(--pf-warn, #f5a623);
  font-size: 10.5px;
  line-height: 1.5;
}

.pf-confirm-warn code {
  font-family: ui-monospace, 'SFMono-Regular', Menlo, Consolas, monospace;
}

.pf-confirm-reason {
  width: 100%;
  min-height: 60px;
  padding: 8px 10px;
  border-radius: 10px;
  border: 1px solid var(--vcp-border-subtle, rgba(255, 255, 255, 0.12));
  background: var(--primary-bg, rgba(0, 0, 0, 0.25));
  color: inherit;
  font-size: 12px;
  line-height: 1.5;
  resize: vertical;
  outline: none;
}

.pf-confirm-error {
  margin: 8px 0 0;
  font-size: 11px;
  color: var(--pf-danger, #ff6b6b);
}

.pf-confirm-actions {
  display: flex;
  gap: 8px;
  padding: 10px 14px calc(12px + var(--vcp-safe-bottom, 0px));
}

.pf-spin {
  animation: pf-spin 0.8s linear infinite;
}

@keyframes pf-spin {
  to {
    transform: rotate(360deg);
  }
}
</style>

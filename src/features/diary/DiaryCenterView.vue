<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, watch } from "vue";
import {
  ChevronDown,
  FolderCog,
  Plus,
  RefreshCw,
  Search,
  Sparkles,
  X,
} from "lucide-vue-next";
import SlidePage from "../../components/ui/SlidePage.vue";
import { useKeyboardInsets } from "../../core/composables/useKeyboardInsets";
import { useModalHistory } from "../../core/composables/useModalHistory";
import { useNotificationStore } from "../../core/stores/notification";
import { useOverlayStore } from "../../core/stores/overlay";
import DiaryActionSheet, { type DiarySheetAction } from "./components/DiaryActionSheet.vue";
import DiaryComposer from "./components/DiaryComposer.vue";
import DiaryEditor from "./components/DiaryEditor.vue";
import DiaryFolderSheet from "./components/DiaryFolderSheet.vue";
import DiaryNoteList from "./components/DiaryNoteList.vue";
import DiaryReader from "./components/DiaryReader.vue";
import DiaryRenameDialog from "./components/DiaryRenameDialog.vue";
import { useDiaryStore } from "./diaryStore";
import type { DiaryNoteKey, DiarySearchMode, DiarySearchScope } from "./types";

const props = defineProps<{
  isOpen: boolean;
  zIndex: number;
  openTarget?: DiaryNoteKey | null;
}>();

const emit = defineEmits<{
  close: [];
  targetConsumed: [];
}>();

type ActiveSheet = "folders" | "folderManager" | "actions" | "move" | null;
type NoteListExpose = {
  getScrollTop: () => number;
  restoreScrollTop: (value: number) => Promise<void>;
};

const diaryStore = useDiaryStore();
const overlayStore = useOverlayStore();
const notificationStore = useNotificationStore();
const { registerModal, unregisterModal } = useModalHistory();
const { keyboardHeight } = useKeyboardInsets();

const noteList = ref<NoteListExpose | null>(null);
const searchInput = ref<HTMLInputElement | null>(null);
const listScrollTop = ref(0);
const searchExpanded = ref(false);
const searchDraftMode = ref<Exclude<DiarySearchMode, "none">>("text");
const activeSheet = ref<ActiveSheet>(null);
const renameOpen = ref(false);
const pendingMoveKeys = ref<DiaryNoteKey[]>([]);
const internalRegistered = ref(false);
const sheetRegistered = ref(false);
const renameRegistered = ref(false);
const searchRegistered = ref(false);
const leaveConfirmPending = ref(false);
let textSearchTimer: ReturnType<typeof setTimeout> | null = null;

const listLoading = computed(() =>
  diaryStore.foldersLoading
  || (diaryStore.searchMode === "none" ? diaryStore.notesLoading : diaryStore.searchLoading),
);
const listRefreshing = computed(() =>
  diaryStore.notesRefreshing
  || (diaryStore.searchLoading && diaryStore.displayedNotes.length > 0),
);
const listError = computed(() => diaryStore.searchMode === "none"
  ? diaryStore.mutationError ?? diaryStore.notesError ?? diaryStore.foldersError
  : diaryStore.mutationError ?? diaryStore.searchError ?? diaryStore.foldersError,
);
const batchErrorSummary = computed(() => {
  const errors = diaryStore.lastBatchOutcome?.errors ?? [];
  if (errors.length === 0) return "";
  const first = errors[0];
  const suffix = errors.length > 1 ? `；另有 ${errors.length - 1} 项失败` : "";
  return `${first.key.folder}/${first.key.file}：${first.message}${suffix}`;
});
const needsSettings = computed(() => {
  const code = listError.value?.code ?? diaryStore.documentError?.code;
  return code === "DIARY_CONFIG_MISSING" || code === "DIARY_AUTH_REQUIRED";
});
const renameServerError = computed(() => diaryStore.mutationError?.message ?? "");
const readerActions = computed<DiarySheetAction[]>(() => [
  { id: "rename", label: "重命名", detail: "不覆盖同名文件" },
  { id: "move", label: "移动", detail: "服务端逐项裁决" },
  { id: "delete", label: "永久删除", danger: true },
]);

function toast(
  type: "info" | "success" | "warning" | "error",
  title: string,
  message: string,
): void {
  notificationStore.addNotification({ type, title, message, toastOnly: true });
}

function ensureInternalModal(): void {
  if (!props.isOpen || !diaryStore.hasInternalState || internalRegistered.value) return;
  internalRegistered.value = true;
  registerModal("Diary:Internal", () => handleInternalBack());
}

function releaseInternalModal(): void {
  if (!internalRegistered.value) return;
  internalRegistered.value = false;
  unregisterModal("Diary:Internal");
}

function finishInternalReduction(): boolean {
  if (diaryStore.hasInternalState) return false;
  releaseInternalModal();
  return true;
}

function scheduleEditorDiscardConfirm(): void {
  if (leaveConfirmPending.value) return;
  leaveConfirmPending.value = true;
  queueMicrotask(async () => {
    try {
      const confirmed = await overlayStore.showConfirm({
        title: "放弃未保存修改？",
        message: "草稿只保存在当前内存中。离开编辑态将丢弃这些修改。",
        isDanger: true,
      });
      if (confirmed) diaryStore.discardDraft();
    } finally {
      leaveConfirmPending.value = false;
    }
  });
}

function scheduleComposerDiscardConfirm(): void {
  if (leaveConfirmPending.value) return;
  leaveConfirmPending.value = true;
  queueMicrotask(async () => {
    try {
      const createUncertain = diaryStore.composerError?.code === "DIARY_CREATE_UNCERTAIN";
      const confirmed = await overlayStore.showConfirm({
        title: createUncertain ? "返回列表核对？" : "放弃新建内容？",
        message: createUncertain
          ? "远端可能已经创建日记。关闭本机表单不代表创建失败；返回后请先刷新目标文件夹核对。"
          : "尚未提交成功的 DailyNote 不会被保存。",
        isDanger: true,
      });
      if (confirmed) {
        diaryStore.discardComposer();
        setTimeout(() => finishInternalReduction(), 0);
      }
    } finally {
      leaveConfirmPending.value = false;
    }
  });
}

/** Return value is consumed synchronously by ModalHistory's close gate. */
function handleInternalBack(): boolean {
  if (diaryStore.isSaving || diaryStore.composerSubmitting || diaryStore.activeMutation) {
    toast("warning", "操作进行中", "完成远端核验后才能离开当前状态。");
    return false;
  }

  if (diaryStore.screen === "preview") {
    diaryStore.returnToEditor();
    return false;
  }

  if (diaryStore.screen === "editor") {
    if (diaryStore.draftDirty) {
      scheduleEditorDiscardConfirm();
      return false;
    }
    diaryStore.discardDraft();
    return false;
  }

  if (diaryStore.screen === "composer") {
    if (diaryStore.composerDirty) {
      scheduleComposerDiscardConfirm();
      return false;
    }
    diaryStore.discardComposer();
    return finishInternalReduction();
  }

  if (diaryStore.screen === "reader") {
    diaryStore.leaveReader();
    void restoreListScroll();
    return finishInternalReduction();
  }

  if (diaryStore.selectionMode) {
    diaryStore.clearSelection();
    return finishInternalReduction();
  }

  if (diaryStore.searchMode !== "none") {
    diaryStore.cancelSearch();
    searchExpanded.value = false;
    return finishInternalReduction();
  }

  return finishInternalReduction();
}

async function restoreListScroll(): Promise<void> {
  await nextTick();
  await noteList.value?.restoreScrollTop(listScrollTop.value);
}

function requestClose(): void {
  if (activeSheet.value) {
    closeSheet();
    return;
  }
  if (renameOpen.value) {
    closeRename();
    return;
  }
  if (searchExpanded.value) {
    closeSearch();
    return;
  }
  if (diaryStore.hasInternalState) {
    handleInternalBack();
    return;
  }
  emit("close");
}

function openSheet(sheet: Exclude<ActiveSheet, null>): void {
  activeSheet.value = sheet;
}

function closeSheet(): void {
  activeSheet.value = null;
}

function closeRename(): void {
  if (diaryStore.activeMutation) return;
  renameOpen.value = false;
  diaryStore.clearOperationMessages();
}

watch(activeSheet, (next, previous) => {
  if (next && !sheetRegistered.value) {
    sheetRegistered.value = true;
    registerModal("Diary:Sheet", () => {
      activeSheet.value = null;
      return true;
    });
  } else if (!next && previous && sheetRegistered.value) {
    sheetRegistered.value = false;
    unregisterModal("Diary:Sheet");
  }
});

watch(renameOpen, (open) => {
  if (open && !renameRegistered.value) {
    renameRegistered.value = true;
    registerModal("Diary:Dialog", () => {
      if (diaryStore.activeMutation) return false;
      renameOpen.value = false;
      return true;
    });
  } else if (!open && renameRegistered.value) {
    renameRegistered.value = false;
    unregisterModal("Diary:Dialog");
  }
});

watch(searchExpanded, (open) => {
  if (open && !searchRegistered.value) {
    searchRegistered.value = true;
    registerModal("Diary:Search", () => {
      closeSearch();
      return true;
    });
  } else if (!open && searchRegistered.value) {
    searchRegistered.value = false;
    unregisterModal("Diary:Search");
  }
});

watch(
  () => [props.isOpen, diaryStore.hasInternalState] as const,
  ([open, internal]) => {
    if (open && internal) ensureInternalModal();
    if (open && !internal) releaseInternalModal();
    if (!open) {
      activeSheet.value = null;
      renameOpen.value = false;
      searchExpanded.value = false;
      releaseInternalModal();
    }
  },
  { immediate: true },
);

async function consumeOpenTarget(target: DiaryNoteKey): Promise<void> {
  emit("targetConsumed");
  await diaryStore.initialize();
  if (target.folder !== diaryStore.selectedFolder) await diaryStore.selectFolder(target.folder);
  listScrollTop.value = noteList.value?.getScrollTop() ?? 0;
  await diaryStore.openNote(target);
}

watch(
  () => props.openTarget,
  (target) => {
    if (props.isOpen && target?.folder && target.file) void consumeOpenTarget({ ...target });
  },
  { immediate: true, deep: true },
);

watch(
  () => props.isOpen,
  (open) => {
    if (open) void diaryStore.initialize();
  },
  { immediate: true },
);

function clearTextSearchTimer(): void {
  if (!textSearchTimer) return;
  clearTimeout(textSearchTimer);
  textSearchTimer = null;
}

function scheduleTextSearch(): void {
  clearTextSearchTimer();
  if (!diaryStore.searchQuery.trim()) return;
  if (diaryStore.searchMode !== "text") diaryStore.setSearchMode("text");
  textSearchTimer = setTimeout(() => void diaryStore.runTextSearch(), 275);
}

function handleSearchInput(): void {
  clearTextSearchTimer();
  diaryStore.invalidateSearchInput();
  if (!diaryStore.searchQuery.trim()) {
    diaryStore.cancelSearch();
    return;
  }
  if (searchDraftMode.value === "semantic") return;
  scheduleTextSearch();
}

function waitForModalHistoryTurn(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

function chooseSearchMode(mode: Exclude<DiarySearchMode, "none">): void {
  clearTextSearchTimer();
  searchExpanded.value = true;
  searchDraftMode.value = mode;
  if (!diaryStore.searchQuery.trim()) {
    diaryStore.cancelSearch();
    return;
  }
  diaryStore.setSearchMode(mode);
  if (mode === "text") scheduleTextSearch();
}

function changeSearchScope(scope: DiarySearchScope): void {
  if (scope === diaryStore.searchScope) return;
  diaryStore.invalidateSearchInput();
  diaryStore.setSearchScope(scope);
  if (searchDraftMode.value === "text" && diaryStore.searchQuery.trim()) {
    diaryStore.setSearchMode("text");
    void diaryStore.runTextSearch();
  }
}

function clearSearchQuery(): void {
  clearTextSearchTimer();
  diaryStore.searchQuery = "";
  diaryStore.cancelSearch();
  void nextTick(() => searchInput.value?.focus());
}

async function openSearch(): Promise<void> {
  searchExpanded.value = true;
  if (diaryStore.searchMode !== "none") searchDraftMode.value = diaryStore.searchMode;
  await nextTick();
  searchInput.value?.focus();
}

function closeSearch(): void {
  clearTextSearchTimer();
  diaryStore.searchQuery = "";
  diaryStore.cancelSearch();
  searchExpanded.value = false;
}

function toggleSearch(): void {
  if (searchExpanded.value) closeSearch();
  else void openSearch();
}

function submitSearch(): void {
  clearTextSearchTimer();
  if (!diaryStore.searchQuery.trim()) return;
  diaryStore.setSearchMode(searchDraftMode.value);
  if (searchDraftMode.value === "semantic") void diaryStore.runSemanticSearch();
  else void diaryStore.runTextSearch();
}

async function openNote(key: DiaryNoteKey): Promise<void> {
  listScrollTop.value = noteList.value?.getScrollTop() ?? 0;
  await diaryStore.openNote(key);
}

async function refreshCurrent(): Promise<void> {
  if (diaryStore.screen === "reader") {
    const key = diaryStore.document?.key ?? diaryStore.documentTarget;
    if (key) await diaryStore.openNote(key, true);
    return;
  }
  if (diaryStore.searchMode === "text") await diaryStore.runTextSearch();
  else if (diaryStore.searchMode === "semantic") await diaryStore.runSemanticSearch();
  else if (diaryStore.selectedFolder) {
    const folder = diaryStore.selectedFolder;
    const notesWereCurrent = diaryStore.notesFolder === folder;
    await diaryStore.loadFolders(true);
    if (diaryStore.selectedFolder === folder && notesWereCurrent) {
      await diaryStore.loadNotes(folder, true);
    }
  }
  else await diaryStore.loadFolders(true);
}

function beginSelection(key: DiaryNoteKey): void {
  diaryStore.enterSelection(key);
}

async function copyDraft(): Promise<void> {
  try {
    await navigator.clipboard.writeText(diaryStore.draft);
    toast("success", "草稿已复制", "可在确认远端内容后安全恢复。");
  } catch {
    toast("error", "复制失败", "无法访问系统剪贴板。");
  }
}

async function loadRemoteAfterConfirm(): Promise<void> {
  const confirmed = await overlayStore.showConfirm({
    title: "加载远端内容？",
    message: "当前草稿会被远端版本替换。建议先复制草稿。",
    isDanger: true,
  });
  if (confirmed) await diaryStore.loadRemoteDraft();
}

async function forceSaveAfterConfirm(): Promise<void> {
  const confirmed = await overlayStore.showConfirm({
    title: "强制覆盖远端？",
    message: "这会忽略当前基线冲突并以本机草稿覆盖远端全文。",
    isDanger: true,
  });
  if (confirmed) await diaryStore.saveDraft(true);
}

async function handleReaderAction(id: string): Promise<void> {
  const key = diaryStore.document?.key ? { ...diaryStore.document.key } : null;
  if (!key) return;

  if (id === "move") {
    pendingMoveKeys.value = [key];
    activeSheet.value = "move";
    return;
  }

  closeSheet();
  await waitForModalHistoryTurn();
  if (id === "rename") {
    diaryStore.clearOperationMessages();
    renameOpen.value = true;
  } else if (id === "delete") {
    await confirmDelete([key]);
  }
}

async function confirmRename(file: string): Promise<void> {
  const outcome = await diaryStore.renameNote(file);
  if (!outcome) return;
  renameOpen.value = false;
  if (outcome.status === "copied_source_retained") {
    toast("warning", "已复制为新名称", "新文件已验证；旧文件未确认删除，按可能存在两份处理。");
  } else {
    toast("success", "重命名完成", outcome.key.file);
  }
}

function beginMove(keys: DiaryNoteKey[]): void {
  if (keys.length === 0) return;
  pendingMoveKeys.value = keys.map((key) => ({ ...key }));
  openSheet("move");
}

async function moveToFolder(folder: string): Promise<void> {
  const keys = pendingMoveKeys.value.map((key) => ({ ...key }));
  closeSheet();
  const outcome = await diaryStore.moveNotes(keys, folder);
  pendingMoveKeys.value = [];
  if (!outcome) {
    if (diaryStore.mutationError) {
      toast("error", "移动失败", diaryStore.mutationError.message);
    }
    return;
  }
  if (outcome.errors.length > 0) {
    toast("warning", "部分文件未移动", `成功 ${outcome.succeeded.length}，失败 ${outcome.errors.length}。`);
  } else {
    toast("success", "移动完成", `已移动 ${outcome.succeeded.length} 个文件。`);
  }
}

async function confirmDelete(keys: DiaryNoteKey[]): Promise<void> {
  if (keys.length === 0) return;
  const confirmed = await overlayStore.showConfirm({
    title: keys.length === 1 ? "永久删除文件？" : `永久删除 ${keys.length} 个文件？`,
    message: "删除不可撤销。部分文件失败时，成功项仍会被删除。",
    isDanger: true,
  });
  if (!confirmed) return;
  const outcome = await diaryStore.deleteNotes(keys);
  if (!outcome) {
    if (diaryStore.mutationError) {
      toast("error", "删除失败", diaryStore.mutationError.message);
    }
    return;
  }
  if (outcome.errors.length > 0) {
    toast("warning", "部分文件未删除", `成功 ${outcome.succeeded.length}，失败 ${outcome.errors.length}。`);
  } else {
    toast("success", "删除完成", `已删除 ${outcome.succeeded.length} 个文件。`);
  }
}

async function requestDeleteFolder(folder: string): Promise<void> {
  closeSheet();
  await waitForModalHistoryTurn();
  const confirmed = await overlayStore.showConfirm({
    title: "删除空文件夹？",
    message: `仅当“${folder}”为空时服务端才会删除；非空会安全拒绝。`,
    isDanger: true,
  });
  if (!confirmed) return;
  const deleted = await diaryStore.deleteEmptyFolder(folder);
  if (deleted) toast("success", "文件夹已删除", folder);
  else if (diaryStore.mutationError) toast("error", "删除失败", diaryStore.mutationError.message);
}

function openSettings(): void {
  overlayStore.openSettings();
}

onBeforeUnmount(() => {
  clearTextSearchTimer();
  if (sheetRegistered.value) unregisterModal("Diary:Sheet");
  if (renameRegistered.value) unregisterModal("Diary:Dialog");
  if (searchRegistered.value) unregisterModal("Diary:Search");
  releaseInternalModal();
});
</script>

<template>
  <SlidePage :is-open="isOpen" :z-index="zIndex">
    <main class="diary-center-root relative h-full min-h-0 overflow-hidden no-swipe bg-[var(--primary-bg)] text-[var(--primary-text)]">
      <DiaryReader
        v-if="diaryStore.screen === 'reader'"
        :document="diaryStore.document"
        :loading="diaryStore.documentLoading"
        :refreshing="diaryStore.documentRefreshing"
        :error="diaryStore.documentError"
        :highlight-term="diaryStore.searchMode === 'text' ? diaryStore.searchQuery : ''"
        @back="requestClose"
        @refresh="refreshCurrent"
        @edit="diaryStore.startEditing()"
        @more="openSheet('actions')"
      />

      <DiaryEditor
        v-else-if="(diaryStore.screen === 'editor' || diaryStore.screen === 'preview') && diaryStore.document"
        :document="diaryStore.document"
        :draft="diaryStore.draft"
        :dirty="diaryStore.draftDirty"
        :preview="diaryStore.screen === 'preview'"
        :save-state="diaryStore.saveState"
        :error="diaryStore.saveError"
        :keyboard-height="keyboardHeight"
        @back="requestClose"
        @update="diaryStore.setDraft"
        @preview="diaryStore.showPreview()"
        @edit="diaryStore.returnToEditor()"
        @save="diaryStore.saveDraft(false)"
        @copy="copyDraft"
        @reload="loadRemoteAfterConfirm"
        @force="forceSaveAfterConfirm"
      />

      <DiaryComposer
        v-else-if="diaryStore.screen === 'composer'"
        :draft="diaryStore.composerDraft"
        :folders="diaryStore.orderedFolders"
        :submitting="diaryStore.composerSubmitting"
        :error="diaryStore.composerError"
        :keyboard-height="keyboardHeight"
        @back="requestClose"
        @update="Object.assign(diaryStore.composerDraft, $event)"
        @submit="diaryStore.createNote()"
      />

      <section v-else class="h-full min-h-0 flex flex-col">
        <header class="diary-list-header shrink-0" data-diary-role="list-header">
          <div class="diary-topbar">
            <button type="button" class="diary-icon-button" aria-label="关闭日记中心" @click="requestClose">
              <X :size="20" />
            </button>

            <template v-if="diaryStore.selectionMode">
              <div class="min-w-0 flex-1 px-2">
                <span class="diary-eyebrow">BATCH SELECT</span>
                <strong class="block mt-0.5 text-[15px]">已选 {{ diaryStore.selectedKeyIds.length }} 项</strong>
              </div>
              <button type="button" class="diary-header-text-button" @click="diaryStore.clearSelection()">完成</button>
            </template>

            <template v-else>
              <button
                type="button"
                class="diary-title-button"
                aria-label="选择日记文件夹"
                @click="openSheet('folders')"
              >
                <span class="diary-eyebrow">VCP MEMO</span>
                <span class="diary-folder-title">
                  <span class="truncate">{{ diaryStore.selectedFolder || "选择文件夹" }}</span>
                  <ChevronDown :size="14" class="shrink-0" />
                </span>
              </button>
              <button
                type="button"
                class="diary-icon-button"
                :class="searchExpanded ? 'active' : ''"
                :aria-label="searchExpanded ? '关闭搜索' : '搜索日记'"
                :aria-expanded="searchExpanded"
                @click="toggleSearch"
              >
                <Search :size="18" />
              </button>
              <button type="button" class="diary-icon-button" aria-label="刷新" :disabled="listLoading" @click="refreshCurrent">
                <RefreshCw :size="17" :class="listRefreshing ? 'animate-spin' : ''" />
              </button>
              <button type="button" class="diary-icon-button" aria-label="管理文件夹" @click="openSheet('folderManager')">
                <FolderCog :size="18" />
              </button>
              <button type="button" class="diary-icon-button" aria-label="新建日记" @click="diaryStore.startComposer()">
                <Plus :size="20" />
              </button>
            </template>
          </div>

          <Transition name="diary-search-panel">
            <div
              v-if="searchExpanded && !diaryStore.selectionMode"
              class="diary-search-panel"
              data-diary-role="search-panel"
            >
              <label class="diary-search-field">
                <Search :size="16" class="shrink-0" />
                <input
                  ref="searchInput"
                  v-model="diaryStore.searchQuery"
                  class="min-w-0 h-full flex-1 border-0 bg-transparent text-sm text-[var(--primary-text)] outline-none"
                  placeholder="搜索记忆内容"
                  aria-label="搜索日记"
                  @input="handleSearchInput"
                  @keydown.enter="submitSearch"
                />
                <button
                  v-if="diaryStore.searchQuery"
                  type="button"
                  class="diary-search-clear"
                  aria-label="清空搜索"
                  @click.prevent="clearSearchQuery"
                >
                  <X :size="15" />
                </button>
                <button
                  v-if="searchDraftMode === 'semantic'"
                  type="button"
                  class="diary-search-submit"
                  :disabled="diaryStore.searchLoading || !diaryStore.searchQuery.trim()"
                  @click.prevent="submitSearch"
                >
                  检索
                </button>
              </label>

              <div class="diary-search-controls">
                <div class="diary-chip-group" role="group" aria-label="搜索方式">
                  <button
                    type="button"
                    class="diary-segment"
                    :class="searchDraftMode === 'text' ? 'active' : ''"
                    @click="chooseSearchMode('text')"
                  >
                    <Search :size="13" />普通
                  </button>
                  <button
                    type="button"
                    class="diary-segment"
                    :class="searchDraftMode === 'semantic' ? 'active' : ''"
                    @click="chooseSearchMode('semantic')"
                  >
                    <Sparkles :size="13" />语义
                  </button>
                </div>
                <span class="diary-search-divider" aria-hidden="true" />
                <div class="diary-chip-group" role="group" aria-label="搜索范围">
                  <button type="button" class="diary-segment" :class="diaryStore.searchScope === 'folder' ? 'active' : ''" @click="changeSearchScope('folder')">
                    当前
                  </button>
                  <button type="button" class="diary-segment" :class="diaryStore.searchScope === 'all' ? 'active' : ''" @click="changeSearchScope('all')">
                    全部
                  </button>
                </div>
              </div>
            </div>
          </Transition>
        </header>

        <div
          v-if="diaryStore.searchLimited || diaryStore.indexMayBeCatchingUp || listError || diaryStore.lastBatchOutcome?.errors.length"
          class="shrink-0 px-4 py-2 border-b border-[var(--border-color)] bg-[var(--secondary-bg)] text-[11px]"
          role="status"
        >
          <span v-if="listError" class="text-[var(--danger-color)]">{{ listError.message }}</span>
          <span v-else-if="diaryStore.lastBatchOutcome?.errors.length" class="text-[var(--danger-color)]">
            {{ batchErrorSummary }}。失败项仍保持选中。
          </span>
          <span v-else-if="diaryStore.searchLimited">结果达到上限，请缩小搜索范围。</span>
          <span v-else>文件已写入；语义索引可能仍在追平。</span>
          <button
            v-if="needsSettings"
            type="button"
            class="ml-2 min-h-12 inline-flex items-center border-0 bg-transparent underline text-[var(--highlight-text)]"
            @click="openSettings"
          >
            前往设置
          </button>
        </div>

        <DiaryNoteList
          ref="noteList"
          :notes="diaryStore.displayedNotes"
          :loading="listLoading"
          :refreshing="listRefreshing"
          :search-mode="diaryStore.searchMode"
          :selection-mode="diaryStore.selectionMode"
          :selected-ids="diaryStore.selectedKeyIds"
          @open="openNote"
          @select="diaryStore.toggleSelection"
          @longpress="beginSelection"
        />

        <footer
          v-if="diaryStore.selectionMode"
          class="shrink-0 grid grid-cols-2 gap-2 px-4 pt-2 pb-[calc(var(--vcp-safe-bottom,48px)+8px)] border-t border-[var(--border-color)] bg-[var(--primary-bg)]"
        >
          <button type="button" class="diary-batch-button" :disabled="diaryStore.selectedKeys.length === 0 || Boolean(diaryStore.activeMutation)" @click="beginMove(diaryStore.selectedKeys)">
            移动
          </button>
          <button type="button" class="diary-batch-button danger" :disabled="diaryStore.selectedKeys.length === 0 || Boolean(diaryStore.activeMutation)" @click="confirmDelete(diaryStore.selectedKeys)">
            删除
          </button>
        </footer>
      </section>

      <DiaryFolderSheet
        :open="activeSheet === 'folders' || activeSheet === 'folderManager' || activeSheet === 'move'"
        :mode="activeSheet === 'folderManager' ? 'manage' : activeSheet === 'move' ? 'move' : 'select'"
        :folders="diaryStore.orderedFolders"
        :hidden-folders="diaryStore.hiddenFolders"
        :collapsed-categories="diaryStore.collapsedCategories"
        :selected-folder="diaryStore.selectedFolder"
        @close="closeSheet"
        @select="(folder) => { closeSheet(); diaryStore.selectFolder(folder); }"
        @move="moveToFolder"
        @hide="diaryStore.hideFolder"
        @restore="diaryStore.restoreFolder"
        @toggle-category="diaryStore.toggleCategoryCollapsed"
        @reorder="diaryStore.setFolderOrder"
        @delete-folder="requestDeleteFolder"
      />

      <DiaryActionSheet
        :open="activeSheet === 'actions'"
        title="文件操作"
        :actions="readerActions"
        @close="closeSheet"
        @action="handleReaderAction"
      />

      <DiaryRenameDialog
        :open="renameOpen"
        :current-file="diaryStore.document?.key.file || ''"
        :busy="Boolean(diaryStore.activeMutation)"
        :server-error="renameServerError"
        @close="closeRename"
        @confirm="confirmRename"
      />
    </main>
  </SlidePage>
</template>

<style scoped>
.diary-center-root {
  --diary-muted-text: var(--secondary-text, var(--primary-text));
  --diary-surface: var(--secondary-bg);
  --diary-surface-soft: var(--primary-bg);
  --diary-line: var(--border-color);
  --diary-surface-focus: var(--secondary-bg);
  --diary-highlight-line-subtle: var(--highlight-text);
  --diary-highlight-line: var(--highlight-text);
  --diary-highlight-line-strong: var(--highlight-text);
  --diary-highlight-surface: var(--accent-bg, var(--secondary-bg));
  --diary-highlight-surface-transparent: var(--accent-bg, var(--secondary-bg));
  --diary-muted-surface: var(--secondary-bg);
  --diary-loading-surface: var(--secondary-bg);
  --diary-warning-surface: var(--warning-color, #eab308);
  --diary-focus-outline: var(--highlight-text);
  --diary-header-safe-top: max(var(--vcp-safe-top, 0px), 32px);
}

@supports (background-color: color-mix(in srgb, black, transparent)) {
  .diary-center-root {
    --diary-muted-text: var(
      --secondary-text,
      color-mix(in srgb, var(--primary-text) 64%, transparent)
    );
    --diary-surface: color-mix(in srgb, var(--secondary-bg) 78%, var(--primary-bg));
    --diary-surface-soft: color-mix(in srgb, var(--secondary-bg) 42%, var(--primary-bg));
    --diary-line: color-mix(in srgb, var(--border-color) 72%, transparent);
    --diary-surface-focus: color-mix(in srgb, var(--secondary-bg) 88%, var(--primary-bg));
    --diary-highlight-line-subtle: color-mix(in srgb, var(--highlight-text) 36%, var(--diary-line));
    --diary-highlight-line: color-mix(in srgb, var(--highlight-text) 50%, var(--diary-line));
    --diary-highlight-line-strong: color-mix(in srgb, var(--highlight-text) 58%, var(--diary-line));
    --diary-highlight-surface: color-mix(in srgb, var(--highlight-text) 9%, var(--diary-surface-soft));
    --diary-highlight-surface-transparent: color-mix(in srgb, var(--highlight-text) 9%, transparent);
    --diary-muted-surface: color-mix(in srgb, var(--diary-muted-text) 34%, transparent);
    --diary-loading-surface: color-mix(in srgb, var(--diary-muted-text) 16%, transparent);
    --diary-warning-surface: color-mix(in srgb, var(--warning-color, #eab308) 34%, transparent);
    --diary-focus-outline: color-mix(in srgb, var(--highlight-text) 70%, transparent);
  }
}

.diary-list-header {
  padding-top: var(--diary-header-safe-top);
  border-bottom: 1px solid var(--diary-line);
  background: var(--primary-bg);
}

.diary-topbar {
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

.diary-icon-button.active,
.diary-icon-button:active {
  background: var(--diary-surface);
}

.diary-icon-button:disabled {
  opacity: 0.3;
}

.diary-title-button {
  min-width: 0;
  min-height: 56px;
  flex: 1;
  padding: 7px 6px 6px 8px;
  display: flex;
  flex-direction: column;
  justify-content: center;
  align-items: flex-start;
  border: 0;
  border-radius: 10px;
  background: transparent;
  color: var(--primary-text);
  text-align: left;
}

.diary-title-button:active {
  background: var(--diary-surface-soft);
}

.diary-eyebrow {
  display: block;
  color: var(--diary-muted-text);
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 9px;
  font-weight: 700;
  line-height: 12px;
  letter-spacing: 0.16em;
}

.diary-folder-title {
  width: 100%;
  min-width: 0;
  margin-top: 2px;
  display: flex;
  align-items: center;
  gap: 5px;
  font-size: 16px;
  font-weight: 700;
  line-height: 20px;
}

.diary-header-text-button {
  min-height: 48px;
  padding: 0 12px;
  border: 0;
  border-radius: 12px;
  background: transparent;
  color: var(--highlight-text);
  font-weight: 600;
}

.diary-search-panel {
  padding: 0 12px 12px;
}

.diary-search-field {
  height: 46px;
  box-sizing: border-box;
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 0 5px 0 14px;
  border: 1px solid var(--diary-line);
  border-radius: 999px;
  background: var(--diary-surface);
  color: var(--diary-muted-text);
  transition: border-color 140ms ease, background-color 140ms ease;
}

.diary-search-field:focus-within {
  border-color: var(--diary-highlight-line-strong);
  background: var(--diary-surface-focus);
}

.diary-search-field input:focus,
.diary-search-field input:focus-visible {
  outline: none;
  box-shadow: none;
}

.diary-search-clear {
  width: 40px;
  height: 40px;
  flex: 0 0 auto;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border: 0;
  border-radius: 999px;
  background: transparent;
  color: var(--diary-muted-text);
}

.diary-search-submit {
  min-width: 52px;
  height: 36px;
  padding: 0 12px;
  border: 1px solid var(--diary-highlight-line);
  border-radius: 999px;
  background: var(--diary-highlight-surface-transparent);
  color: var(--highlight-text);
  font-size: 11px;
  font-weight: 700;
}

.diary-search-submit:disabled {
  opacity: 0.35;
}

.diary-search-controls {
  min-height: 44px;
  margin-top: 6px;
  display: flex;
  align-items: center;
  gap: 7px;
  overflow-x: auto;
  scrollbar-width: none;
}

.diary-search-controls::-webkit-scrollbar {
  display: none;
}

.diary-chip-group {
  display: flex;
  align-items: center;
  gap: 4px;
}

.diary-search-divider {
  width: 1px;
  height: 18px;
  flex: 0 0 auto;
  background: var(--diary-line);
}

.diary-segment {
  min-height: 40px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 4px;
  padding: 0 11px;
  border: 1px solid transparent;
  border-radius: 999px;
  background: transparent;
  color: var(--diary-muted-text);
  font-size: 11px;
  font-weight: 600;
}

.diary-segment.active {
  border-color: var(--diary-highlight-line);
  color: var(--highlight-text);
  background: var(--diary-highlight-surface);
}

.diary-batch-button {
  min-height: 48px;
  border: 1px solid var(--diary-highlight-line);
  border-radius: 12px;
  background: var(--diary-surface);
  color: var(--highlight-text);
  font-weight: 700;
}

.diary-batch-button.danger {
  border-color: var(--danger-color);
  color: var(--danger-color);
}

.diary-batch-button:disabled {
  opacity: 0.35;
}

.diary-search-panel-enter-active,
.diary-search-panel-leave-active {
  transition: opacity 140ms ease, transform 140ms ease;
}

.diary-search-panel-enter-from,
.diary-search-panel-leave-to {
  opacity: 0;
  transform: translateY(-4px);
}

button:focus-visible {
  outline: 2px solid var(--diary-focus-outline);
  outline-offset: 1px;
}

@media (prefers-reduced-motion: reduce) {
  .animate-spin,
  .diary-search-panel-enter-active,
  .diary-search-panel-leave-active {
    animation: none;
    transition: none;
  }
}
</style>

<script setup lang="ts">
/**
 * V工程「源码」模式 shell（对齐桌面 VCPChat 源码侧栏）。
 *
 * - 工作区 / 工程选择（ProjectForge ListWorkspaces + ListProjects；工程根目录取 GetProject.root）；
 * - 按文件名搜索 + 懒加载文件树（FileOperator，失败自动降级 PTY）；
 * - 点文件进入只读查看 / 编辑子页；新建文件；
 * - 写操作强确认集中在这里：保存（在 PfCodeEditor 内）、新建（PfCodeEditor 内）、删除、移动/重命名；
 * - 未保存离开确认（编辑子页返回 / 切换工作区/工程 / 刷新时拦截）。
 */
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import {
  AlertTriangle,
  Check,
  FilePlus2,
  FolderInput,
  FolderTree,
  Loader2,
  RefreshCw,
  Search,
  Trash2,
  X,
} from "@lucide/vue";
import PfFileTree from './PfFileTree.vue';
import PfCodeReader from './PfCodeReader.vue';
import PfCodeEditor from './PfCodeEditor.vue';
import { useProjectForgeSource } from '../useProjectForgeSource';

const props = withDefaults(defineProps<{ isOpen?: boolean }>(), { isOpen: true });
const emit = defineEmits<{ 'dirty-change': [dirty: boolean] }>();

type Views = 'tree' | 'reader' | 'editor';

const source = useProjectForgeSource();
const {
  workspaces,
  workspacesLoading,
  workspacesError,
  selectedWorkspaceAlias,
  projects,
  projectsLoading,
  projectsError,
  selectedProjectId,
  projectRoot,
  projectLoading,
  projectError,
  transportNote,
  connectionLabel,
  treeRows,
  treeLoading,
  treeError,
  searchQuery,
  searchResults,
  searchTotal,
  searchLoading,
  searchError,
  searchNote,
  runSearch,
  clearSearch,
  toggleDir,
  refreshTree,
  currentFile,
  initSource,
  refreshAll,
  selectWorkspace,
  selectProject,
  openFile,
  removeFile,
  moveFile,
  actionError,
  lastWriteNotice,
  actionBusy,
  clearActionError,
} = source;

const view = ref<Views>('tree');
const createMode = ref(false);
const editorRef = ref<InstanceType<typeof PfCodeEditor> | null>(null);
const editorDirty = ref(false);

// 未保存修改状态外抛：页面 shell 关闭 V工程 前可据此拦截（同时暴露 hasUnsavedChanges 方法）。
watch(
  [view, editorDirty],
  () => {
    emit('dirty-change', view.value === 'editor' && editorDirty.value);
  },
  { immediate: true },
);

/** 供页面 shell（Lead）在关闭整页 / 切换分段前检查是否有未保存修改。 */
defineExpose({
  hasUnsavedChanges: (): boolean => view.value === 'editor' && editorDirty.value,
});

// —— 未保存离开确认 ——
const leaveOpen = ref(false);
const pendingAction = ref<(() => void | Promise<void>) | null>(null);

// —— 删除确认 ——
const deleteOpen = ref(false);
const deleteTarget = ref('');
const deleteReason = ref('');
const deleteError = ref('');

// —— 移动确认 ——
const moveOpen = ref(false);
const moveFrom = ref('');
const moveTo = ref('');
const moveReason = ref('');
const moveError = ref('');

let searchTimer: ReturnType<typeof setTimeout> | null = null;

const rootLabel = computed(() => projectRoot.value || '（未选择工程）');

const deleteBusy = computed(() => actionBusy.value);
const moveBusy = computed(() => actionBusy.value);

const errorBanner = computed(() => {
  return projectError.value || projectsError.value || workspacesError.value || actionError.value || '';
});

const hasProjects = computed(() => projects.value.length > 0);

onMounted(() => {
  if (props.isOpen) void initSource();
});

watch(
  () => props.isOpen,
  (open) => {
    if (open) void initSource();
  },
);

onBeforeUnmount(() => {
  if (searchTimer) clearTimeout(searchTimer);
});

// ============================ 搜索 ============================

function onSearchInput(): void {
  if (searchTimer) clearTimeout(searchTimer);
  const keyword = String(searchQuery.value ?? '').trim();
  if (!keyword) {
    clearSearch();
    return;
  }
  searchTimer = setTimeout(() => {
    void runSearch();
  }, 300);
}

function onSearchSubmit(): void {
  if (searchTimer) clearTimeout(searchTimer);
  if (!String(searchQuery.value ?? '').trim()) {
    clearSearch();
    return;
  }
  void runSearch();
}

// ============================ 未保存离开守卫 ============================

function guardLeave(action: () => void | Promise<void>): void {
  if (view.value === 'editor' && editorDirty.value) {
    pendingAction.value = action;
    leaveOpen.value = true;
    return;
  }
  void action();
}

function cancelLeave(): void {
  leaveOpen.value = false;
  pendingAction.value = null;
}

function confirmDiscardAndLeave(): void {
  const action = pendingAction.value;
  leaveOpen.value = false;
  pendingAction.value = null;
  editorRef.value?.resetDraft();
  editorDirty.value = false;
  if (action) void action();
}

// ============================ 工作区 / 工程选择 ============================

function onWorkspaceChange(event: Event): void {
  const alias = (event.target as HTMLSelectElement).value;
  if (!alias || alias === selectedWorkspaceAlias.value) return;
  guardLeave(async () => {
    view.value = 'tree';
    try {
      await selectWorkspace(alias);
    } catch {
      // 错误已显示在 errorBanner / treeError
    }
  });
}

function onProjectChange(event: Event): void {
  const projectId = (event.target as HTMLSelectElement).value;
  if (!projectId || projectId === selectedProjectId.value) return;
  guardLeave(async () => {
    view.value = 'tree';
    try {
      await selectProject(projectId);
    } catch {
      // 错误已显示
    }
  });
}

function onRefreshAll(): void {
  guardLeave(async () => {
    await refreshAll();
  });
}

function onRefreshTree(): void {
  guardLeave(async () => {
    clearSearch();
    await refreshTree({ retryTransport: true });
  });
}

function onTreeRetry(): void {
  void refreshTree({ retryTransport: true });
}

// ============================ 打开文件 / 子页切换 ============================

async function openReader(path: string): Promise<void> {
  try {
    await openFile(path);
  } catch {
    // fileError 会显示在 PfCodeReader
  }
  createMode.value = false;
  view.value = 'reader';
}

function onTreeOpen(path: string): void {
  if (!path) return;
  guardLeave(() => openReader(path));
}

function onReaderBack(): void {
  view.value = 'tree';
}

function onReaderEdit(): void {
  if (!currentFile.value) return;
  createMode.value = false;
  view.value = 'editor';
}

function onEditorBack(): void {
  if (editorDirty.value) {
    pendingAction.value = () => {
      leaveEditor();
    };
    leaveOpen.value = true;
    return;
  }
  leaveEditor();
}

function leaveEditor(): void {
  const wasCreate = createMode.value;
  createMode.value = false;
  view.value = wasCreate ? 'tree' : 'reader';
}

function onEditorSaved(): void {
  createMode.value = false;
  view.value = 'reader';
}

function onEditorCreated(): void {
  createMode.value = false;
  view.value = 'reader';
}

function onStartCreate(): void {
  guardLeave(() => {
    createMode.value = true;
    view.value = 'editor';
  });
}

function onReaderOpenFile(path: string): void {
  void openReader(path);
}

// ============================ 删除 ============================

function openDeleteConfirm(path: string): void {
  if (!path) return;
  deleteTarget.value = path;
  deleteReason.value = '';
  deleteError.value = '';
  deleteOpen.value = true;
}

function closeDeleteConfirm(): void {
  if (deleteBusy.value) return;
  deleteOpen.value = false;
}

async function confirmDelete(): Promise<void> {
  if (!deleteReason.value.trim()) {
    deleteError.value = '请填写删除原因（reason 必填，会写入桌面施工记录）';
    return;
  }
  deleteError.value = '';
  try {
    await removeFile(deleteTarget.value, deleteReason.value);
    deleteOpen.value = false;
    deleteReason.value = '';
    view.value = 'tree';
    createMode.value = false;
  } catch (error) {
    deleteError.value = error instanceof Error ? error.message : String(error);
  }
}

// ============================ 移动 / 重命名 ============================

function openMoveConfirm(path: string): void {
  if (!path) return;
  moveFrom.value = path;
  moveTo.value = path;
  moveReason.value = '';
  moveError.value = '';
  moveOpen.value = true;
}

function closeMoveConfirm(): void {
  if (moveBusy.value) return;
  moveOpen.value = false;
}

async function confirmMove(): Promise<void> {
  const from = moveFrom.value.trim();
  const to = moveTo.value.trim();
  if (!to) {
    moveError.value = '请填写目标路径';
    return;
  }
  if (from === to) {
    moveError.value = '目标路径与原路径相同，无需移动';
    return;
  }
  if (!moveReason.value.trim()) {
    moveError.value = '请填写移动原因（reason 必填，会写入桌面施工记录）';
    return;
  }
  moveError.value = '';
  try {
    await moveFile({ from, to, reason: moveReason.value });
    moveOpen.value = false;
    moveReason.value = '';
    view.value = 'reader';
    createMode.value = false;
  } catch (error) {
    moveError.value = error instanceof Error ? error.message : String(error);
  }
}

function onReaderDelete(): void {
  openDeleteConfirm(currentFile.value?.path ?? '');
}

function onReaderMove(): void {
  openMoveConfirm(currentFile.value?.path ?? '');
}

function onEditorDelete(): void {
  openDeleteConfirm(currentFile.value?.path ?? '');
}

function onEditorMove(): void {
  openMoveConfirm(currentFile.value?.path ?? '');
}
</script>

<template>
  <section class="pf-source" aria-label="V工程源码模式">
    <!-- 树视图的头部 -->
    <template v-if="view === 'tree'">
      <header class="pf-source-head">
        <label class="pf-select-box">
          <span class="pf-select-label">工作区</span>
          <select
            class="pf-select"
            :value="selectedWorkspaceAlias"
            :disabled="workspacesLoading || !workspaces.length"
            aria-label="选择工作区"
            @change="onWorkspaceChange"
          >
            <option v-if="!workspaces.length" value="">（未登记工作区）</option>
            <option v-for="workspace in workspaces" :key="workspace.id || workspace.alias" :value="workspace.alias">
              {{ workspace.alias }}{{ workspace.enabled === false ? '（已停用）' : '' }}
            </option>
          </select>
        </label>
        <label class="pf-select-box">
          <span class="pf-select-label">工程</span>
          <select
            class="pf-select"
            :value="selectedProjectId"
            :disabled="projectsLoading || !hasProjects"
            aria-label="选择工程"
            @change="onProjectChange"
          >
            <option v-if="!hasProjects" value="">（无工程）</option>
            <option v-for="project in projects" :key="project.id" :value="project.id">
              {{ project.name || project.id }}（{{ project.id }}）
            </option>
          </select>
        </label>
        <button
          type="button"
          class="pf-icon-btn"
          :disabled="workspacesLoading || projectsLoading"
          title="刷新工作区与工程"
          aria-label="刷新"
          @click="onRefreshAll"
        >
          <RefreshCw :size="18" :class="{ 'pf-spin': workspacesLoading || projectsLoading }" />
        </button>
      </header>

      <div class="pf-source-subhead">
        <div class="pf-search-box">
          <Search :size="15" class="pf-search-icon" />
          <input
            v-model="searchQuery"
            class="pf-search-input"
            type="search"
            placeholder="按文件名搜索（空格分隔多个关键字）"
            autocomplete="off"
            spellcheck="false"
            aria-label="按文件名搜索"
            @input="onSearchInput"
            @keydown.enter.prevent="onSearchSubmit"
          />
          <button
            v-if="searchQuery"
            type="button"
            class="pf-search-clear"
            aria-label="清空搜索"
            @click="clearSearch()"
          >
            <X :size="14" />
          </button>
        </div>
        <button type="button" class="pf-new-btn" title="新建文件" @click="onStartCreate">
          <FilePlus2 :size="16" />
          新建
        </button>
        <button type="button" class="pf-icon-btn" title="重新扫描文件树" aria-label="刷新文件树" @click="onRefreshTree">
          <RefreshCw :size="17" :class="{ 'pf-spin': treeLoading || searchLoading }" />
        </button>
      </div>

      <div class="pf-source-meta">
        <span class="pf-meta-root" :title="rootLabel">
          <FolderTree :size="12" />
          {{ rootLabel }}
        </span>
        <span class="pf-meta-transport">{{ connectionLabel }}</span>
        <span v-if="projectLoading || projectsLoading" class="pf-meta-loading">
          <Loader2 :size="12" class="pf-spin" /> 加载中
        </span>
      </div>

      <div v-if="transportNote" class="pf-banner pf-banner-info">{{ transportNote }}</div>
      <div v-if="errorBanner" class="pf-banner pf-banner-error">
        <span class="pf-banner-text">{{ errorBanner }}</span>
        <button type="button" class="pf-banner-close" aria-label="关闭错误提示" @click="clearActionError">
          <X :size="14" />
        </button>
      </div>
      <div v-if="lastWriteNotice && !errorBanner" class="pf-banner pf-banner-ok">
        <Check :size="14" />
        {{ lastWriteNotice }}
      </div>
      <div v-if="searchNote" class="pf-banner pf-banner-info">{{ searchNote }}</div>
      <div v-if="searchError" class="pf-banner pf-banner-error">{{ searchError }}</div>

      <div class="pf-source-body">
        <PfFileTree
          :rows="treeRows"
          :results="searchResults"
          :search-query="searchQuery"
          :searching="searchLoading"
          :active-path="currentFile?.path ?? ''"
          :loading="treeLoading || projectLoading"
          :error="treeError"
          empty-text="工程根目录为空"
          @open="onTreeOpen"
          @toggle="toggleDir"
          @retry="onTreeRetry"
        />
      </div>

      <div v-if="searchQuery.trim()" class="pf-source-status">
        匹配 {{ searchTotal }} 个文件<template v-if="searchLoading"> · 搜索中…</template>
      </div>
    </template>

    <!-- 只读查看子页 -->
    <PfCodeReader
      v-else-if="view === 'reader'"
      @back="onReaderBack"
      @edit="onReaderEdit"
      @delete="onReaderDelete"
      @move="onReaderMove"
      @open-file="onReaderOpenFile"
    />

    <!-- 编辑 / 新建子页 -->
    <PfCodeEditor
      v-else
      ref="editorRef"
      :mode="createMode ? 'create' : 'edit'"
      @back="onEditorBack"
      @saved="onEditorSaved"
      @created="onEditorCreated"
      @delete="onEditorDelete"
      @move="onEditorMove"
      @dirty-change="editorDirty = $event"
    />

    <!-- 未保存离开确认 -->
    <div v-if="leaveOpen" class="pf-mask" role="dialog" aria-modal="true" aria-label="未保存修改">
      <div class="pf-dialog">
        <header class="pf-dialog-head">
          <strong>放弃未保存的修改？</strong>
          <button type="button" class="pf-icon-btn" aria-label="关闭" @click="cancelLeave">
            <X :size="18" />
          </button>
        </header>
        <div class="pf-dialog-body">
          <p class="pf-dialog-text">
            当前文件有未保存的修改，继续操作会丢弃这些修改。也可以先返回编辑器点「保存」。
          </p>
        </div>
        <footer class="pf-dialog-actions">
          <button type="button" class="pf-dialog-btn" @click="cancelLeave">取消</button>
          <button type="button" class="pf-dialog-btn pf-dialog-danger" @click="confirmDiscardAndLeave">放弃修改</button>
        </footer>
      </div>
    </div>

    <!-- 删除确认 -->
    <div v-if="deleteOpen" class="pf-mask" role="dialog" aria-modal="true" aria-label="确认删除文件">
      <div class="pf-dialog">
        <header class="pf-dialog-head">
          <strong>删除文件</strong>
          <button type="button" class="pf-icon-btn" aria-label="关闭" :disabled="deleteBusy" @click="closeDeleteConfirm">
            <X :size="18" />
          </button>
        </header>
        <div class="pf-dialog-body">
          <p class="pf-dialog-line">
            <AlertTriangle :size="15" class="pf-warn-icon" />
            <code class="pf-dialog-path">{{ deleteTarget }}</code>
          </p>
          <p class="pf-dialog-warn">
            将通过桌面 ProjectForge <code>RemoveFile</code> 删除：先存快照，再移到系统回收站（不会硬删），可在桌面端回退恢复。
          </p>
          <label class="pf-dialog-label" for="pf-delete-reason">删除原因（必填，写入施工记录）</label>
          <textarea
            id="pf-delete-reason"
            v-model="deleteReason"
            class="pf-dialog-input"
            rows="2"
            maxlength="500"
            placeholder="例如：旧实现已被新模块替代"
          ></textarea>
          <p v-if="deleteError" class="pf-dialog-error">{{ deleteError }}</p>
        </div>
        <footer class="pf-dialog-actions">
          <button type="button" class="pf-dialog-btn" :disabled="deleteBusy" @click="closeDeleteConfirm">取消</button>
          <button
            type="button"
            class="pf-dialog-btn pf-dialog-danger"
            :disabled="deleteBusy || !deleteReason.trim()"
            @click="confirmDelete"
          >
            <Loader2 v-if="deleteBusy" :size="15" class="pf-spin" />
            <Trash2 v-else :size="15" />
            确认删除
          </button>
        </footer>
      </div>
    </div>

    <!-- 移动 / 重命名确认 -->
    <div v-if="moveOpen" class="pf-mask" role="dialog" aria-modal="true" aria-label="确认移动文件">
      <div class="pf-dialog">
        <header class="pf-dialog-head">
          <strong>移动 / 重命名文件</strong>
          <button type="button" class="pf-icon-btn" aria-label="关闭" :disabled="moveBusy" @click="closeMoveConfirm">
            <X :size="18" />
          </button>
        </header>
        <div class="pf-dialog-body">
          <p class="pf-dialog-line">
            <FolderInput :size="15" />
            <code class="pf-dialog-path">{{ moveFrom }}</code>
          </p>
          <label class="pf-dialog-label" for="pf-move-target">目标路径（工程内相对路径）</label>
          <input
            id="pf-move-target"
            v-model="moveTo"
            class="pf-dialog-input"
            type="text"
            autocomplete="off"
            spellcheck="false"
          />
          <label class="pf-dialog-label" for="pf-move-reason">移动原因（必填，写入施工记录）</label>
          <textarea
            id="pf-move-reason"
            v-model="moveReason"
            class="pf-dialog-input"
            rows="2"
            maxlength="500"
            placeholder="例如：工具函数统一收进 utils 目录"
          ></textarea>
          <p class="pf-dialog-hint">目标已存在时桌面端会拒绝；移动后自动回读校验。</p>
          <p v-if="moveError" class="pf-dialog-error">{{ moveError }}</p>
        </div>
        <footer class="pf-dialog-actions">
          <button type="button" class="pf-dialog-btn" :disabled="moveBusy" @click="closeMoveConfirm">取消</button>
          <button
            type="button"
            class="pf-dialog-btn pf-dialog-primary"
            :disabled="moveBusy || !moveReason.trim() || !moveTo.trim()"
            @click="confirmMove"
          >
            <Loader2 v-if="moveBusy" :size="15" class="pf-spin" />
            <FolderInput v-else :size="15" />
            确认移动
          </button>
        </footer>
      </div>
    </div>
  </section>
</template>

<style scoped>
.pf-source {
  position: relative;
  display: flex;
  flex-direction: column;
  min-height: 0;
  height: 100%;
  color: var(--primary-text);
  background: var(--primary-bg, transparent);
}

/* —— 头部 —— */
.pf-source-head {
  display: flex;
  align-items: flex-end;
  gap: 6px;
  padding: 6px 8px 4px;
  flex: 0 0 auto;
}

.pf-select-box {
  display: flex;
  flex-direction: column;
  gap: 2px;
  flex: 1;
  min-width: 0;
}

.pf-select-label {
  font-size: 9px;
  opacity: 0.45;
  padding-left: 2px;
}

.pf-select {
  width: 100%;
  min-height: 40px;
  padding: 0 8px;
  border-radius: 9px;
  border: 1px solid var(--vcp-border-subtle, rgba(255, 255, 255, 0.12));
  background: var(--secondary-bg, rgba(255, 255, 255, 0.05));
  color: inherit;
  font-size: 12px;
  outline: none;
  text-overflow: ellipsis;
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
  -webkit-tap-highlight-color: transparent;
}

.pf-icon-btn:disabled {
  opacity: 0.35;
}

.pf-icon-btn:active {
  background: color-mix(in srgb, var(--highlight-text, #4f8cff) 14%, transparent);
}

/* —— 搜索行 —— */
.pf-source-subhead {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 2px 8px 4px;
  flex: 0 0 auto;
}

.pf-search-box {
  position: relative;
  flex: 1;
  min-width: 0;
  display: flex;
  align-items: center;
}

.pf-search-icon {
  position: absolute;
  left: 10px;
  opacity: 0.4;
  pointer-events: none;
}

.pf-search-input {
  width: 100%;
  min-height: 40px;
  padding: 0 32px 0 32px;
  border-radius: 10px;
  border: 1px solid var(--vcp-border-subtle, rgba(255, 255, 255, 0.12));
  background: var(--secondary-bg, rgba(255, 255, 255, 0.05));
  color: inherit;
  font-size: 12.5px;
  outline: none;
}

.pf-search-clear {
  position: absolute;
  right: 4px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 32px;
  height: 32px;
  border: 0;
  border-radius: 8px;
  background: transparent;
  color: inherit;
  opacity: 0.55;
}

.pf-new-btn {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  min-height: 40px;
  padding: 0 12px;
  border-radius: 10px;
  border: 1px solid var(--highlight-text, #4f8cff);
  background: color-mix(in srgb, var(--highlight-text, #4f8cff) 16%, transparent);
  color: var(--highlight-text, #4f8cff);
  font-size: 11.5px;
  font-weight: 700;
  cursor: pointer;
  flex: 0 0 auto;
}

/* —— 元信息 —— */
.pf-source-meta {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 0 10px 4px;
  font-size: 9.5px;
  opacity: 0.5;
  flex: 0 0 auto;
  min-width: 0;
}

.pf-meta-root {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  direction: rtl;
  text-align: left;
  font-family: ui-monospace, 'SFMono-Regular', Menlo, Consolas, monospace;
}

.pf-meta-transport,
.pf-meta-loading {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  flex: 0 0 auto;
}

/* —— 横幅 —— */
.pf-banner {
  display: flex;
  align-items: flex-start;
  gap: 6px;
  margin: 0 8px 4px;
  padding: 6px 10px;
  border-radius: 8px;
  font-size: 10.5px;
  line-height: 1.45;
  flex: 0 0 auto;
  word-break: break-word;
}

.pf-banner-info {
  background: color-mix(in srgb, var(--pf-info, #6aa7ff) 12%, transparent);
  color: var(--pf-info, #6aa7ff);
}

.pf-banner-warn {
  background: color-mix(in srgb, var(--pf-warn, #f5a623) 12%, transparent);
  color: var(--pf-warn, #f5a623);
}

.pf-banner-error {
  background: color-mix(in srgb, var(--pf-danger, #ff6b6b) 14%, transparent);
  color: var(--pf-danger, #ff6b6b);
}

.pf-banner-ok {
  background: color-mix(in srgb, var(--pf-ok, #3ecf8e) 12%, transparent);
  color: var(--pf-ok, #3ecf8e);
}

.pf-banner-text {
  flex: 1;
  min-width: 0;
}

.pf-banner-close {
  flex: 0 0 auto;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 28px;
  height: 28px;
  border: 0;
  border-radius: 6px;
  background: transparent;
  color: inherit;
  opacity: 0.7;
}

/* —— 树主体 —— */
.pf-source-body {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  border-top: 1px solid var(--vcp-border-subtle, rgba(255, 255, 255, 0.06));
}

.pf-source-status {
  flex: 0 0 auto;
  padding: 4px 12px calc(6px + var(--vcp-safe-bottom, 0px));
  font-size: 10px;
  opacity: 0.5;
}

/* —— 弹层 —— */
.pf-mask {
  position: absolute;
  inset: 0;
  z-index: 30;
  display: flex;
  align-items: flex-end;
  justify-content: center;
  padding: 12px;
  padding-bottom: calc(12px + var(--vcp-safe-bottom, 0px));
  background: rgba(0, 0, 0, 0.55);
}

.pf-dialog {
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

.pf-dialog-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 10px 8px 6px 14px;
  font-size: 13px;
}

.pf-dialog-body {
  padding: 0 14px 6px;
  overflow-y: auto;
}

.pf-dialog-line {
  display: flex;
  align-items: center;
  gap: 8px;
  margin: 0 0 8px;
  font-size: 11.5px;
}

.pf-warn-icon {
  color: var(--pf-danger, #ff6b6b);
  flex: 0 0 auto;
}

.pf-dialog-path {
  font-family: ui-monospace, 'SFMono-Regular', Menlo, Consolas, monospace;
  font-size: 11px;
  word-break: break-all;
}

.pf-dialog-warn {
  margin: 0 0 10px;
  padding: 8px 10px;
  border-radius: 8px;
  background: color-mix(in srgb, var(--pf-warn, #f5a623) 12%, transparent);
  color: var(--pf-warn, #f5a623);
  font-size: 10.5px;
  line-height: 1.5;
}

.pf-dialog-warn code,
.pf-dialog-text code {
  font-family: ui-monospace, 'SFMono-Regular', Menlo, Consolas, monospace;
}

.pf-dialog-label {
  display: block;
  font-size: 10.5px;
  opacity: 0.55;
  margin: 6px 0 4px;
}

.pf-dialog-input {
  width: 100%;
  min-height: 44px;
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

.pf-dialog-hint {
  margin: 8px 0 0;
  font-size: 9.5px;
  opacity: 0.45;
}

.pf-dialog-error {
  margin: 8px 0 0;
  font-size: 11px;
  color: var(--pf-danger, #ff6b6b);
}

.pf-dialog-text {
  margin: 0;
  font-size: 12px;
  line-height: 1.6;
  opacity: 0.8;
}

.pf-dialog-actions {
  display: flex;
  gap: 8px;
  padding: 10px 14px calc(12px + var(--vcp-safe-bottom, 0px));
}

.pf-dialog-btn {
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

.pf-dialog-btn:disabled {
  opacity: 0.4;
}

.pf-dialog-danger {
  flex: 1;
  border-color: color-mix(in srgb, var(--pf-danger, #ff6b6b) 45%, transparent);
  background: color-mix(in srgb, var(--pf-danger, #ff6b6b) 16%, transparent);
  color: var(--pf-danger, #ff6b6b);
}

.pf-dialog-primary {
  flex: 1;
  border-color: var(--highlight-text, #4f8cff);
  background: color-mix(in srgb, var(--highlight-text, #4f8cff) 22%, transparent);
  color: var(--highlight-text, #4f8cff);
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

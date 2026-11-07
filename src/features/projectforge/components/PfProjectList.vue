<script setup lang="ts">
/**
 * PfProjectList.vue — 「V工程」工程列表。
 *
 * 对齐桌面 VCPChat ProjectForge 侧栏「工程」页：搜索（名称 / ID / 署名）、
 * 「含已删除」开关、工程列表（状态徽标 / ID / 工作区 / Todo 进度条 / 变动次数 / 最近时间）、
 * 新建工程、删除（已验收才显示，二次确认）、恢复 / 永久清除（危险操作二次确认）。
 *
 * 数据全部来自 T1 的 useProjectForgeStore，组件不直接访问桥接。
 * 事件：select(projectId) —— 由页面 shell（Lead）负责切换到工程详情。
 */
import { computed, onBeforeUnmount, onMounted, ref } from 'vue';
import { AlertTriangle, Loader2, Plus, RotateCcw, Search, Trash2, X } from "@lucide/vue";
import { useNotificationStore } from '../../../core/stores/notification';
import { useProjectForgeStore } from '../projectForgeStore';
import type { PfProject } from '../projectForgeTypes';

const props = withDefaults(
  defineProps<{
    /** 当前已打开的工程 ID（可选，仅用于高亮）。 */
    activeProjectId?: string;
  }>(),
  { activeProjectId: '' },
);

const emit = defineEmits<{ select: [projectId: string] }>();

const store = useProjectForgeStore();
const notificationStore = useNotificationStore();

// —— 列表过滤 ——
const query = ref('');
const includeDeleted = ref<boolean>(Boolean((store as any).includeDeleted));

// —— 新建工程 ——
const creatingOpen = ref(false);
const creating = ref(false);
const newName = ref('');
const newWorkspace = ref('');
const newDir = ref('');
const newTodos = ref('');
const createError = ref('');

// —— 行内操作 / 确认弹层 ——
const busyId = ref('');
const confirmState = ref<{ kind: 'delete' | 'purge'; project: PfProject } | null>(null);

const STATUS_LABELS: Record<string, string> = {
  active: '施工中',
  review: '待验收',
  accepted: '已验收',
};

const loading = computed(() => Boolean(store.loading?.projects));
const listError = computed(() => {
  const specific = String((store as any).listError || '');
  return specific || String(store.error || '');
});

const workspaceOptions = computed(() =>
  (Array.isArray(store.workspaces) ? store.workspaces : []).filter((item: any) => item?.enabled !== false),
);

/** 本地即时过滤：与桌面一致，匹配名称 / 工程 ID / 施工署名（maid）。 */
const visibleProjects = computed<PfProject[]>(() => {
  const list = Array.isArray(store.projects) ? store.projects : [];
  const term = query.value.trim().toLowerCase();
  if (!term) return list;
  return list.filter((project) => {
    const name = String(project?.name || '').toLowerCase();
    const id = String(project?.id || '').toLowerCase();
    const maids = Array.isArray((project as any)?.maids) ? (project as any).maids : [];
    return (
      name.includes(term) ||
      id.includes(term) ||
      maids.some((maid: unknown) => String(maid || '').toLowerCase().includes(term))
    );
  });
});

const confirmTitle = computed(() => (confirmState.value?.kind === 'purge' ? '永久清除工程' : '删除工程'));
const confirmBody = computed(() =>
  confirmState.value?.kind === 'purge'
    ? '将删除该工程的记录、todo、验收报告与全部历史快照，之后无法回退。磁盘上的文件不受影响。'
    : '仅删除数据库记录，不改动任何磁盘文件；删除后可在「含已删除」里恢复。',
);
const confirmOkText = computed(() => (confirmState.value?.kind === 'purge' ? '永久清除' : '删除'));

// ---------------------------------------------------------------------------
// 数据加载
// ---------------------------------------------------------------------------

let searchTimer: ReturnType<typeof setTimeout> | null = null;

function errorText(error: unknown): string {
  if (error instanceof Error) return error.message;
  const message = (error as any)?.message;
  return message ? String(message) : String(error ?? '');
}

function notify(type: 'success' | 'error' | 'info', message: string): void {
  notificationStore.addNotification({
    id: `pf-list-${Date.now()}-${Math.random().toString(36).slice(2, 6)}`,
    title: 'V工程',
    message,
    type,
    duration: type === 'error' ? 4200 : 2400,
    toastOnly: true,
  });
}

async function reload(): Promise<void> {
  try {
    await store.loadProjects({ includeDeleted: includeDeleted.value });
  } catch (error) {
    // 错误文案由 store.error / listError 承载，模板里展示
    void error;
  }
}

async function searchRemote(): Promise<void> {
  const term = query.value.trim();
  try {
    if (term) {
      await store.searchProjects(term);
    } else {
      await reload();
    }
  } catch (error) {
    void error;
  }
}

function onQueryInput(): void {
  if (searchTimer) clearTimeout(searchTimer);
  searchTimer = setTimeout(() => {
    searchTimer = null;
    void searchRemote();
  }, 300);
}

function clearQuery(): void {
  query.value = '';
  void reload();
}

function onIncludeDeletedChange(): void {
  // 同步到 store（T1 契约里 includeDeleted 是可写 ref，部分查询会读取它）
  try {
    (store as any).includeDeleted = includeDeleted.value;
  } catch {
    /* 只读实现时忽略，下面显式传参兜底 */
  }
  if (query.value.trim()) void searchRemote();
  else void reload();
}

onMounted(() => {
  if (!store.projects.length && !loading.value) void reload();
});

onBeforeUnmount(() => {
  if (searchTimer) clearTimeout(searchTimer);
});

// ---------------------------------------------------------------------------
// 展示辅助
// ---------------------------------------------------------------------------

function formatTime(value?: string | null): string {
  if (!value) return '—';
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return String(value);
  const pad = (input: number) => String(input).padStart(2, '0');
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

function badgeLabel(project: PfProject): string {
  if (project.deleted_at) return '已删除';
  const status = String(project.status || 'active');
  return STATUS_LABELS[status] || status;
}

function badgeClass(project: PfProject): string[] {
  if (project.deleted_at) return ['is-deleted'];
  const status = String(project.status || 'active');
  return [`is-${STATUS_LABELS[status] ? status : 'unknown'}`];
}

function progressDone(project: PfProject): number {
  const progress = project.progress;
  if (progress && Number.isFinite(Number(progress.done))) return Number(progress.done);
  // T1 会从 ListProjects 的 Markdown 摘要里解析出 todoDone/todoTotal（结构化 details 不含进度）
  if (Number.isFinite(Number(project.todoDone))) return Number(project.todoDone);
  return 0;
}

function progressTotal(project: PfProject): number {
  const progress = project.progress;
  if (progress && Number.isFinite(Number(progress.total))) return Number(progress.total);
  if (Number.isFinite(Number(project.todoTotal))) return Number(project.todoTotal);
  return 0;
}

function progressLabel(project: PfProject): string {
  const text = project.progress?.text;
  if (text) return String(text);
  const total = progressTotal(project);
  return total ? `${progressDone(project)}/${total}` : '无';
}

function progressPercent(project: PfProject): number {
  const total = progressTotal(project);
  if (!total) return 0;
  return Math.max(0, Math.min(100, Math.round((progressDone(project) / total) * 100)));
}

function nodeCount(project: PfProject): number {
  const stats = project.stats as any;
  return Number(stats?.nodeCount || 0);
}

function lastChangedAt(project: PfProject): string {
  const stats = project.stats as any;
  return String(stats?.lastAt || project.updated_at || '');
}

/** 与桌面一致：只有已验收且未删除的工程才显示「删除」。 */
function canDelete(project: PfProject): boolean {
  return String(project.status || '') === 'accepted' && !project.deleted_at;
}

function selectProject(project: PfProject): void {
  if (!project?.id) return;
  emit('select', String(project.id));
}

// ---------------------------------------------------------------------------
// 新建 / 删除 / 恢复 / 清除
// ---------------------------------------------------------------------------

function toggleCreate(): void {
  creatingOpen.value = !creatingOpen.value;
  if (!creatingOpen.value) {
    createError.value = '';
    newName.value = '';
    newWorkspace.value = '';
    newDir.value = '';
    newTodos.value = '';
  }
}

async function submitCreate(): Promise<void> {
  createError.value = '';
  const name = newName.value.trim();
  if (!name) {
    createError.value = '请填写工程名称';
    return;
  }
  creating.value = true;
  try {
    const created = await store.createProject({
      name,
      workspace: newWorkspace.value || undefined,
      dir: newDir.value.trim() || undefined,
      todos: newTodos.value.trim() || undefined,
    });
    const projectId = String((created as any)?.id || '');
    notify('success', `工程已创建：${(created as any)?.name || name}`);
    toggleCreate();
    if (projectId) emit('select', projectId);
  } catch (error) {
    createError.value = errorText(error);
    notify('error', `创建工程失败：${createError.value}`);
  } finally {
    creating.value = false;
  }
}

function askDelete(project: PfProject): void {
  confirmState.value = { kind: 'delete', project };
}

function askPurge(project: PfProject): void {
  confirmState.value = { kind: 'purge', project };
}

async function syncList(): Promise<void> {
  try {
    await store.loadProjects({ includeDeleted: includeDeleted.value });
  } catch {
    /* store.error 已记录 */
  }
}

async function runConfirmed(): Promise<void> {
  const state = confirmState.value;
  if (!state) return;
  const project = state.project;
  busyId.value = project.id;
  try {
    if (state.kind === 'delete') {
      await store.deleteProjects([project.id]);
      notify('success', '工程已删除（仅数据库记录，可在「含已删除」中恢复）');
    } else {
      await store.purgeProjects([project.id]);
      notify('success', '工程记录已永久清除');
    }
    confirmState.value = null;
    await syncList();
  } catch (error) {
    notify('error', `${state.kind === 'purge' ? '永久清除' : '删除'}工程失败：${errorText(error)}`);
  } finally {
    busyId.value = '';
  }
}

async function restoreProject(project: PfProject): Promise<void> {
  if (busyId.value) return;
  busyId.value = project.id;
  try {
    await store.restoreProjects([project.id]);
    notify('success', `已恢复工程：${project.name}`);
    await syncList();
  } catch (error) {
    notify('error', `恢复工程失败：${errorText(error)}`);
  } finally {
    busyId.value = '';
  }
}
</script>

<template>
  <div class="pf-list">
    <!-- 搜索 / 新建 -->
    <div class="pf-toolbar">
      <label class="pf-search">
        <Search :size="16" class="pf-search-icon" aria-hidden="true" />
        <input
          v-model="query"
          type="search"
          class="pf-search-input"
          placeholder="搜索工程名称 / ID / 署名"
          autocomplete="off"
          aria-label="搜索工程名称 / ID / 署名"
          @input="onQueryInput"
        />
        <button v-if="query" type="button" class="pf-search-clear" aria-label="清空搜索" @click="clearQuery">
          <X :size="16" />
        </button>
      </label>
      <button type="button" class="pf-new-btn" @click="toggleCreate">
        <Plus :size="16" aria-hidden="true" />
        <span>新建</span>
      </button>
    </div>

    <label class="pf-include-deleted">
      <input v-model="includeDeleted" type="checkbox" @change="onIncludeDeletedChange" />
      <span>含已删除</span>
    </label>

    <!-- 新建工程表单 -->
    <form v-if="creatingOpen" class="pf-create" @submit.prevent="submitCreate">
      <div class="pf-section-title">新建工程</div>
      <label class="pf-field">
        <span class="pf-field-label">工程名称 <em>*</em></span>
        <input v-model="newName" type="text" maxlength="120" placeholder="例如：示例工程 A" />
      </label>
      <label class="pf-field">
        <span class="pf-field-label">工作区</span>
        <select v-model="newWorkspace">
          <option value="">（默认工作区）</option>
          <option v-for="workspace in workspaceOptions" :key="workspace.id || workspace.alias" :value="workspace.alias || workspace.id">
            {{ workspace.alias || workspace.path || workspace.id }}
          </option>
        </select>
      </label>
      <label class="pf-field">
        <span class="pf-field-label">子目录</span>
        <input v-model="newDir" type="text" placeholder="可选：工作区内子目录作为工程根" />
      </label>
      <label class="pf-field">
        <span class="pf-field-label">初始 Todo</span>
        <textarea v-model="newTodos" rows="2" placeholder="可选：换行或逗号分隔" />
      </label>
      <p v-if="createError" class="pf-form-error">{{ createError }}</p>
      <div class="pf-create-actions">
        <button type="button" class="pf-btn" @click="toggleCreate">取消</button>
        <button type="submit" class="pf-btn primary" :disabled="creating">
          {{ creating ? '创建中…' : '创建工程' }}
        </button>
      </div>
    </form>

    <!-- 加载态 -->
    <div v-if="loading && !visibleProjects.length" class="pf-state">
      <Loader2 class="pf-spin" :size="18" aria-hidden="true" />
      <span>正在加载工程…</span>
    </div>

    <!-- 错误态 -->
    <div v-else-if="listError && !visibleProjects.length" class="pf-state is-error">
      <AlertTriangle :size="18" aria-hidden="true" />
      <p class="pf-state-text">{{ listError }}</p>
      <button type="button" class="pf-btn" @click="reload">重试</button>
    </div>

    <!-- 空态 -->
    <div v-else-if="!visibleProjects.length" class="pf-state">
      <p class="pf-state-text">
        {{ query.trim() ? '没有匹配的工程。' : '没有工程。Agent 通过 ProjectForge CreateProject 创建后会出现在这里。' }}
      </p>
    </div>

    <!-- 工程列表 -->
    <ul v-else class="pf-projects" aria-label="工程列表">
      <li
        v-for="project in visibleProjects"
        :key="project.id"
        class="pf-project"
        :class="{ 'is-deleted': !!project.deleted_at, 'is-active': project.id === props.activeProjectId }"
      >
        <div
          class="pf-project-main"
          role="button"
          tabindex="0"
          @click="selectProject(project)"
          @keydown.enter.prevent="selectProject(project)"
          @keydown.space.prevent="selectProject(project)"
        >
          <div class="pf-project-row">
            <span class="pf-project-name" :title="project.name">{{ project.name }}</span>
            <span class="pf-badge" :class="badgeClass(project)">{{ badgeLabel(project) }}</span>
          </div>
          <div class="pf-project-sub">
            <code class="pf-mono">{{ project.id }}</code>
            <span class="pf-dot">·</span>
            <span>{{ project.workspace_alias || '未指定工作区' }}</span>
            <span class="pf-dot">·</span>
            <span>Todo {{ progressLabel(project) }}</span>
          </div>
          <div class="pf-project-meta">
            <span>{{ nodeCount(project) }} 次变动</span>
            <span class="pf-dot">·</span>
            <span>最近 {{ formatTime(lastChangedAt(project)) }}</span>
          </div>
          <div class="pf-progress" :title="`Todo ${progressLabel(project)}`">
            <span class="pf-progress-fill" :style="{ width: `${progressPercent(project)}%` }" />
          </div>
        </div>

        <div class="pf-project-actions">
          <button
            v-if="canDelete(project)"
            type="button"
            class="pf-action-btn is-danger"
            :aria-label="`删除工程 ${project.name}`"
            :disabled="busyId === project.id"
            @click.stop="askDelete(project)"
          >
            <Trash2 :size="15" aria-hidden="true" />
            <span>删除</span>
          </button>
          <template v-if="project.deleted_at">
            <button
              type="button"
              class="pf-action-btn"
              :disabled="busyId === project.id"
              :aria-label="`恢复工程 ${project.name}`"
              @click.stop="restoreProject(project)"
            >
              <RotateCcw :size="15" aria-hidden="true" />
              <span>恢复</span>
            </button>
            <button
              type="button"
              class="pf-action-btn is-danger"
              :disabled="busyId === project.id"
              :aria-label="`永久清除工程 ${project.name}`"
              @click.stop="askPurge(project)"
            >
              <X :size="15" aria-hidden="true" />
              <span>永久清除</span>
            </button>
          </template>
        </div>
      </li>
    </ul>

    <!-- 删除 / 永久清除 二次确认 -->
    <Teleport to="body">
      <div v-if="confirmState" class="pf-confirm-mask" @click.self="confirmState = null">
        <div class="pf-confirm" role="dialog" aria-modal="true" :aria-label="confirmTitle">
          <h3 class="pf-confirm-title">{{ confirmTitle }}</h3>
          <p class="pf-confirm-name">{{ confirmState.project.name }}</p>
          <p class="pf-confirm-body">{{ confirmBody }}</p>
          <p v-if="confirmState.kind === 'purge'" class="pf-confirm-warn">⚠ 此操作不可撤销</p>
          <div class="pf-confirm-actions">
            <button type="button" class="pf-btn" @click="confirmState = null">取消</button>
            <button
              type="button"
              class="pf-btn danger"
              :disabled="busyId === confirmState.project.id"
              @click="runConfirmed"
            >
              {{ busyId === confirmState.project.id ? '处理中…' : confirmOkText }}
            </button>
          </div>
        </div>
      </div>
    </Teleport>
  </div>
</template>

<style scoped>
.pf-list {
  --pf-ok: #4caf7d;
  --pf-warn: #e0a84a;
  --pf-danger: #e57373;
  display: flex;
  flex-direction: column;
  min-height: 0;
  height: 100%;
  color: var(--primary-text);
  font-size: 14px;
}

/* ---------- 工具栏 ---------- */
.pf-toolbar {
  display: flex;
  gap: 8px;
  padding: 10px 12px 6px;
  flex-shrink: 0;
}

.pf-search {
  position: relative;
  flex: 1;
  min-width: 0;
  display: flex;
  align-items: center;
  min-height: 44px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: var(--secondary-bg);
  padding: 0 8px 0 10px;
}

.pf-search:focus-within {
  border-color: var(--highlight-text, var(--primary-text));
}

.pf-search-icon {
  flex-shrink: 0;
  opacity: 0.55;
}

.pf-search-input {
  flex: 1;
  min-width: 0;
  border: none;
  outline: none;
  background: transparent;
  color: inherit;
  font: inherit;
  padding: 0 6px;
  height: 42px;
}

.pf-search-input::-webkit-search-cancel-button {
  display: none;
}

.pf-search-clear {
  flex-shrink: 0;
  width: 40px;
  height: 40px;
  display: grid;
  place-items: center;
  border: none;
  background: transparent;
  color: inherit;
  opacity: 0.6;
}

.pf-new-btn {
  flex-shrink: 0;
  min-height: 44px;
  min-width: 44px;
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 0 12px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: transparent;
  color: inherit;
  font: inherit;
}

.pf-new-btn:active {
  background: var(--secondary-bg);
}

.pf-include-deleted {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  margin: 0 12px 6px;
  min-height: 44px;
  font-size: 13px;
  color: var(--secondary-text, var(--primary-text));
  opacity: 0.85;
  flex-shrink: 0;
  width: fit-content;
}

.pf-include-deleted input {
  width: 18px;
  height: 18px;
  accent-color: var(--highlight-text, var(--primary-text));
}

/* ---------- 新建表单 ---------- */
.pf-create {
  margin: 0 12px 10px;
  padding: 10px 12px 12px;
  border: 1px solid var(--border-color);
  border-radius: 10px;
  background: var(--secondary-bg);
  flex-shrink: 0;
}

.pf-section-title {
  font-size: 13px;
  font-weight: 600;
  margin-bottom: 8px;
}

.pf-field {
  display: flex;
  flex-direction: column;
  gap: 4px;
  margin-bottom: 8px;
}

.pf-field-label {
  font-size: 12px;
  color: var(--secondary-text, var(--primary-text));
  opacity: 0.8;
}

.pf-field-label em {
  color: var(--pf-danger);
  font-style: normal;
}

.pf-field input,
.pf-field select,
.pf-field textarea {
  width: 100%;
  box-sizing: border-box;
  min-height: 44px;
  padding: 8px 10px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: var(--primary-bg);
  color: inherit;
  font: inherit;
  outline: none;
}

.pf-field textarea {
  min-height: 60px;
  resize: vertical;
}

.pf-field input:focus,
.pf-field select:focus,
.pf-field textarea:focus {
  border-color: var(--highlight-text, var(--primary-text));
}

.pf-form-error {
  margin: 0 0 8px;
  font-size: 12px;
  color: var(--pf-danger);
}

.pf-create-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
}

/* ---------- 状态 ---------- */
.pf-state {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 10px;
  padding: 24px 20px;
  color: var(--secondary-text, var(--primary-text));
  opacity: 0.8;
  text-align: center;
  font-size: 13px;
  line-height: 1.6;
}

.pf-state.is-error {
  color: var(--pf-danger);
  opacity: 1;
}

.pf-state-text {
  margin: 0;
  max-width: 300px;
}

.pf-spin {
  animation: pf-rotate 0.9s linear infinite;
}

@keyframes pf-rotate {
  to {
    transform: rotate(360deg);
  }
}

/* ---------- 列表 ---------- */
.pf-projects {
  list-style: none;
  margin: 0;
  padding: 0 0 calc(var(--vcp-safe-bottom, 0px) + 12px);
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  -webkit-overflow-scrolling: touch;
}

.pf-project {
  display: flex;
  align-items: flex-start;
  gap: 8px;
  padding: 10px 12px;
  border-bottom: 1px solid var(--border-color);
}

.pf-project.is-active {
  box-shadow: inset 2px 0 0 var(--highlight-text, var(--primary-text));
  background: var(--secondary-bg);
}

.pf-project.is-deleted {
  opacity: 0.6;
}

.pf-project-main {
  flex: 1;
  min-width: 0;
  cursor: pointer;
  outline: none;
}

.pf-project-main:focus-visible {
  outline: 2px solid var(--highlight-text, var(--primary-text));
  outline-offset: 2px;
  border-radius: 4px;
}

.pf-project-row {
  display: flex;
  align-items: center;
  gap: 8px;
}

.pf-project-name {
  flex: 1;
  min-width: 0;
  font-weight: 600;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.pf-badge {
  flex-shrink: 0;
  padding: 1px 8px;
  border: 1px solid var(--border-color);
  border-radius: 999px;
  font-size: 11px;
  color: var(--secondary-text, var(--primary-text));
  white-space: nowrap;
}

.pf-badge.is-active {
  color: var(--highlight-text, var(--primary-text));
  border-color: var(--highlight-text, var(--primary-text));
}

.pf-badge.is-review {
  color: var(--pf-warn);
  border-color: var(--pf-warn);
}

.pf-badge.is-accepted {
  color: var(--pf-ok);
  border-color: var(--pf-ok);
}

.pf-badge.is-deleted {
  color: var(--pf-danger);
  border-color: var(--pf-danger);
}

.pf-project-sub {
  display: flex;
  flex-wrap: wrap;
  align-items: baseline;
  gap: 0 6px;
  margin-top: 4px;
  font-size: 12px;
  color: var(--secondary-text, var(--primary-text));
  opacity: 0.85;
}

.pf-project-meta {
  display: flex;
  flex-wrap: wrap;
  gap: 0 6px;
  margin-top: 3px;
  font-size: 12px;
  color: var(--secondary-text, var(--primary-text));
  opacity: 0.7;
}

.pf-dot {
  opacity: 0.5;
}

.pf-mono {
  font-family: Consolas, 'Cascadia Code', ui-monospace, monospace;
  font-size: 12px;
}

.pf-progress {
  height: 4px;
  margin-top: 8px;
  border-radius: 2px;
  background: var(--border-color);
  overflow: hidden;
}

.pf-progress-fill {
  display: block;
  height: 100%;
  background: var(--pf-ok);
  transition: width 0.25s ease;
}

/* ---------- 行内操作 ---------- */
.pf-project-actions {
  display: flex;
  flex-direction: column;
  gap: 4px;
  flex-shrink: 0;
}

.pf-action-btn {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  min-height: 44px;
  padding: 0 10px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: transparent;
  color: inherit;
  font-size: 12px;
  white-space: nowrap;
}

.pf-action-btn:active {
  background: var(--secondary-bg);
}

.pf-action-btn.is-danger {
  color: var(--pf-danger);
  border-color: color-mix(in srgb, var(--pf-danger) 45%, transparent);
}

.pf-action-btn:disabled {
  opacity: 0.45;
}

/* ---------- 按钮 / 弹层 ---------- */
.pf-btn {
  min-height: 44px;
  padding: 0 16px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: transparent;
  color: inherit;
  font: inherit;
  font-size: 14px;
}

.pf-btn:active {
  background: var(--secondary-bg);
}

.pf-btn.primary {
  background: var(--highlight-text, var(--primary-text));
  border-color: transparent;
  color: var(--primary-bg);
  font-weight: 600;
}

.pf-btn.danger {
  color: var(--pf-danger);
  border-color: var(--pf-danger);
}

.pf-btn:disabled {
  opacity: 0.5;
}

.pf-confirm-mask {
  position: fixed;
  inset: 0;
  z-index: var(--layer-dialog, 60);
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 24px;
  background: rgba(0, 0, 0, 0.55);
}

.pf-confirm {
  width: 100%;
  max-width: 360px;
  padding: 16px;
  border: 1px solid var(--border-color);
  border-radius: 12px;
  background: var(--secondary-bg);
  color: var(--primary-text);
}

.pf-confirm-title {
  margin: 0 0 8px;
  font-size: 16px;
}

.pf-confirm-name {
  margin: 0 0 6px;
  font-weight: 600;
  word-break: break-all;
}

.pf-confirm-body {
  margin: 0 0 8px;
  font-size: 13px;
  line-height: 1.6;
  color: var(--secondary-text, var(--primary-text));
}

.pf-confirm-warn {
  margin: 0 0 8px;
  font-size: 13px;
  color: var(--pf-warn);
}

.pf-confirm-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  margin-top: 12px;
}

@media (prefers-reduced-motion: reduce) {
  .pf-spin {
    animation: none;
  }

  .pf-progress-fill {
    transition: none;
  }
}
</style>

<script setup lang="ts">
/**
 * PfOverviewTab.vue — 工程详情「概览」tab。
 *
 * 对齐桌面 VCPChat ProjectForge 概览：统计卡（Todo 完成 / 变动节点 / 涉及文件 / 累计增删行 /
 * 最近变动）、Todo 完整编辑（新增 / 完成 / 进行中 / 待处理 / 阻塞 / 删除 / 改标题备注）、
 * 施工角色 contributor 表 + 「查看其改动」（跳搜索历史 byMaid）。
 *
 * 数据来自 useProjectForgeStore（T1），写操作调用 store.updateTodos（内部写后回读）。
 */
import { computed, ref } from 'vue';
import {
  AlertTriangle,
  CheckCircle2,
  Circle,
  CircleDot,
  Loader2,
  Pencil,
  Plus,
  Trash2,
} from "@lucide/vue";
import { useNotificationStore } from '../../../core/stores/notification';
import { useProjectForgeStore } from '../projectForgeStore';
import type { PfContributor, PfProject, PfTodo } from '../projectForgeTypes';

const emit = defineEmits<{ 'open-history': [filter: { byMaid: string }] }>();

const store = useProjectForgeStore();
const notificationStore = useNotificationStore();

const TODO_LABELS: Record<string, string> = {
  pending: '待处理',
  doing: '进行中',
  done: '完成',
  blocked: '阻塞',
};

const detail = computed(() => store.detail);
const project = computed<PfProject | null>(() => detail.value?.project ?? null);
const todos = computed<PfTodo[]>(() => detail.value?.todos ?? []);
const contributors = computed<PfContributor[]>(() => detail.value?.contributors ?? []);
const stats = computed<any>(() => (detail.value as any)?.stats ?? {});
const loading = computed(() => Boolean(store.loading?.detail));

// —— Todo 交互状态 ——
const addText = ref('');
const adding = ref(false);
const todoBusySeq = ref<number | null>(null);
const editingSeq = ref<number | null>(null);
const editTitle = ref('');
const editNote = ref('');
const todoError = ref('');

const todoProgressText = computed(() => {
  const progress = project.value?.progress;
  if (progress?.text) return String(progress.text);
  const total = todos.value.length;
  const done = todos.value.filter((todo) => todo.status === 'done').length;
  return total ? `${done}/${total}` : '无';
});

const todoPercent = computed(() => {
  const total = todos.value.length;
  if (!total) return 0;
  const done = todos.value.filter((todo) => todo.status === 'done').length;
  return Math.round((done / total) * 100);
});

function errorText(error: unknown): string {
  if (error instanceof Error) return error.message;
  const message = (error as any)?.message;
  return message ? String(message) : String(error ?? '');
}

function notify(type: 'success' | 'error' | 'info', message: string): void {
  notificationStore.addNotification({
    id: `pf-overview-${Date.now()}-${Math.random().toString(36).slice(2, 6)}`,
    title: 'V工程',
    message,
    type,
    duration: type === 'error' ? 4200 : 2400,
    toastOnly: true,
  });
}

function formatTime(value?: string | null): string {
  if (!value) return '—';
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return String(value);
  const pad = (input: number) => String(input).padStart(2, '0');
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

function statusLabel(status: string): string {
  return TODO_LABELS[status] || status || '待处理';
}

function statusIcon(status: string): any {
  if (status === 'done') return CheckCircle2;
  if (status === 'doing') return CircleDot;
  if (status === 'blocked') return AlertTriangle;
  return Circle;
}

function maidLabel(maid?: string | null, kind?: string): string {
  if (maid) return `@${maid}`;
  // 与桌面 maidHtml(c.maid, 'external') 一致：施工角色聚合里 null 表示外部修改
  return kind === 'external' ? '@外部' : '@未署名';
}

// ---------------------------------------------------------------------------
// Todo 操作（UpdateTodos）：完成后由 store 写后回读，界面自动刷新
// ---------------------------------------------------------------------------

async function runTodoPatch(patch: Record<string, unknown>, seq: number | null, successMessage = ''): Promise<void> {
  todoError.value = '';
  todoBusySeq.value = seq;
  try {
    await store.updateTodos(patch);
    if (successMessage) notify('success', successMessage);
  } catch (error) {
    todoError.value = errorText(error);
    notify('error', `待办更新失败：${todoError.value}`);
  } finally {
    todoBusySeq.value = null;
  }
}

function setTodoStatus(todo: PfTodo, status: 'done' | 'doing' | 'pending' | 'blocked'): void {
  const seq = Number(todo.seq);
  const patch: Record<string, unknown> = {};
  patch[status] = String(seq);
  void runTodoPatch(patch, seq, `待办 #${seq} 已标记为「${statusLabel(status)}」`);
}

function toggleDone(todo: PfTodo): void {
  setTodoStatus(todo, todo.status === 'done' ? 'pending' : 'done');
}

function removeTodo(todo: PfTodo): void {
  void runTodoPatch({ remove: String(todo.seq) }, Number(todo.seq), `已删除待办 #${todo.seq}`);
}

function startEdit(todo: PfTodo): void {
  editingSeq.value = Number(todo.seq);
  editTitle.value = String(todo.title || '');
  editNote.value = String(todo.note || '');
  todoError.value = '';
}

function cancelEdit(): void {
  editingSeq.value = null;
  editTitle.value = '';
  editNote.value = '';
}

async function saveEdit(): Promise<void> {
  const seq = editingSeq.value;
  if (seq == null) return;
  if (!editTitle.value.trim()) {
    todoError.value = '待办标题不能为空';
    return;
  }
  await runTodoPatch(
    { seq: String(seq), title: editTitle.value.trim(), note: editNote.value.trim() },
    seq,
    `待办 #${seq} 已更新`,
  );
  if (!todoError.value) cancelEdit();
}

async function addTodo(): Promise<void> {
  const text = addText.value.trim();
  if (!text) {
    todoError.value = '请输入待办内容';
    return;
  }
  adding.value = true;
  todoError.value = '';
  try {
    await store.updateTodos({ add: text });
    addText.value = '';
    notify('success', '已新增待办');
  } catch (error) {
    todoError.value = errorText(error);
    notify('error', `新增待办失败：${todoError.value}`);
  } finally {
    adding.value = false;
  }
}

function viewContributorChanges(contributor: PfContributor): void {
  if (!contributor?.maid) return;
  emit('open-history', { byMaid: String(contributor.maid) });
}
</script>

<template>
  <div class="pf-overview">
    <!-- 加载 / 空态 -->
    <div v-if="!detail" class="pf-state">
      <Loader2 v-if="loading" class="pf-spin" :size="18" aria-hidden="true" />
      <span>{{ loading ? '正在加载工程数据…' : '暂无工程数据' }}</span>
    </div>

    <template v-else>
      <!-- 进度统计卡 -->
      <div class="pf-section-title">进度</div>
      <div class="pf-stat-grid">
        <div class="pf-stat">
          <div class="pf-stat-value">{{ todoProgressText }}</div>
          <div class="pf-stat-key">Todo 完成</div>
          <div class="pf-progress"><span class="pf-progress-fill" :style="{ width: `${todoPercent}%` }" /></div>
        </div>
        <div class="pf-stat">
          <div class="pf-stat-value">{{ Number(stats.nodeCount || 0) }}</div>
          <div class="pf-stat-key">变动节点</div>
        </div>
        <div class="pf-stat">
          <div class="pf-stat-value">{{ Number(stats.fileCount || 0) }}</div>
          <div class="pf-stat-key">涉及文件</div>
        </div>
        <div class="pf-stat">
          <div class="pf-stat-value">
            <span class="pf-plus">+{{ Number(stats.added || 0) }}</span>
            <span class="pf-minus">-{{ Number(stats.removed || 0) }}</span>
          </div>
          <div class="pf-stat-key">累计增删行</div>
        </div>
        <div class="pf-stat">
          <div class="pf-stat-value is-time">{{ formatTime(stats.lastAt) }}</div>
          <div class="pf-stat-key">最近变动</div>
        </div>
      </div>

      <!-- Todo 完整编辑 -->
      <div class="pf-section-title">Todo</div>
      <p v-if="todoError" class="pf-inline-error">{{ todoError }}</p>
      <div class="pf-todo-add">
        <input
          v-model="addText"
          type="text"
          class="pf-todo-add-input"
          maxlength="200"
          placeholder="新增待办（回车提交）"
          aria-label="新增待办"
          @keydown.enter.prevent="addTodo"
        />
        <button type="button" class="pf-btn primary" :disabled="adding" @click="addTodo">
          <Plus :size="16" aria-hidden="true" />
          <span>{{ adding ? '添加中…' : '添加' }}</span>
        </button>
      </div>

      <p v-if="!todos.length" class="pf-empty-line">暂无 todo</p>
      <ul v-else class="pf-todos">
        <li v-for="todo in todos" :key="todo.seq" class="pf-todo" :class="`is-${todo.status}`">
          <button
            type="button"
            class="pf-todo-check"
            :aria-label="todo.status === 'done' ? '标记为待处理' : '标记为完成'"
            :disabled="todoBusySeq === Number(todo.seq)"
            @click="toggleDone(todo)"
          >
            <component :is="statusIcon(todo.status)" :size="20" aria-hidden="true" />
          </button>

          <div class="pf-todo-body">
            <template v-if="editingSeq === Number(todo.seq)">
              <input v-model="editTitle" class="pf-todo-edit-input" type="text" maxlength="200" aria-label="待办标题" />
              <textarea v-model="editNote" class="pf-todo-edit-note" rows="2" maxlength="500" placeholder="备注（可选）" aria-label="待办备注" />
              <div class="pf-todo-actions">
                <button type="button" class="pf-btn small primary" @click="saveEdit">保存</button>
                <button type="button" class="pf-btn small" @click="cancelEdit">取消</button>
              </div>
            </template>
            <template v-else>
              <div class="pf-todo-title">
                <span class="pf-todo-seq">#{{ todo.seq }}</span>
                <span class="pf-todo-name">{{ todo.title }}</span>
              </div>
              <div v-if="todo.note" class="pf-todo-note">{{ todo.note }}</div>
              <div class="pf-todo-meta">
                <span class="pf-todo-status">{{ statusLabel(todo.status) }}</span>
                <span v-if="todo.updated_by" class="pf-maid">@{{ todo.updated_by }}</span>
                <span class="pf-muted">{{ formatTime(todo.updated_at) }}</span>
                <button type="button" class="pf-link-btn" @click="startEdit(todo)">
                  <Pencil :size="13" aria-hidden="true" />
                  <span>编辑</span>
                </button>
              </div>
              <div class="pf-todo-actions">
                <button
                  v-for="option in [
                    { key: 'done', label: '完成' },
                    { key: 'doing', label: '进行中' },
                    { key: 'pending', label: '待处理' },
                    { key: 'blocked', label: '阻塞' },
                  ]"
                  :key="option.key"
                  type="button"
                  class="pf-chip-btn"
                  :class="{ 'is-current': todo.status === option.key }"
                  :disabled="todoBusySeq === Number(todo.seq) || todo.status === option.key"
                  @click="setTodoStatus(todo, option.key as 'done' | 'doing' | 'pending' | 'blocked')"
                >
                  {{ option.label }}
                </button>
                <button
                  type="button"
                  class="pf-chip-btn is-danger"
                  :disabled="todoBusySeq === Number(todo.seq)"
                  @click="removeTodo(todo)"
                >
                  <Trash2 :size="13" aria-hidden="true" />
                  <span>删除</span>
                </button>
                <Loader2
                  v-if="todoBusySeq === Number(todo.seq)"
                  class="pf-spin pf-todo-busy"
                  :size="14"
                  aria-hidden="true"
                />
              </div>
            </template>
          </div>
        </li>
      </ul>

      <!-- 施工角色 -->
      <div class="pf-section-title">施工角色</div>
      <p v-if="!contributors.length" class="pf-empty-line">暂无施工角色</p>
      <ul v-else class="pf-contributors">
        <li v-for="(contributor, index) in contributors" :key="contributor.maid || `none-${index}`" class="pf-contributor">
          <div class="pf-contributor-main">
            <span class="pf-maid" :class="{ 'is-none': !contributor.maid }">
              {{ maidLabel(contributor.maid, contributor.maid ? undefined : 'external') }}
            </span>
            <span class="pf-muted">批次 {{ contributor.batches }}</span>
            <span><span class="pf-plus">+{{ contributor.added }}</span> / <span class="pf-minus">-{{ contributor.removed }}</span></span>
            <span class="pf-muted">最近 {{ formatTime(contributor.last_at) }}</span>
          </div>
          <button
            v-if="contributor.maid"
            type="button"
            class="pf-link-btn"
            @click="viewContributorChanges(contributor)"
          >
            查看其改动
          </button>
        </li>
      </ul>
    </template>
  </div>
</template>

<style scoped>
.pf-overview {
  --pf-ok: #4caf7d;
  --pf-warn: #e0a84a;
  --pf-danger: #e57373;
  padding: 0 0 calc(var(--vcp-safe-bottom, 0px) + 16px);
  color: var(--primary-text);
  font-size: 14px;
}

.pf-section-title {
  margin: 16px 0 8px;
  font-size: 13px;
  font-weight: 600;
  color: var(--secondary-text, var(--primary-text));
  letter-spacing: 0.5px;
}

.pf-section-title:first-child {
  margin-top: 0;
}

.pf-state {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 8px;
  padding: 32px 16px;
  color: var(--secondary-text, var(--primary-text));
  opacity: 0.8;
  font-size: 13px;
}

.pf-spin {
  animation: pf-rotate 0.9s linear infinite;
}

@keyframes pf-rotate {
  to {
    transform: rotate(360deg);
  }
}

/* ---------- 统计卡 ---------- */
.pf-stat-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(104px, 1fr));
  gap: 8px;
}

.pf-stat {
  padding: 10px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  min-width: 0;
}

.pf-stat-value {
  font-size: 18px;
  font-weight: 600;
  display: flex;
  gap: 8px;
  align-items: baseline;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.pf-stat-value.is-time {
  font-size: 13px;
  white-space: normal;
  word-break: break-all;
}

.pf-stat-key {
  margin-top: 2px;
  font-size: 12px;
  color: var(--secondary-text, var(--primary-text));
  opacity: 0.8;
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
}

.pf-plus {
  color: var(--pf-ok);
}

.pf-minus {
  color: var(--pf-danger);
}

.pf-muted {
  color: var(--secondary-text, var(--primary-text));
  opacity: 0.75;
}

.pf-maid {
  color: var(--highlight-text, var(--primary-text));
  font-weight: 500;
}

.pf-maid.is-none {
  color: var(--secondary-text, var(--primary-text));
  font-style: italic;
  font-weight: 400;
}

/* ---------- Todo ---------- */
.pf-inline-error {
  margin: 0 0 8px;
  padding: 6px 10px;
  border: 1px solid var(--pf-danger);
  border-radius: 6px;
  color: var(--pf-danger);
  font-size: 12px;
}

.pf-todo-add {
  display: flex;
  gap: 8px;
  margin-bottom: 10px;
}

.pf-todo-add-input {
  flex: 1;
  min-width: 0;
  min-height: 44px;
  padding: 0 10px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: var(--secondary-bg);
  color: inherit;
  font: inherit;
  outline: none;
}

.pf-todo-add-input:focus {
  border-color: var(--highlight-text, var(--primary-text));
}

.pf-btn {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  min-height: 44px;
  padding: 0 14px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: transparent;
  color: inherit;
  font: inherit;
  font-size: 14px;
}

.pf-btn.small {
  min-height: 40px;
  padding: 0 12px;
  font-size: 13px;
}

.pf-btn.primary {
  background: var(--highlight-text, var(--primary-text));
  border-color: transparent;
  color: var(--primary-bg);
  font-weight: 600;
}

.pf-btn:disabled {
  opacity: 0.5;
}

.pf-empty-line {
  margin: 0 0 8px;
  font-size: 13px;
  color: var(--secondary-text, var(--primary-text));
  opacity: 0.7;
}

.pf-todos {
  list-style: none;
  margin: 0;
  padding: 0;
}

.pf-todo {
  display: flex;
  align-items: flex-start;
  gap: 8px;
  padding: 10px 0;
  border-bottom: 1px dashed var(--border-color);
}

.pf-todo-check {
  flex-shrink: 0;
  width: 44px;
  height: 44px;
  display: grid;
  place-items: center;
  border: none;
  background: transparent;
  color: var(--secondary-text, var(--primary-text));
  opacity: 0.8;
}

.pf-todo.is-done .pf-todo-check {
  color: var(--pf-ok);
  opacity: 1;
}

.pf-todo.is-doing .pf-todo-check {
  color: var(--highlight-text, var(--primary-text));
  opacity: 1;
}

.pf-todo.is-blocked .pf-todo-check {
  color: var(--pf-danger);
  opacity: 1;
}

.pf-todo-body {
  flex: 1;
  min-width: 0;
  padding-top: 2px;
}

.pf-todo-title {
  display: flex;
  gap: 6px;
  align-items: baseline;
}

.pf-todo-seq {
  flex-shrink: 0;
  font-family: Consolas, 'Cascadia Code', ui-monospace, monospace;
  font-size: 12px;
  color: var(--secondary-text, var(--primary-text));
  opacity: 0.7;
}

.pf-todo-name {
  word-break: break-word;
}

.pf-todo.is-done .pf-todo-name {
  text-decoration: line-through;
  opacity: 0.65;
}

.pf-todo-note {
  margin-top: 2px;
  font-size: 12px;
  color: var(--secondary-text, var(--primary-text));
  line-height: 1.5;
  word-break: break-word;
}

.pf-todo-meta {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 4px 10px;
  margin-top: 4px;
  font-size: 12px;
  color: var(--secondary-text, var(--primary-text));
  opacity: 0.85;
}

.pf-todo-status {
  color: var(--secondary-text, var(--primary-text));
}

.pf-todo.is-done .pf-todo-status {
  color: var(--pf-ok);
}

.pf-todo.is-doing .pf-todo-status {
  color: var(--highlight-text, var(--primary-text));
}

.pf-todo.is-blocked .pf-todo-status {
  color: var(--pf-danger);
}

.pf-todo-actions {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 6px;
  margin-top: 6px;
}

.pf-todo-busy {
  margin-left: 2px;
}

.pf-chip-btn {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  min-height: 40px;
  padding: 0 10px;
  border: 1px solid var(--border-color);
  border-radius: 999px;
  background: transparent;
  color: inherit;
  font: inherit;
  font-size: 12px;
}

.pf-chip-btn.is-current {
  border-color: var(--highlight-text, var(--primary-text));
  color: var(--highlight-text, var(--primary-text));
}

.pf-chip-btn.is-danger {
  color: var(--pf-danger);
  border-color: color-mix(in srgb, var(--pf-danger) 45%, transparent);
}

.pf-chip-btn:disabled {
  opacity: 0.55;
}

.pf-todo-edit-input,
.pf-todo-edit-note {
  width: 100%;
  box-sizing: border-box;
  min-height: 44px;
  padding: 8px 10px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: var(--secondary-bg);
  color: inherit;
  font: inherit;
  outline: none;
}

.pf-todo-edit-note {
  min-height: 56px;
  margin-top: 6px;
  resize: vertical;
}

.pf-link-btn {
  display: inline-flex;
  align-items: center;
  gap: 3px;
  min-height: 40px;
  padding: 0 6px;
  border: none;
  background: transparent;
  color: var(--highlight-text, var(--primary-text));
  font: inherit;
  font-size: 12px;
  white-space: nowrap;
}

/* ---------- 施工角色 ---------- */
.pf-contributors {
  list-style: none;
  margin: 0;
  padding: 0;
}

.pf-contributor {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  padding: 8px 0;
  border-bottom: 1px dashed var(--border-color);
}

.pf-contributor-main {
  display: flex;
  flex-wrap: wrap;
  align-items: baseline;
  gap: 2px 10px;
  font-size: 12px;
  min-width: 0;
}

@media (prefers-reduced-motion: reduce) {
  .pf-spin {
    animation: none;
  }
}
</style>

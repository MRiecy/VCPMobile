<script setup lang="ts">
/**
 * PfHistoryTab.vue — 工程详情「搜索历史」tab。
 *
 * 对齐桌面 VCPChat ProjectForge：过滤条件 keyword / content / file(通配) / byMaid /
 * op(create/edit/delete/move/rollback/external) / limit；结果表（节点、批次、op、文件、
 * 操作者、原因、+/-、时间）→ 点节点打开 PfDiffSheet。
 *
 * 支持外部跳转传入初始过滤（概览「查看其改动」传 byMaid、改动文件点行传 file），
 * 通过 props.initialFilter 传入并自动执行搜索。
 */
import { computed, onMounted, ref, watch } from 'vue';
import { AlertTriangle, Loader2, RotateCcw, Search } from "@lucide/vue";
import { useProjectForgeStore } from '../projectForgeStore';
import type { PfHistoryRow } from '../projectForgeTypes';

interface HistorySeed {
  keyword?: string;
  content?: string;
  file?: string;
  byMaid?: string;
  op?: string;
  limit?: number;
  /** 外部重复跳转时用于触发重新应用（时间戳）。 */
  _stamp?: number;
}

type DiffTarget = { kind: 'node'; node: PfHistoryRow } | { kind: 'batch'; batch: any };

const props = withDefaults(defineProps<{ initialFilter?: HistorySeed | null }>(), { initialFilter: null });

const emit = defineEmits<{ 'open-diff': [target: DiffTarget] }>();

const store = useProjectForgeStore();

const OP_OPTIONS = [
  { value: '', label: '全部操作' },
  { value: 'create', label: 'create（新建）' },
  { value: 'edit', label: 'edit（编辑）' },
  { value: 'delete', label: 'delete（删除）' },
  { value: 'move', label: 'move（移动）' },
  { value: 'rollback', label: 'rollback（回退）' },
  { value: 'external', label: 'external（外部修改）' },
];

const LIMIT_OPTIONS = [30, 100, 300, 500];

const keyword = ref('');
const content = ref('');
const file = ref('');
const byMaid = ref('');
const op = ref('');
const limit = ref(100);

const rows = ref<PfHistoryRow[]>([]);
const searched = ref(false);
const loading = ref(false);
const error = ref('');

const projectId = computed(() => String((store as any).currentProjectId || (store.detail as any)?.project?.id || ''));

function errorText(value: unknown): string {
  if (value instanceof Error) return value.message;
  const message = (value as any)?.message;
  return message ? String(message) : String(value ?? '');
}

function formatTime(value?: string | null): string {
  if (!value) return '—';
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return String(value);
  const pad = (input: number) => String(input).padStart(2, '0');
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

function maidLabel(row: PfHistoryRow): string {
  if (row.maid) return `@${row.maid}`;
  return (row as any).batch_kind === 'external' ? '@外部' : '@未署名';
}

function reasonLabel(row: PfHistoryRow): string {
  return String(row.effective_reason || (row as any).reason || row.summary || '（未记录原因）');
}

function applySeed(seed: HistorySeed | null | undefined): void {
  if (!seed) return;
  keyword.value = seed.keyword ?? '';
  content.value = seed.content ?? '';
  file.value = seed.file ?? '';
  byMaid.value = seed.byMaid ?? '';
  op.value = seed.op ?? '';
  limit.value = Number(seed.limit || 100) || 100;
}

function resetFilters(): void {
  keyword.value = '';
  content.value = '';
  file.value = '';
  byMaid.value = '';
  op.value = '';
  limit.value = 100;
}

async function runSearch(): Promise<void> {
  const filters: Record<string, unknown> = { limit: limit.value };
  if (keyword.value.trim()) filters.keyword = keyword.value.trim();
  if (content.value.trim()) filters.content = content.value.trim();
  if (file.value.trim()) filters.file = file.value.trim();
  if (byMaid.value.trim()) filters.byMaid = byMaid.value.trim();
  if (op.value) filters.op = op.value;

  loading.value = true;
  error.value = '';
  try {
    const result = await store.searchHistory(filters as any, { projectId: projectId.value });
    rows.value = Array.isArray(result) ? result : [];
    searched.value = true;
  } catch (cause) {
    rows.value = [];
    searched.value = true;
    error.value = errorText(cause);
  } finally {
    loading.value = false;
  }
}

function openNode(row: PfHistoryRow): void {
  emit('open-diff', { kind: 'node', node: row });
}

watch(
  () => props.initialFilter,
  (seed) => {
    if (!seed) return;
    applySeed(seed);
    void runSearch();
  },
  { deep: true },
);

onMounted(() => {
  if (props.initialFilter) {
    applySeed(props.initialFilter);
    void runSearch();
  }
});
</script>

<template>
  <div class="pf-history">
    <!-- 过滤表单 -->
    <form class="pf-filter" @submit.prevent="runSearch">
      <label class="pf-field">
        <span class="pf-field-label">关键字</span>
        <input v-model="keyword" type="search" placeholder="reason / 摘要 / 路径" autocomplete="off" />
      </label>
      <label class="pf-field">
        <span class="pf-field-label">内容包含</span>
        <input v-model="content" type="search" placeholder="改动后的文件内容" autocomplete="off" />
      </label>
      <label class="pf-field">
        <span class="pf-field-label">文件路径</span>
        <input v-model="file" type="search" placeholder="可用通配，如 src/*.js" autocomplete="off" />
      </label>
      <label class="pf-field">
        <span class="pf-field-label">操作者</span>
        <input v-model="byMaid" type="search" placeholder="按署名过滤，如 小明" autocomplete="off" />
      </label>
      <label class="pf-field">
        <span class="pf-field-label">操作类型</span>
        <select v-model="op">
          <option v-for="option in OP_OPTIONS" :key="option.value" :value="option.value">{{ option.label }}</option>
        </select>
      </label>
      <label class="pf-field">
        <span class="pf-field-label">返回条数</span>
        <select v-model.number="limit">
          <option v-for="option in LIMIT_OPTIONS" :key="option" :value="option">{{ option }} 条</option>
        </select>
      </label>
      <div class="pf-filter-actions">
        <button type="button" class="pf-btn" @click="resetFilters">
          <RotateCcw :size="15" aria-hidden="true" />
          <span>清空条件</span>
        </button>
        <button type="submit" class="pf-btn primary" :disabled="loading">
          <Search :size="15" aria-hidden="true" />
          <span>{{ loading ? '搜索中…' : '搜索' }}</span>
        </button>
      </div>
    </form>

    <!-- 结果区 -->
    <div v-if="loading" class="pf-state">
      <Loader2 class="pf-spin" :size="16" aria-hidden="true" />
      <span>搜索中…</span>
    </div>

    <div v-else-if="error" class="pf-state is-error">
      <AlertTriangle :size="16" aria-hidden="true" />
      <p class="pf-state-text">{{ error }}</p>
      <button type="button" class="pf-btn" @click="runSearch">重试</button>
    </div>

    <p v-else-if="!searched" class="pf-state">输入条件后搜索本工程的变动历史。</p>

    <p v-else-if="!rows.length" class="pf-state">没有匹配的记录。</p>

    <template v-else>
      <div class="pf-result-count">共 {{ rows.length }} 条</div>
      <ul class="pf-history-list" aria-label="历史搜索结果">
        <li
          v-for="row in rows"
          :key="row.id"
          class="pf-history-row"
          role="button"
          tabindex="0"
          :aria-label="`查看节点 n${row.id} 的差异`"
          @click="openNode(row)"
          @keydown.enter.prevent="openNode(row)"
          @keydown.space.prevent="openNode(row)"
        >
          <div class="pf-history-head">
            <code class="pf-mono">n{{ row.id }}</code>
            <code class="pf-mono pf-batch">b{{ row.batch_id ?? '-' }}</code>
            <span class="pf-badge">{{ row.op }}</span>
            <span class="pf-history-time">{{ formatTime(row.created_at) }}</span>
          </div>
          <div class="pf-history-file" :title="row.file_path">{{ row.file_path }}</div>
          <div class="pf-history-meta">
            <span class="pf-maid" :class="{ 'is-none': !row.maid }">{{ maidLabel(row) }}</span>
            <span class="pf-diff-count">
              <span class="pf-plus">+{{ row.added }}</span>/<span class="pf-minus">-{{ row.removed }}</span>
            </span>
            <span class="pf-history-reason">{{ reasonLabel(row) }}</span>
          </div>
        </li>
      </ul>
    </template>
  </div>
</template>

<style scoped>
.pf-history {
  --pf-ok: #4caf7d;
  --pf-danger: #e57373;
  padding: 0 0 calc(var(--vcp-safe-bottom, 0px) + 16px);
  color: var(--primary-text);
  font-size: 14px;
}

/* ---------- 过滤表单 ---------- */
.pf-filter {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 8px;
  padding-bottom: 10px;
  border-bottom: 1px solid var(--border-color);
}

.pf-field {
  display: flex;
  flex-direction: column;
  gap: 4px;
  min-width: 0;
}

.pf-field-label {
  font-size: 12px;
  color: var(--secondary-text, var(--primary-text));
  opacity: 0.8;
}

.pf-field input,
.pf-field select {
  width: 100%;
  box-sizing: border-box;
  min-height: 44px;
  padding: 0 10px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: var(--secondary-bg);
  color: inherit;
  font: inherit;
  outline: none;
}

.pf-field input:focus,
.pf-field select:focus {
  border-color: var(--highlight-text, var(--primary-text));
}

.pf-filter-actions {
  grid-column: 1 / -1;
  display: flex;
  justify-content: flex-end;
  gap: 8px;
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

.pf-btn.primary {
  background: var(--highlight-text, var(--primary-text));
  border-color: transparent;
  color: var(--primary-bg);
  font-weight: 600;
}

.pf-btn:disabled {
  opacity: 0.5;
}

/* ---------- 结果 ---------- */
.pf-state {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 8px;
  padding: 28px 16px;
  text-align: center;
  font-size: 13px;
  color: var(--secondary-text, var(--primary-text));
  opacity: 0.8;
}

.pf-state.is-error {
  color: var(--pf-danger);
  opacity: 1;
}

.pf-state-text {
  margin: 0;
  max-width: 320px;
  word-break: break-word;
}

.pf-spin {
  animation: pf-rotate 0.9s linear infinite;
}

@keyframes pf-rotate {
  to {
    transform: rotate(360deg);
  }
}

.pf-result-count {
  padding: 8px 2px;
  font-size: 12px;
  color: var(--secondary-text, var(--primary-text));
  opacity: 0.8;
}

.pf-history-list {
  list-style: none;
  margin: 0;
  padding: 0;
}

.pf-history-row {
  padding: 9px 4px;
  border-bottom: 1px solid var(--border-color);
  cursor: pointer;
  outline: none;
  min-height: 44px;
}

.pf-history-row:active {
  background: var(--secondary-bg);
}

.pf-history-row:focus-visible {
  outline: 2px solid var(--highlight-text, var(--primary-text));
  outline-offset: -2px;
}

.pf-history-head {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 4px 8px;
}

.pf-mono {
  font-family: Consolas, 'Cascadia Code', ui-monospace, monospace;
  font-size: 12px;
}

.pf-batch {
  color: var(--secondary-text, var(--primary-text));
  opacity: 0.7;
}

.pf-badge {
  padding: 0 7px;
  border: 1px solid var(--border-color);
  border-radius: 999px;
  font-size: 11px;
  color: var(--secondary-text, var(--primary-text));
  text-transform: lowercase;
  line-height: 17px;
}

.pf-history-time {
  margin-left: auto;
  font-size: 12px;
  color: var(--secondary-text, var(--primary-text));
  opacity: 0.75;
}

.pf-history-file {
  margin-top: 3px;
  font-family: Consolas, 'Cascadia Code', ui-monospace, monospace;
  font-size: 12px;
  word-break: break-all;
}

.pf-history-meta {
  display: flex;
  flex-wrap: wrap;
  align-items: baseline;
  gap: 2px 10px;
  margin-top: 3px;
  font-size: 12px;
  color: var(--secondary-text, var(--primary-text));
  opacity: 0.88;
}

.pf-maid {
  color: var(--highlight-text, var(--primary-text));
  font-weight: 500;
  opacity: 1;
}

.pf-maid.is-none {
  color: var(--secondary-text, var(--primary-text));
  font-style: italic;
  font-weight: 400;
  opacity: 0.8;
}

.pf-plus {
  color: var(--pf-ok);
}

.pf-minus {
  color: var(--pf-danger);
}

.pf-history-reason {
  word-break: break-word;
}

@media (prefers-reduced-motion: reduce) {
  .pf-spin {
    animation: none;
  }
}
</style>

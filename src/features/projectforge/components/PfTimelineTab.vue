<script setup lang="ts">
/**
 * PfTimelineTab.vue — 工程详情「开发脉络」tab。
 *
 * 对齐桌面 VCPChat ProjectForge：批次 timeline 列表（b{id}、kind 徽标、署名、时间、
 * 节点数、+/-、reason、涉及文件）；点击批次展开节点列表（n{id}、op、路径、reason、+/-）；
 * 点节点 / 点「查看批次差异」打开 PfDiffSheet。
 *
 * 批次聚合由 T1 detail（GetProject + SearchHistory 聚合）提供；节点列表按 batch 过滤，
 * 优先用 store.getBatchNodes（T1 便捷 action），没有则退化到 searchHistory({ batch })。
 */
import { computed, ref } from 'vue';
import { AlertTriangle, ChevronDown, ChevronRight, GitCompare, Loader2 } from "@lucide/vue";
import { useProjectForgeStore } from '../projectForgeStore';
import type { PfBatchSummary, PfHistoryRow } from '../projectForgeTypes';

type DiffTarget = { kind: 'node'; node: PfHistoryRow } | { kind: 'batch'; batch: PfBatchSummary };

const emit = defineEmits<{ 'open-diff': [target: DiffTarget] }>();

const store = useProjectForgeStore();

const KIND_LABELS: Record<string, string> = {
  edit: '编辑',
  create: '新建',
  remove: '删除',
  delete: '删除',
  move: '移动',
  rollback: '回退',
  external: '外部修改',
};

const batches = computed<PfBatchSummary[]>(() => (store.detail as any)?.timeline ?? []);
const projectId = computed(() => String((store as any).currentProjectId || (store.detail as any)?.project?.id || ''));

const expandedId = ref<number | null>(null);
const nodesById = ref<Record<number, PfHistoryRow[]>>({});
const loadingId = ref<number | null>(null);
const errorsById = ref<Record<number, string>>({});

function errorText(error: unknown): string {
  if (error instanceof Error) return error.message;
  const message = (error as any)?.message;
  return message ? String(message) : String(error ?? '');
}

function formatTime(value?: string | null): string {
  if (!value) return '—';
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return String(value);
  const pad = (input: number) => String(input).padStart(2, '0');
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

function kindLabel(kind?: string | null): string {
  const key = String(kind || 'edit');
  return KIND_LABELS[key] || key;
}

function maidLabel(maid?: string | null, kind?: string): string {
  if (maid) return `@${maid}`;
  return kind === 'external' ? '@外部' : '@未署名';
}

function filesSummary(batch: PfBatchSummary): string {
  const files = Array.isArray(batch.files) ? batch.files : [];
  if (!files.length) return '（无文件记录）';
  const head = files.slice(0, 6).map((file) => String(file)).join('、');
  return files.length > 6 ? `${head} 等 ${files.length} 个` : head;
}

async function loadBatchNodes(batch: PfBatchSummary): Promise<PfHistoryRow[]> {
  const batchId = Number(batch.id);
  const anyStore = store as any;
  let rows: PfHistoryRow[] = [];
  if (typeof anyStore.getBatchNodes === 'function') {
    rows = await anyStore.getBatchNodes(projectId.value, batchId);
  } else {
    rows = await store.searchHistory({ batch: `b${batchId}`, limit: 500 }, { projectId: projectId.value });
  }
  const list = Array.isArray(rows) ? rows : [];
  const filtered = list.filter((row) => Number((row as any).batch_id) === batchId);
  return filtered.length || !list.length ? filtered : list;
}

async function toggleBatch(batch: PfBatchSummary): Promise<void> {
  const batchId = Number(batch.id);
  if (expandedId.value === batchId) {
    expandedId.value = null;
    return;
  }
  expandedId.value = batchId;
  if (nodesById.value[batchId]) return;
  loadingId.value = batchId;
  try {
    const rows = await loadBatchNodes(batch);
    nodesById.value = { ...nodesById.value, [batchId]: rows };
  } catch (error) {
    errorsById.value = { ...errorsById.value, [batchId]: errorText(error) };
  } finally {
    loadingId.value = null;
  }
}

function retryBatch(batch: PfBatchSummary): void {
  const batchId = Number(batch.id);
  const nextNodes = { ...nodesById.value };
  delete nextNodes[batchId];
  nodesById.value = nextNodes;
  const nextErrors = { ...errorsById.value };
  delete nextErrors[batchId];
  errorsById.value = nextErrors;
  expandedId.value = null;
  void toggleBatch(batch);
}

function openNodeDiff(node: PfHistoryRow): void {
  emit('open-diff', { kind: 'node', node });
}

function openBatchDiff(batch: PfBatchSummary): void {
  emit('open-diff', { kind: 'batch', batch });
}

function nodeReason(row: PfHistoryRow): string {
  return String((row as any).effective_reason || (row as any).reason || row.summary || '');
}

function openBatchNodes(batch: PfBatchSummary): PfHistoryRow[] {
  return nodesById.value[Number(batch.id)] ?? [];
}

function opLabel(op?: string | null): string {
  return String(op || 'edit');
}
</script>

<template>
  <div class="pf-timeline">
    <p v-if="!batches.length" class="pf-empty-line">暂无改动</p>

    <ul v-else class="pf-batch-list">
      <li
        v-for="batch in batches"
        :key="batch.id"
        class="pf-batch"
        :class="[`kind-${batch.kind || 'edit'}`, { 'is-open': expandedId === Number(batch.id) }]"
      >
        <div class="pf-batch-card">
          <!-- 批次头：点击展开节点 -->
          <div
            class="pf-batch-head"
            role="button"
            tabindex="0"
            :aria-expanded="expandedId === Number(batch.id)"
            @click="toggleBatch(batch)"
            @keydown.enter.prevent="toggleBatch(batch)"
            @keydown.space.prevent="toggleBatch(batch)"
          >
            <component
              :is="expandedId === Number(batch.id) ? ChevronDown : ChevronRight"
              :size="16"
              class="pf-batch-chevron"
              aria-hidden="true"
            />
            <code class="pf-mono pf-batch-id">b{{ batch.id }}</code>
            <span class="pf-badge">{{ kindLabel(batch.kind) }}</span>
            <span class="pf-maid" :class="{ 'is-none': !batch.maid }">{{ maidLabel(batch.maid, batch.kind) }}</span>
            <span class="pf-muted">{{ formatTime(batch.created_at) }}</span>
            <span class="pf-muted">
              {{ batch.node_count }} 个节点 ·
              <span class="pf-plus">+{{ batch.added }}</span>/<span class="pf-minus">-{{ batch.removed }}</span>
            </span>
          </div>

          <div class="pf-batch-reason">{{ batch.reason || '（未记录原因）' }}</div>
          <div class="pf-batch-files">{{ filesSummary(batch) }}</div>

          <div class="pf-batch-tools">
            <button type="button" class="pf-link-btn" @click.stop="openBatchDiff(batch)">
              <GitCompare :size="14" aria-hidden="true" />
              <span>查看批次差异</span>
            </button>
          </div>

          <!-- 节点列表 -->
          <div v-if="expandedId === Number(batch.id)" class="pf-batch-nodes">
            <div v-if="loadingId === Number(batch.id)" class="pf-nodes-state">
              <Loader2 class="pf-spin" :size="16" aria-hidden="true" />
              <span>加载中…</span>
            </div>
            <div v-else-if="errorsById[Number(batch.id)]" class="pf-nodes-state is-error">
              <AlertTriangle :size="16" aria-hidden="true" />
              <span>{{ errorsById[Number(batch.id)] }}</span>
              <button type="button" class="pf-link-btn" @click="retryBatch(batch)">重试</button>
            </div>
            <p v-else-if="!openBatchNodes(batch).length" class="pf-nodes-state">无节点</p>
            <ul v-else class="pf-node-list">
              <li
                v-for="node in openBatchNodes(batch)"
                :key="node.id"
                class="pf-node"
                role="button"
                tabindex="0"
                :aria-label="`查看节点 n${node.id} 的差异`"
                @click="openNodeDiff(node)"
                @keydown.enter.prevent="openNodeDiff(node)"
                @keydown.space.prevent="openNodeDiff(node)"
              >
                <code class="pf-mono">n{{ node.id }}</code>
                <span class="pf-badge op-badge">{{ opLabel(node.op) }}</span>
                <span class="pf-node-path" :title="node.file_path">{{ node.file_path }}</span>
                <span class="pf-node-reason">{{ nodeReason(node) }}</span>
                <span class="pf-node-diff">
                  <span class="pf-plus">+{{ node.added }}</span>/<span class="pf-minus">-{{ node.removed }}</span>
                </span>
              </li>
            </ul>
          </div>
        </div>
      </li>
    </ul>
  </div>
</template>

<style scoped>
.pf-timeline {
  --pf-ok: #4caf7d;
  --pf-warn: #e0a84a;
  --pf-danger: #e57373;
  padding: 0 0 calc(var(--vcp-safe-bottom, 0px) + 16px);
  color: var(--primary-text);
  font-size: 14px;
}

.pf-empty-line {
  margin: 0;
  padding: 24px 8px;
  text-align: center;
  font-size: 13px;
  color: var(--secondary-text, var(--primary-text));
  opacity: 0.7;
}

.pf-batch-list {
  list-style: none;
  margin: 0;
  padding: 0;
}

.pf-batch {
  position: relative;
  padding-left: 14px;
  margin-bottom: 10px;
}

.pf-batch::before {
  content: '';
  position: absolute;
  left: 3px;
  top: 8px;
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: var(--primary-bg);
  border: 2px solid var(--highlight-text, var(--primary-text));
}

.pf-batch.kind-rollback::before {
  border-color: var(--pf-warn);
}

.pf-batch.kind-external::before {
  border-color: #b388ff;
}

.pf-batch.kind-remove::before,
.pf-batch.kind-delete::before {
  border-color: var(--pf-danger);
}

.pf-batch-card {
  border: 1px solid var(--border-color);
  border-radius: 10px;
  padding: 10px 12px;
}

.pf-batch-head {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 6px 8px;
  min-height: 44px;
  cursor: pointer;
  outline: none;
}

.pf-batch-head:focus-visible {
  outline: 2px solid var(--highlight-text, var(--primary-text));
  outline-offset: 2px;
  border-radius: 4px;
}

.pf-batch-chevron {
  flex-shrink: 0;
  opacity: 0.7;
}

.pf-batch-id {
  font-family: Consolas, 'Cascadia Code', ui-monospace, monospace;
  font-size: 13px;
  font-weight: 600;
}

.pf-badge {
  padding: 1px 8px;
  border: 1px solid var(--border-color);
  border-radius: 999px;
  font-size: 11px;
  color: var(--secondary-text, var(--primary-text));
  white-space: nowrap;
}

.pf-badge.op-badge {
  text-transform: lowercase;
}

.pf-maid {
  color: var(--highlight-text, var(--primary-text));
  font-size: 12px;
  font-weight: 500;
}

.pf-maid.is-none {
  color: var(--secondary-text, var(--primary-text));
  font-style: italic;
  font-weight: 400;
}

.pf-muted {
  color: var(--secondary-text, var(--primary-text));
  font-size: 12px;
  opacity: 0.8;
}

.pf-plus {
  color: var(--pf-ok);
}

.pf-minus {
  color: var(--pf-danger);
}

.pf-batch-reason {
  margin-top: 6px;
  font-size: 13px;
  line-height: 1.55;
  word-break: break-word;
}

.pf-batch-files {
  margin-top: 4px;
  font-size: 12px;
  color: var(--secondary-text, var(--primary-text));
  opacity: 0.75;
  word-break: break-all;
}

.pf-batch-tools {
  display: flex;
  justify-content: flex-end;
  margin-top: 2px;
}

.pf-link-btn {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  min-height: 40px;
  padding: 0 6px;
  border: none;
  background: transparent;
  color: var(--highlight-text, var(--primary-text));
  font: inherit;
  font-size: 12px;
  white-space: nowrap;
}

.pf-batch-nodes {
  margin-top: 6px;
  padding-top: 6px;
  border-top: 1px dashed var(--border-color);
}

.pf-nodes-state {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 6px;
  padding: 12px 4px;
  font-size: 12px;
  color: var(--secondary-text, var(--primary-text));
  opacity: 0.8;
}

.pf-nodes-state.is-error {
  color: var(--pf-danger);
  opacity: 1;
}

.pf-spin {
  animation: pf-rotate 0.9s linear infinite;
}

@keyframes pf-rotate {
  to {
    transform: rotate(360deg);
  }
}

.pf-node-list {
  list-style: none;
  margin: 0;
  padding: 0;
}

.pf-node {
  display: flex;
  flex-wrap: wrap;
  align-items: baseline;
  gap: 4px 8px;
  min-height: 44px;
  padding: 8px 6px;
  border-radius: 6px;
  cursor: pointer;
  font-size: 13px;
  outline: none;
}

.pf-node:active {
  background: var(--secondary-bg);
}

.pf-node:focus-visible {
  outline: 2px solid var(--highlight-text, var(--primary-text));
  outline-offset: -2px;
}

.pf-mono {
  font-family: Consolas, 'Cascadia Code', ui-monospace, monospace;
  font-size: 12px;
}

.pf-node-path {
  flex: 1;
  min-width: 120px;
  font-family: Consolas, 'Cascadia Code', ui-monospace, monospace;
  font-size: 12px;
  word-break: break-all;
}

.pf-node-reason {
  width: 100%;
  font-size: 12px;
  color: var(--secondary-text, var(--primary-text));
  opacity: 0.8;
  word-break: break-word;
}

.pf-node-diff {
  flex-shrink: 0;
  font-size: 12px;
}

@media (prefers-reduced-motion: reduce) {
  .pf-spin {
    animation: none;
  }
}
</style>

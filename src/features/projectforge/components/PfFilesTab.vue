<script setup lang="ts">
/**
 * PfFilesTab.vue — 工程详情「改动文件」tab。
 *
 * 对齐桌面 VCPChat ProjectForge：文件表（文件路径 / 改动次数 / +/- / 最近节点 n{id}）。
 * 点文件 → 跳搜索历史并按 file 过滤；点「最近节点」→ 直接打开该节点 diff。
 *
 * 文件聚合（edits/added/removed/last_node）由 T1 detail（SearchHistory 聚合）提供。
 */
import { computed } from 'vue';
import { FileCode2, GitCompare } from "@lucide/vue";
import { useProjectForgeStore } from '../projectForgeStore';
import type { PfFileChange, PfHistoryRow } from '../projectForgeTypes';

type DiffTarget = { kind: 'node'; node: PfHistoryRow } | { kind: 'batch'; batch: any };

const emit = defineEmits<{
  'open-history': [filter: { file: string }];
  'open-diff': [target: DiffTarget];
}>();

const store = useProjectForgeStore();

const files = computed<PfFileChange[]>(() => (store.detail as any)?.files ?? []);

function openFileHistory(file: PfFileChange): void {
  if (!file?.file_path) return;
  emit('open-history', { file: String(file.file_path) });
}

/**
 * 用文件聚合行构造一个最小节点记录打开 diff（GetNodeDiff 只需要 node id；
 * 回退「撤销此变动」会用 searchHistory 反查同文件上一节点，不依赖 batch_id）。
 */
function openLastNodeDiff(file: PfFileChange): void {
  const nodeId = Number(file.last_node);
  if (!Number.isFinite(nodeId) || nodeId <= 0) return;
  const node: PfHistoryRow = {
    id: nodeId,
    batch_id: 0,
    op: 'edit',
    file_path: String(file.file_path || ''),
    maid: null,
    effective_reason: '',
    summary: '',
    added: Number(file.added || 0),
    removed: Number(file.removed || 0),
    created_at: '',
  };
  emit('open-diff', { kind: 'node', node });
}

function formatCount(value: unknown): number {
  return Number(value || 0);
}
</script>

<template>
  <div class="pf-files">
    <p v-if="!files.length" class="pf-empty-line">暂无改动文件</p>

    <ul v-else class="pf-file-list" aria-label="改动文件列表">
      <li v-for="file in files" :key="file.file_path" class="pf-file">
        <div
          class="pf-file-main"
          role="button"
          tabindex="0"
          :aria-label="`查看 ${file.file_path} 的历史`"
          @click="openFileHistory(file)"
          @keydown.enter.prevent="openFileHistory(file)"
          @keydown.space.prevent="openFileHistory(file)"
        >
          <div class="pf-file-row">
            <FileCode2 :size="16" class="pf-file-icon" aria-hidden="true" />
            <span class="pf-file-path" :title="file.file_path">{{ file.file_path }}</span>
          </div>
          <div class="pf-file-meta">
            <span>改动 {{ formatCount(file.edits) }} 次</span>
            <span class="pf-muted">·</span>
            <span>
              <span class="pf-plus">+{{ formatCount(file.added) }}</span> /
              <span class="pf-minus">-{{ formatCount(file.removed) }}</span>
            </span>
            <span class="pf-muted">·</span>
            <span>最近 <code class="pf-mono">n{{ file.last_node }}</code></span>
          </div>
        </div>
        <button
          type="button"
          class="pf-link-btn"
          :aria-label="`查看 ${file.file_path} 最近节点的差异`"
          @click.stop="openLastNodeDiff(file)"
        >
          <GitCompare :size="14" aria-hidden="true" />
          <span>最近节点差异</span>
        </button>
      </li>
    </ul>
  </div>
</template>

<style scoped>
.pf-files {
  --pf-ok: #4caf7d;
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

.pf-file-list {
  list-style: none;
  margin: 0;
  padding: 0;
}

.pf-file {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 0;
  border-bottom: 1px solid var(--border-color);
}

.pf-file-main {
  flex: 1;
  min-width: 0;
  cursor: pointer;
  outline: none;
  min-height: 44px;
  display: flex;
  flex-direction: column;
  justify-content: center;
}

.pf-file-main:focus-visible {
  outline: 2px solid var(--highlight-text, var(--primary-text));
  outline-offset: 2px;
  border-radius: 4px;
}

.pf-file-row {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
}

.pf-file-icon {
  flex-shrink: 0;
  opacity: 0.65;
}

.pf-file-path {
  min-width: 0;
  font-family: Consolas, 'Cascadia Code', ui-monospace, monospace;
  font-size: 12px;
  word-break: break-all;
}

.pf-file-meta {
  display: flex;
  flex-wrap: wrap;
  align-items: baseline;
  gap: 0 6px;
  margin-top: 3px;
  font-size: 12px;
  color: var(--secondary-text, var(--primary-text));
  opacity: 0.85;
}

.pf-mono {
  font-family: Consolas, 'Cascadia Code', ui-monospace, monospace;
  font-size: 12px;
}

.pf-muted {
  color: var(--secondary-text, var(--primary-text));
  opacity: 0.6;
}

.pf-plus {
  color: var(--pf-ok);
}

.pf-minus {
  color: var(--pf-danger);
}

.pf-link-btn {
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  gap: 4px;
  min-height: 44px;
  padding: 0 8px;
  border: none;
  background: transparent;
  color: var(--highlight-text, var(--primary-text));
  font: inherit;
  font-size: 12px;
  white-space: nowrap;
}
</style>

<script setup lang="ts">
/**
 * PfDiffSheet.vue — 节点 / 批次 Diff 底部弹层 + 回退栏。
 *
 * 对齐桌面 VCPChat ProjectForge 节点弹窗：
 * - GetNodeDiff(node=nX | batch=bX) 统一 diff 渲染（+/- 着色、等宽、横向滚动、相同行号栏）；
 * - 回退栏：署名显示、回退原因输入、「撤销此变动」（恢复到该节点之前，需反查同文件上一节点）、
 *   「恢复到此版本」（rollback toNode 该节点）、批次模式下「撤销此批次」（rollback batch）；
 * - 只读 / 已删除工程禁止回退；无署名禁止回退；操作二次确认；冲突时强确认（force）；
 * - 成功 / 失败 toast 通过 notificationStore。
 *
 * 组件只调用 T1 store 的 getNodeDiff / rollback / searchHistory，不直接 fetch。
 */
import { computed, onBeforeUnmount, ref, watch } from 'vue';
import { AlertTriangle, Check, Loader2, RotateCcw, Undo2, X } from "@lucide/vue";
import { useModalHistory } from '../../../core/composables/useModalHistory';
import { useNotificationStore } from '../../../core/stores/notification';
import { useProjectForgeStore } from '../projectForgeStore';
import type { PfBatchSummary, PfDiffResult, PfHistoryRow } from '../projectForgeTypes';

type DiffTarget = { kind: 'node'; node: PfHistoryRow } | { kind: 'batch'; batch: PfBatchSummary };
type RollbackMode = 'before' | 'after' | 'batch';
type DiffLineKind = 'add' | 'del' | 'hunk' | 'meta' | 'context' | 'header';

interface DiffLine {
  kind: DiffLineKind;
  text: string;
}

const props = withDefaults(
  defineProps<{
    open: boolean;
    projectId: string;
    target: DiffTarget | null;
    /** 工程只读或已删除：禁止回退。 */
    readonly?: boolean;
  }>(),
  { readonly: false },
);

const emit = defineEmits<{ close: []; 'rolled-back': [] }>();

const store = useProjectForgeStore();
const notificationStore = useNotificationStore();
const { registerModal, unregisterModal } = useModalHistory();

const MODAL_ID = 'ProjectForge:DiffSheet';

const diff = ref<PfDiffResult | null>(null);
const loading = ref(false);
const loadError = ref('');
const reason = ref('');
const rollbackBusy = ref(false);
const rollbackError = ref('');
const previousNode = ref<PfHistoryRow | null>(null);
const previousLoading = ref(false);
const confirmMode = ref<RollbackMode | null>(null);
const forceNeeded = ref(false);

const signature = computed(() => String((store as any).signature || '').trim());

const targetInfo = computed(() => {
  const target = props.target;
  if (!target) return null;
  if (target.kind === 'node') {
    return {
      kind: 'node' as const,
      id: Number(target.node.id),
      batchId: Number((target.node as any).batch_id || 0),
      filePath: String(target.node.file_path || ''),
      maid: (target.node as any).maid ?? null,
      batchKind: String((target.node as any).batch_kind || ''),
      op: String(target.node.op || 'edit'),
      createdAt: String(target.node.created_at || ''),
      added: Number(target.node.added || 0),
      removed: Number(target.node.removed || 0),
      reason: String((target.node as any).effective_reason || (target.node as any).reason || target.node.summary || ''),
    };
  }
  const batch = target.batch;
  return {
    kind: 'batch' as const,
    id: Number(batch.id),
    batchId: Number(batch.id),
    filePath: '',
    maid: batch.maid ?? null,
    op: String(batch.kind || 'edit'),
    createdAt: String(batch.created_at || ''),
    added: Number(batch.added || 0),
    removed: Number(batch.removed || 0),
    reason: String(batch.reason || ''),
    nodeCount: Number(batch.node_count || 0),
    files: Array.isArray(batch.files) ? batch.files : [],
  };
});

const title = computed(() => {
  const info = targetInfo.value;
  if (!info) return '节点差异';
  if (info.kind === 'node') return `n${info.id} · ${info.filePath || '节点差异'}`;
  return `批次 b${info.id} · 改动差异`;
});

const canRollback = computed(() => !props.readonly && Boolean(signature.value));

const rollbackBlockReason = computed(() => {
  if (props.readonly) return '工程只读或已删除，无法回退';
  if (!signature.value) return '回退需要署名，请先在页面顶部填写署名';
  return '';
});

// ---------------------------------------------------------------------------
// 工具
// ---------------------------------------------------------------------------

function errorText(error: unknown): string {
  if (error instanceof Error) return error.message;
  const message = (error as any)?.message;
  return message ? String(message) : String(error ?? '');
}

function notify(type: 'success' | 'error' | 'info', message: string): void {
  notificationStore.addNotification({
    id: `pf-diff-${Date.now()}-${Math.random().toString(36).slice(2, 6)}`,
    title: 'V工程',
    message,
    type,
    duration: type === 'error' ? 5000 : 2600,
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

function maidLabel(maid?: string | null, kind?: string): string {
  if (maid) return `@${maid}`;
  return kind === 'external' ? '@外部' : '@未署名';
}

function classifyLine(line: string): DiffLine {
  if (/^(diff --git|index |--- |\+\+\+ |new file mode|deleted file mode|old mode|new mode|similarity index|rename from|rename to)/.test(line)) {
    return { kind: 'header', text: line };
  }
  if (line.startsWith('@@')) return { kind: 'hunk', text: line };
  if (line.startsWith('+')) return { kind: 'add', text: line };
  if (line.startsWith('-')) return { kind: 'del', text: line };
  return { kind: 'context', text: line };
}

/**
 * GetNodeDiff 返回的 text 是 content[0] Markdown（标题 + ```diff 围栏）。
 * 解析时剥掉围栏行：围栏外按 meta 展示，围栏内按 unified diff 着色。
 */
function parseDiffText(raw: string): DiffLine[] {
  const lines = String(raw || '').replace(/\r\n?/g, '\n').split('\n');
  const result: DiffLine[] = [];
  let inFence = false;
  let sawFence = false;
  for (const line of lines) {
    if (/^\s*```/.test(line)) {
      inFence = !inFence;
      sawFence = true;
      continue;
    }
    if (!sawFence) {
      result.push(classifyLine(line));
      continue;
    }
    if (inFence) {
      result.push(classifyLine(line));
    } else if (line.trim()) {
      result.push({ kind: 'meta', text: line });
    }
  }
  while (result.length && !result[result.length - 1].text.trim()) result.pop();
  return result;
}

const diffLines = computed<DiffLine[]>(() => parseDiffText(String(diff.value?.text || '')));

// ---------------------------------------------------------------------------
// 加载 diff / 上一节点
// ---------------------------------------------------------------------------

async function loadDiff(): Promise<void> {
  const target = props.target;
  if (!target) return;
  loading.value = true;
  loadError.value = '';
  diff.value = null;
  try {
    if (target.kind === 'node') {
      diff.value = await store.getNodeDiff(props.projectId, { node: target.node.id, maxLines: 800 });
    } else {
      diff.value = await store.getNodeDiff(props.projectId, { batch: target.batch.id, maxLines: 800 });
    }
  } catch (error) {
    loadError.value = errorText(error);
  } finally {
    loading.value = false;
  }
}

async function loadPreviousNode(node: PfHistoryRow): Promise<void> {
  previousNode.value = null;
  previousLoading.value = true;
  try {
    const rows = await store.searchHistory(
      { file: String(node.file_path || ''), limit: 500 },
      { projectId: props.projectId },
    );
    const list = Array.isArray(rows) ? rows : [];
    const candidates = list
      .filter((row) => Number(row.id) < Number(node.id))
      .filter((row) => !row.file_path || String(row.file_path) === String(node.file_path || ''))
      .sort((a, b) => Number(b.id) - Number(a.id));
    previousNode.value = candidates[0] ?? null;
  } catch {
    previousNode.value = null;
  } finally {
    previousLoading.value = false;
  }
}

function resetState(): void {
  diff.value = null;
  loadError.value = '';
  reason.value = '';
  rollbackError.value = '';
  previousNode.value = null;
  previousLoading.value = false;
  confirmMode.value = null;
  forceNeeded.value = false;
}

watch(
  () => [props.open, props.target] as const,
  () => {
    if (!props.open || !props.target) {
      resetState();
      return;
    }
    resetState();
    void loadDiff();
    if (props.target.kind === 'node') void loadPreviousNode(props.target.node);
  },
  { immediate: true },
);

// 返回手势 / 硬件返回先关闭弹层
watch(
  () => props.open,
  (open) => {
    if (open) registerModal(MODAL_ID, () => emit('close'));
    else unregisterModal(MODAL_ID);
  },
  { immediate: true },
);

onBeforeUnmount(() => {
  unregisterModal(MODAL_ID);
});

// ---------------------------------------------------------------------------
// 回退
// ---------------------------------------------------------------------------

const confirmTitle = computed(() => {
  if (confirmMode.value === 'before') return '撤销此变动';
  if (confirmMode.value === 'after') return '恢复到此版本';
  return '撤销此批次';
});

const confirmLines = computed<{ label: string; value: string }[]>(() => {
  const info = targetInfo.value;
  if (!info) return [];
  const lines: { label: string; value: string }[] = [];
  if (confirmMode.value === 'before' && info.kind === 'node') {
    lines.push({ label: '动作', value: `把文件恢复到 n${info.id} 这次改动之前` });
    lines.push({ label: '文件', value: info.filePath });
    lines.push({
      label: '上一节点',
      value: previousNode.value ? `n${previousNode.value.id}` : '（没有更早节点）',
    });
  } else if (confirmMode.value === 'after' && info.kind === 'node') {
    lines.push({ label: '动作', value: `把文件恢复到 n${info.id} 完成时的状态` });
    lines.push({ label: '文件', value: info.filePath });
    lines.push({ label: '目标节点', value: `n${info.id}` });
  } else if (confirmMode.value === 'batch') {
    lines.push({ label: '动作', value: `撤销整个批次 b${info.batchId}（批量回退）` });
    lines.push({ label: '节点数', value: String((info as any).nodeCount || '—') });
    if (info.kind === 'node') lines.push({ label: '批次', value: `b${info.batchId}` });
  }
  lines.push({ label: '署名', value: `@${signature.value || '未设置'}` });
  if (reason.value.trim()) lines.push({ label: '原因', value: reason.value.trim() });
  return lines;
});

function askRollback(mode: RollbackMode): void {
  rollbackError.value = '';
  if (!canRollback.value) {
    notify('error', rollbackBlockReason.value || '当前状态无法回退');
    return;
  }
  if (mode === 'before') {
    if (previousLoading.value) return;
    if (!previousNode.value) {
      notify('error', '该文件没有更早的节点，无法单独撤销这次变动；可改为「撤销此批次」。');
      return;
    }
  }
  forceNeeded.value = false;
  confirmMode.value = mode;
}

function cancelConfirm(): void {
  confirmMode.value = null;
  forceNeeded.value = false;
  rollbackError.value = '';
}

function needsForce(message: string): boolean {
  return /冲突|强制|force|不一致|外部修改|手动修改|已被修改|磁盘内容|覆盖|conflict/i.test(message);
}

async function performRollback(force = false): Promise<void> {
  const mode = confirmMode.value;
  const info = targetInfo.value;
  if (!mode || !info || rollbackBusy.value) return;
  rollbackBusy.value = true;
  rollbackError.value = '';
  try {
    const args: Record<string, unknown> = {
      projectId: props.projectId,
      force,
      reason: reason.value.trim() || undefined,
    };
    if (mode === 'batch') {
      args.batch = info.batchId;
    } else if (mode === 'after' && info.kind === 'node') {
      args.toNode = info.id;
      args.path = info.filePath;
    } else if (mode === 'before' && info.kind === 'node' && previousNode.value) {
      args.toNode = previousNode.value.id;
      args.path = info.filePath;
    } else {
      rollbackBusy.value = false;
      return;
    }

    const result: any = await store.rollback(args as any);
    if (result?.status === 'conflict') {
      if (force) {
        rollbackError.value = '回退仍然冲突：文件在确认期间发生变化，请重新操作。';
        notify('error', rollbackError.value);
        cancelConfirm();
        return;
      }
      forceNeeded.value = true;
      return;
    }
    if (result?.status === 'noop') {
      notify('info', '磁盘内容已是目标状态，无需回退。');
      cancelConfirm();
      return;
    }

    const batchId = result?.batchId != null ? Number(result.batchId) : null;
    const maid = String(result?.maid || signature.value || '');
    notify(
      'success',
      batchId ? `已回退：批次 b${batchId}（${maid ? `@${maid}` : '已记录'}）` : '回退成功',
    );
    cancelConfirm();
    emit('rolled-back');
    emit('close');
  } catch (error) {
    const message = errorText(error);
    if (!force && needsForce(message)) {
      forceNeeded.value = true;
      return;
    }
    rollbackError.value = message;
    notify('error', `回退失败：${message}`);
  } finally {
    rollbackBusy.value = false;
  }
}

watch(
  () => forceNeeded.value,
  (needed) => {
    if (!needed) rollbackError.value = '';
  },
);
</script>

<template>
  <Teleport to="body">
    <div v-if="props.open && props.target" class="pf-sheet-mask" @click.self="emit('close')">
      <section class="pf-sheet" role="dialog" aria-modal="true" :aria-label="title">
        <!-- 头部 -->
        <header class="pf-sheet-head">
          <div class="pf-sheet-title-block">
            <div class="pf-sheet-title">
              <span class="pf-sheet-title-text">{{ title }}</span>
            </div>
            <div v-if="targetInfo" class="pf-sheet-meta">
              <span v-if="targetInfo.kind === 'node'">批次 <code class="pf-mono">b{{ targetInfo.batchId || '-' }}</code></span>
              <span>操作者 <span class="pf-maid">{{ maidLabel(targetInfo.maid, targetInfo.batchKind || targetInfo.op) }}</span></span>
              <span>{{ formatTime(targetInfo.createdAt) }}</span>
              <span>
                <span class="pf-plus">+{{ targetInfo.added }}</span>/<span class="pf-minus">-{{ targetInfo.removed }}</span>
              </span>
              <span v-if="targetInfo.reason" class="pf-sheet-reason">{{ targetInfo.reason }}</span>
            </div>
          </div>
          <button type="button" class="pf-icon-btn" aria-label="关闭差异弹层" @click="emit('close')">
            <X :size="20" />
          </button>
        </header>

        <!-- diff 区 -->
        <div class="pf-diff-area">
          <div v-if="loading" class="pf-diff-state">
            <Loader2 class="pf-spin" :size="18" aria-hidden="true" />
            <span>正在读取差异…</span>
          </div>
          <div v-else-if="loadError" class="pf-diff-state is-error">
            <AlertTriangle :size="18" aria-hidden="true" />
            <p>{{ loadError }}</p>
            <button type="button" class="pf-btn" @click="loadDiff">重试</button>
          </div>
          <div v-else-if="!diffLines.length" class="pf-diff-state">
            <p>{{ diff?.title || '（没有可显示的 diff）' }}</p>
          </div>
          <template v-else>
            <div class="pf-diff-legend">
              <span>改动前（红）</span>
              <span>改动后（绿）</span>
            </div>
            <div class="pf-diff-scroll" role="region" aria-label="节点差异内容">
              <div
                v-for="(line, index) in diffLines"
                :key="index"
                class="pf-diff-line"
                :class="`is-${line.kind}`"
              >
                <span class="pf-diff-marker" aria-hidden="true">{{ line.kind === 'add' ? '+' : line.kind === 'del' ? '-' : line.kind === 'hunk' ? '@' : ' ' }}</span>
                <span class="pf-diff-text">{{ line.text }}</span>
              </div>
            </div>
          </template>
        </div>

        <!-- 回退栏 -->
        <footer class="pf-revert">
          <p v-if="rollbackBlockReason" class="pf-revert-hint is-warn">⚠ {{ rollbackBlockReason }}</p>
          <p v-else-if="targetInfo?.kind === 'node'" class="pf-revert-hint">
            署名 <span class="pf-maid">@{{ signature }}</span>
            <template v-if="previousLoading">· 正在查找该文件上一节点…</template>
            <template v-else-if="previousNode">· 该文件上一节点 <code class="pf-mono">n{{ previousNode.id }}</code></template>
            <template v-else>· 该文件没有更早节点（「撤销此变动」不可用）</template>
          </p>
          <p v-else-if="targetInfo?.kind === 'batch'" class="pf-revert-hint">
            署名 <span class="pf-maid">@{{ signature }}</span> · 将撤销整个批次 b{{ targetInfo.batchId }}
          </p>
          <p v-if="rollbackError" class="pf-revert-hint is-error">{{ rollbackError }}</p>

          <div class="pf-revert-row">
            <input
              v-model="reason"
              type="text"
              class="pf-revert-reason"
              maxlength="500"
              placeholder="回退原因（可选，将写入开发脉络）"
              aria-label="回退原因"
              :disabled="!canRollback || rollbackBusy"
            />
          </div>

          <div class="pf-revert-actions">
            <template v-if="targetInfo?.kind === 'node'">
              <button
                type="button"
                class="pf-btn danger"
                :disabled="!canRollback || rollbackBusy || !previousNode || previousLoading"
                :title="previousNode ? `把文件恢复到 n${previousNode.id}（这次改动之前）` : '该文件没有更早节点'"
                @click="askRollback('before')"
              >
                <Undo2 :size="16" aria-hidden="true" />
                <span>{{ rollbackBusy && confirmMode === 'before' ? '回退中…' : '撤销此变动' }}</span>
              </button>
              <button
                type="button"
                class="pf-btn primary"
                :disabled="!canRollback || rollbackBusy"
                :title="`把文件恢复到 n${targetInfo.id} 完成时的状态`"
                @click="askRollback('after')"
              >
                <Check :size="16" aria-hidden="true" />
                <span>恢复到此版本</span>
              </button>
            </template>
            <button
              v-else
              type="button"
              class="pf-btn danger"
              :disabled="!canRollback || rollbackBusy"
              :title="`撤销批次 b${targetInfo?.batchId}`"
              @click="askRollback('batch')"
            >
              <RotateCcw :size="16" aria-hidden="true" />
              <span>{{ rollbackBusy ? '回退中…' : '撤销此批次' }}</span>
            </button>
          </div>
        </footer>
      </section>

      <!-- 二次确认 -->
      <div v-if="confirmMode" class="pf-confirm-mask" @click.self="cancelConfirm">
        <div class="pf-confirm" role="dialog" aria-modal="true" :aria-label="confirmTitle">
          <h3 class="pf-confirm-title">{{ forceNeeded ? '需要强制回退' : confirmTitle }}</h3>

          <template v-if="forceNeeded">
            <p class="pf-confirm-warn">
              <AlertTriangle :size="15" aria-hidden="true" />
              <span>磁盘内容与记录不一致（或该文件之后还有改动）。继续将强制覆盖，当前内容会先存快照，仍可再回退。</span>
            </p>
            <p v-if="rollbackError" class="pf-confirm-line">{{ rollbackError }}</p>
          </template>

          <template v-else>
            <p v-for="line in confirmLines" :key="line.label" class="pf-confirm-line">
              <span class="pf-confirm-label">{{ line.label }}</span>
              <span class="pf-confirm-value">{{ line.value }}</span>
            </p>
          </template>

          <div class="pf-confirm-actions">
            <button type="button" class="pf-btn" :disabled="rollbackBusy" @click="cancelConfirm">取消</button>
            <button
              type="button"
              class="pf-btn danger"
              :disabled="rollbackBusy"
              @click="performRollback(forceNeeded)"
            >
              {{ rollbackBusy ? '回退中…' : forceNeeded ? '强制回退' : '确认回退' }}
            </button>
          </div>
        </div>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.pf-sheet-mask {
  position: fixed;
  inset: 0;
  z-index: var(--layer-sheet, 50);
  display: flex;
  align-items: flex-end;
  background: rgba(0, 0, 0, 0.55);
}

.pf-sheet {
  --pf-ok: #4caf7d;
  --pf-warn: #e0a84a;
  --pf-danger: #e57373;
  display: flex;
  flex-direction: column;
  width: 100%;
  height: 92vh;
  max-height: 92vh;
  min-height: 0;
  background: var(--primary-bg);
  color: var(--primary-text);
  border-top-left-radius: 14px;
  border-top-right-radius: 14px;
  padding: 10px 12px calc(var(--vcp-safe-bottom, 0px) + var(--vcp-ime-offset, 0px) + 10px);
  box-sizing: border-box;
  animation: pf-sheet-up 0.22s cubic-bezier(0.2, 0.8, 0.2, 1);
}

@keyframes pf-sheet-up {
  from {
    transform: translateY(100%);
  }
  to {
    transform: translateY(0);
  }
}

/* ---------- 头部 ---------- */
.pf-sheet-head {
  display: flex;
  align-items: flex-start;
  gap: 8px;
  flex-shrink: 0;
  padding-bottom: 8px;
  border-bottom: 1px solid var(--border-color);
}

.pf-sheet-title-block {
  flex: 1;
  min-width: 0;
}

.pf-sheet-title {
  display: flex;
  align-items: baseline;
  gap: 6px;
}

.pf-sheet-title-text {
  font-size: 15px;
  font-weight: 600;
  word-break: break-all;
}

.pf-sheet-meta {
  display: flex;
  flex-wrap: wrap;
  align-items: baseline;
  gap: 2px 10px;
  margin-top: 4px;
  font-size: 12px;
  color: var(--secondary-text, var(--primary-text));
  opacity: 0.85;
}

.pf-sheet-reason {
  width: 100%;
  color: var(--primary-text);
  opacity: 0.9;
  word-break: break-word;
}

.pf-icon-btn {
  flex-shrink: 0;
  width: 44px;
  height: 44px;
  display: grid;
  place-items: center;
  border: none;
  background: transparent;
  color: inherit;
  opacity: 0.8;
}

.pf-icon-btn:active {
  background: var(--secondary-bg);
}

.pf-maid {
  color: var(--highlight-text, var(--primary-text));
  font-weight: 500;
}

.pf-plus {
  color: var(--pf-ok);
}

.pf-minus {
  color: var(--pf-danger);
}

.pf-mono {
  font-family: Consolas, 'Cascadia Code', ui-monospace, monospace;
  font-size: 12px;
}

/* ---------- diff ---------- */
.pf-diff-area {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  margin-top: 8px;
}

.pf-diff-legend {
  display: flex;
  justify-content: space-between;
  padding: 0 4px 4px;
  font-size: 11px;
  color: var(--secondary-text, var(--primary-text));
  opacity: 0.7;
  flex-shrink: 0;
}

.pf-diff-scroll {
  flex: 1;
  min-height: 0;
  overflow: auto;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: var(--secondary-bg);
  font-family: Consolas, 'Cascadia Code', ui-monospace, monospace;
  font-size: 12px;
  line-height: 1.55;
  -webkit-overflow-scrolling: touch;
}

.pf-diff-line {
  display: flex;
  align-items: flex-start;
  min-width: max-content;
  padding: 0 8px;
  white-space: pre;
}

.pf-diff-line.is-add {
  background: rgba(46, 160, 67, 0.2);
}

.pf-diff-line.is-del {
  background: rgba(248, 81, 73, 0.18);
}

.pf-diff-line.is-hunk {
  color: var(--highlight-text, var(--primary-text));
  background: rgba(127, 127, 127, 0.12);
}

.pf-diff-line.is-header {
  color: var(--secondary-text, var(--primary-text));
  opacity: 0.8;
}

.pf-diff-line.is-meta {
  color: var(--secondary-text, var(--primary-text));
  opacity: 0.85;
  font-family: inherit;
  white-space: pre-wrap;
  word-break: break-word;
}

.pf-diff-marker {
  flex-shrink: 0;
  width: 2ch;
  padding-right: 6px;
  opacity: 0.6;
  user-select: none;
}

.pf-diff-text {
  flex: 1;
}

.pf-diff-state {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 8px;
  padding: 24px 16px;
  text-align: center;
  font-size: 13px;
  color: var(--secondary-text, var(--primary-text));
  opacity: 0.85;
}

.pf-diff-state.is-error {
  color: var(--pf-danger);
  opacity: 1;
}

.pf-diff-state p {
  margin: 0;
  max-width: 320px;
  line-height: 1.6;
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

/* ---------- 回退栏 ---------- */
.pf-revert {
  flex-shrink: 0;
  margin-top: 8px;
  padding-top: 8px;
  border-top: 1px solid var(--border-color);
}

.pf-revert-hint {
  margin: 0 0 6px;
  font-size: 12px;
  line-height: 1.5;
  color: var(--secondary-text, var(--primary-text));
  word-break: break-word;
}

.pf-revert-hint.is-warn {
  color: var(--pf-warn);
}

.pf-revert-hint.is-error {
  color: var(--pf-danger);
}

.pf-revert-row {
  display: flex;
}

.pf-revert-reason {
  width: 100%;
  box-sizing: border-box;
  min-height: 44px;
  padding: 0 10px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: var(--secondary-bg);
  color: inherit;
  font: inherit;
  font-size: 13px;
  outline: none;
}

.pf-revert-reason:focus {
  border-color: var(--highlight-text, var(--primary-text));
}

.pf-revert-reason:disabled {
  opacity: 0.5;
}

.pf-revert-actions {
  display: flex;
  gap: 8px;
  margin-top: 8px;
}

.pf-btn {
  flex: 1;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 6px;
  min-height: 44px;
  padding: 0 14px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: transparent;
  color: inherit;
  font: inherit;
  font-size: 14px;
  white-space: nowrap;
}

.pf-btn.danger {
  color: var(--pf-danger);
  border-color: var(--pf-danger);
}

.pf-btn.primary {
  background: var(--highlight-text, var(--primary-text));
  border-color: transparent;
  color: var(--primary-bg);
  font-weight: 600;
}

.pf-btn:disabled {
  opacity: 0.45;
}

/* ---------- 确认 ---------- */
.pf-confirm-mask {
  position: fixed;
  inset: 0;
  z-index: calc(var(--layer-sheet, 50) + 1);
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 24px;
  background: rgba(0, 0, 0, 0.55);
}

.pf-confirm {
  width: 100%;
  max-width: 380px;
  padding: 16px;
  border: 1px solid var(--border-color);
  border-radius: 12px;
  background: var(--secondary-bg);
  color: var(--primary-text);
}

.pf-confirm-title {
  margin: 0 0 10px;
  font-size: 16px;
}

.pf-confirm-line {
  display: flex;
  gap: 8px;
  margin: 0 0 6px;
  font-size: 13px;
  line-height: 1.6;
  word-break: break-word;
}

.pf-confirm-label {
  flex-shrink: 0;
  width: 4em;
  color: var(--secondary-text, var(--primary-text));
  opacity: 0.8;
}

.pf-confirm-value {
  flex: 1;
  min-width: 0;
}

.pf-confirm-warn {
  display: flex;
  align-items: flex-start;
  gap: 6px;
  margin: 0 0 10px;
  font-size: 13px;
  line-height: 1.6;
  color: var(--pf-warn);
}

.pf-confirm-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  margin-top: 12px;
}

.pf-confirm-actions .pf-btn {
  flex: 0 0 auto;
}

@media (prefers-reduced-motion: reduce) {
  .pf-sheet {
    animation: none;
  }

  .pf-spin {
    animation: none;
  }
}
</style>

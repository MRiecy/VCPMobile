/**
 * V工程（桌面端 VCPChat ProjectForge 工程施工台）手机端 Store。
 *
 * 数据来源：MobilePluginBridge 的 `POST /invoke`，toolName 固定 `ProjectForge`。
 * 具体 HTTP/协议细节在 `projectForgeApi.ts`；DTO 在 `projectForgeTypes.ts`。
 *
 * 聚合规则（桌面 GetProject.details 只保证 project/todos/stats）：
 * - timeline：用 `SearchHistory(projectId, limit≈300)` 按 batch_id 聚合为 PfBatchSummary；
 * - files：按 file_path 聚合为 PfFileChange；
 * - contributors：按 maid 聚合为 PfContributor；
 * - stats：优先 GetProject.details.stats；缺失/为 0 且历史非空时用历史行兜底计算。
 *
 * 写操作统一 `runMutation` 串行防重；写成功后自动强制重读 detail（回读确认），
 * 让调用方在 await 结束时 `store.detail` / `store.projects` 已是新数据。
 */
import { computed, ref, watch } from 'vue';
import { defineStore } from 'pinia';
import {
  BRIDGE_STORAGE_KEY,
  LEGACY_STORAGE_KEYS,
  bridgeBaseUrl,
  bridgeConfigSource,
  bridgeToken,
  refreshBridgeConfig,
  saveBridgeConfig,
} from '../../core/bridge/bridgeConfig';
import { useNotificationStore } from '../../core/stores/notification';
import {
  ProjectForgeError,
  bridgeContentParts,
  bridgeContentText,
  pfInvoke,
  pfInvokeEnvelope,
  type PfBridgeEnvelope,
} from './projectForgeApi';
import type {
  PfBatchSummary,
  PfBridgeSettings,
  PfConnectionStatus,
  PfContributor,
  PfCreateFileArgs,
  PfDeleteResult,
  PfDiffResult,
  PfDiffSelector,
  PfEditCodeArgs,
  PfEditResult,
  PfErrorKind,
  PfFileChange,
  PfFindSymbolOptions,
  PfFindSymbolResult,
  PfHistoryFilters,
  PfHistoryRow,
  PfMoveFileArgs,
  PfOutlineOptions,
  PfOutlineResult,
  PfProject,
  PfProjectDetail,
  PfPurgeResult,
  PfReadCodeOptions,
  PfReadCodeResult,
  PfRemoveFileArgs,
  PfResolveEditArgs,
  PfRestoreResult,
  PfRollbackArgs,
  PfRollbackItem,
  PfRollbackResult,
  PfStats,
  PfSubmitReportInput,
  PfTodo,
  PfTodoStatus,
  PfTraceOptions,
  PfTraceResult,
  PfTransferCodeArgs,
  PfWorkspace,
} from './projectForgeTypes';

// 兼容旧版对外导出（ProjectForgeView 等仍 import ProjectForgeError / todoStatusLabel / 类型）
export { ProjectForgeError };
export type { PfBridgeEnvelope };
export type {
  PfBatchSummary,
  PfBridgeSettings,
  PfConnectionStatus,
  PfContributor,
  PfCreateFileArgs,
  PfDeleteResult,
  PfDiffResult,
  PfDiffSelector,
  PfEditCodeArgs,
  PfEditResult,
  PfErrorKind,
  PfFileChange,
  PfFileEntry,
  PfFindSymbolOptions,
  PfFindSymbolResult,
  PfHistoryFilters,
  PfHistoryRow,
  PfLoadingState,
  PfMoveFileArgs,
  PfOutlineOptions,
  PfOutlineResult,
  PfProject,
  PfProjectDetail,
  PfProgress,
  PfPurgeResult,
  PfReadCodeOptions,
  PfReadCodeResult,
  PfRemoveFileArgs,
  PfResolveEditArgs,
  PfRestoreResult,
  PfRollbackArgs,
  PfRollbackItem,
  PfRollbackResult,
  PfRootInfo,
  PfSearchFileHit,
  PfStats,
  PfSubmitReportInput,
  PfTodo,
  PfTodoStatus,
  PfTraceOptions,
  PfTraceResult,
  PfTransferCodeArgs,
  PfWorkspace,
} from './projectForgeTypes';

// ============================ 常量 ============================

/** 署名持久化 key：桌面端同款语义，但移动端独立 key，绝不与桥接配置 key 混用。 */
export const SIGNATURE_STORAGE_KEY = 'vcp.projectforge.signature.v1';
/** 列表一次拉取上限（桌面默认 10，手机列表页放宽到 50）。 */
const LIST_LIMIT = 50;
/** 列表缓存 TTL：60 秒内重复进页面复用缓存，下拉刷新/搜索会强制刷新。 */
const LIST_CACHE_TTL = 60_000;
/** 详情聚合用的历史条数（≈300 条，覆盖最近批次到文件/角色维度）。 */
const DETAIL_HISTORY_LIMIT = 300;
/** 详情时间线最多展示的批次数。 */
const DETAIL_TIMELINE_LIMIT = 300;
/** 旧版 groupTimeline 默认条数。 */
const TIMELINE_BATCH_LIMIT = 10;

const TODO_STATUSES: PfTodoStatus[] = ['pending', 'doing', 'done', 'blocked'];

export const TODO_STATUS_LABELS: Record<string, string> = {
  pending: '待办',
  doing: '进行中',
  done: '已完成',
  blocked: '受阻',
};

export const PF_STATUS_LABELS: Record<string, string> = {
  active: '施工中',
  review: '待验收',
  accepted: '已验收',
};

export const PF_KIND_LABELS: Record<string, string> = {
  create: '新建',
  edit: '编辑',
  remove: '删除',
  move: '移动',
  rollback: '回退',
  external: '外部修改',
};

export function todoStatusLabel(status: string): string {
  return TODO_STATUS_LABELS[status] || status || '待办';
}

export function projectStatusLabel(status: string): string {
  return PF_STATUS_LABELS[status] || status || '施工中';
}

export function opKindLabel(kind: string): string {
  return PF_KIND_LABELS[kind] || kind || '-';
}

// ============================ 归一化 / 聚合（纯函数，可单测） ============================

function toNumber(value: unknown, fallback = 0): number {
  const num = Number(value);
  return Number.isFinite(num) ? num : fallback;
}

function toInt(value: unknown, fallback = 0): number {
  return Math.trunc(toNumber(value, fallback));
}

function trimmedOrNull(value: unknown): string | null {
  if (typeof value !== 'string') return null;
  const text = value.trim();
  return text ? text : null;
}

function asProjectForgeError(input: unknown): ProjectForgeError {
  if (input instanceof ProjectForgeError) return input;
  return new ProjectForgeError('bridge', String((input as Error)?.message || input || '未知错误'));
}

function readStoredSignature(): string {
  try {
    return String(localStorage.getItem(SIGNATURE_STORAGE_KEY) ?? '').trim();
  } catch {
    return '';
  }
}

/** 桌面 ListProjects 的 Markdown 表格行：| `ID` | 名称 | 工作区 | 状态 | Todo | 报告 | 更新 | */
const LIST_ROW_RE = /^\|\s*`([^`]+)`\s*\|([^|]*)\|([^|]*)\|([^|]*)\|([^|]*)\|([^|]*)\|([^|]*)\|\s*$/;

/** 从 ListProjects 的 Markdown 摘要中解析 todo 进度与报告标记（结构化 details 不含这些字段）。 */
export function parseProjectProgress(
  text: string,
): Map<string, { done: number; total: number; hasReport: boolean }> {
  const result = new Map<string, { done: number; total: number; hasReport: boolean }>();
  for (const rawLine of String(text ?? '').split(/\r?\n/)) {
    const matched = LIST_ROW_RE.exec(rawLine.trim());
    if (!matched) continue;
    const id = matched[1].trim();
    const todoCell = matched[5] || '';
    const reportCell = matched[6] || '';
    const progress = /(\d+)\s*\/\s*(\d+)/.exec(todoCell);
    result.set(id, {
      done: progress ? Number(progress[1]) : 0,
      total: progress ? Number(progress[2]) : 0,
      hasReport: reportCell.includes('有'),
    });
  }
  return result;
}

export function normalizeWorkspace(raw: unknown): PfWorkspace {
  const value = (raw ?? {}) as Record<string, unknown>;
  return {
    id: String(value.id ?? ''),
    alias: String(value.alias ?? ''),
    path: String(value.path ?? ''),
    enabled: value.enabled !== false,
    status: value.status ? String(value.status) : undefined,
    current: value.current === true ? true : undefined,
  };
}

export function normalizeStats(raw: unknown): PfStats {
  const value = (raw ?? {}) as Record<string, unknown>;
  return {
    nodeCount: toInt(value.nodeCount),
    fileCount: toInt(value.fileCount),
    added: toInt(value.added),
    removed: toInt(value.removed),
    lastNodeId: value.lastNodeId === undefined || value.lastNodeId === null ? null : toInt(value.lastNodeId),
    lastAt: value.lastAt ? String(value.lastAt) : null,
  };
}

export function normalizeProject(raw: unknown): PfProject {
  const value = (raw ?? {}) as Record<string, unknown>;
  const root = String(value.root ?? '');
  const rootInfoRaw = value.rootInfo as Record<string, unknown> | undefined;
  const progressRaw = value.progress as Record<string, unknown> | undefined;
  const statsRaw = value.stats as Record<string, unknown> | undefined;
  const progress =
    progressRaw && typeof progressRaw === 'object'
      ? {
          done: toInt(progressRaw.done),
          total: toInt(progressRaw.total),
          text: String(progressRaw.text ?? ''),
        }
      : undefined;
  const maidsRaw = value.maids;
  const project: PfProject = {
    id: String(value.id ?? ''),
    name: String(value.name ?? ''),
    workspace_id: value.workspace_id ? String(value.workspace_id) : undefined,
    workspace_alias: value.workspace_alias ? String(value.workspace_alias) : undefined,
    root,
    subpath: value.subpath === undefined || value.subpath === null ? '' : String(value.subpath),
    status: String(value.status ?? 'active'),
    report: typeof value.report === 'string' && value.report.trim() ? value.report : null,
    created_by: value.created_by === undefined || value.created_by === null ? null : String(value.created_by),
    report_by: value.report_by === undefined || value.report_by === null ? null : String(value.report_by),
    created_at: value.created_at ? String(value.created_at) : '',
    updated_at: value.updated_at ? String(value.updated_at) : '',
    deleted_at: value.deleted_at ? String(value.deleted_at) : null,
    deleted_by: value.deleted_by ? String(value.deleted_by) : null,
    hasReport: value.hasReport === true,
    maids: Array.isArray(maidsRaw) ? maidsRaw.map((item) => String(item ?? '')).filter(Boolean) : undefined,
  };
  if (rootInfoRaw && typeof rootInfoRaw === 'object') {
    project.rootInfo = {
      root: rootInfoRaw.root ? String(rootInfoRaw.root) : root,
      workspaceAlias: rootInfoRaw.workspaceAlias
        ? String(rootInfoRaw.workspaceAlias)
        : project.workspace_alias,
      writable: rootInfoRaw.writable !== false,
      blockedReason: rootInfoRaw.blockedReason ? String(rootInfoRaw.blockedReason) : undefined,
    };
  }
  if (progress) project.progress = progress;
  if (statsRaw && typeof statsRaw === 'object') project.stats = normalizeStats(statsRaw);
  if (project.report) project.hasReport = true;
  return project;
}

export function normalizeTodo(raw: unknown): PfTodo {
  const value = (raw ?? {}) as Record<string, unknown>;
  return {
    id: value.id === undefined || value.id === null ? undefined : toInt(value.id),
    project_id: value.project_id ? String(value.project_id) : undefined,
    seq: toInt(value.seq),
    title: String(value.title ?? ''),
    status: String(value.status ?? 'pending'),
    note: trimmedOrNull(value.note),
    updated_by: value.updated_by === undefined || value.updated_by === null ? null : String(value.updated_by),
    updated_at: value.updated_at ? String(value.updated_at) : null,
  };
}

export function normalizeHistoryRow(raw: unknown): PfHistoryRow {
  const value = (raw ?? {}) as Record<string, unknown>;
  return {
    id: toInt(value.id),
    batch_id: toInt(value.batch_id),
    batch_kind: value.batch_kind ? String(value.batch_kind) : undefined,
    op: String(value.op ?? ''),
    project_id: value.project_id ? String(value.project_id) : undefined,
    file_path: String(value.file_path ?? ''),
    maid: value.maid === undefined || value.maid === null || value.maid === '' ? null : String(value.maid),
    effective_reason: trimmedOrNull(value.effective_reason) ?? undefined,
    reason: trimmedOrNull(value.reason) ?? undefined,
    summary: trimmedOrNull(value.summary) ?? undefined,
    added: toInt(value.added),
    removed: toInt(value.removed),
    created_at: String(value.created_at ?? ''),
    todo_id: value.todo_id === undefined || value.todo_id === null ? null : toInt(value.todo_id),
    after_hash: value.after_hash === undefined || value.after_hash === null ? null : String(value.after_hash),
  };
}

function validRows(nodes: unknown[]): PfHistoryRow[] {
  const rows: PfHistoryRow[] = [];
  for (const node of Array.isArray(nodes) ? nodes : []) {
    const row = normalizeHistoryRow(node);
    // 脏节点（缺 batch_id，通常缺少有效数据库记录）不参与聚合
    if (row.batch_id <= 0) continue;
    rows.push(row);
  }
  return rows;
}

function reasonOf(row: PfHistoryRow): string {
  return String(row.effective_reason || row.reason || row.summary || '');
}

/** SearchHistory 节点（每个文件一条）按 batch_id 聚合为开发脉络，倒序。 */
export function groupTimeline(nodes: unknown[], limit = TIMELINE_BATCH_LIMIT): PfBatchSummary[] {
  const rows = validRows(nodes).sort((a, b) => b.batch_id - a.batch_id || b.id - a.id);
  const grouped = new Map<number, PfBatchSummary>();
  for (const row of rows) {
    let item = grouped.get(row.batch_id);
    if (!item) {
      item = {
        id: row.batch_id,
        kind: row.batch_kind || row.op || 'edit',
        maid: row.maid ?? null,
        reason: reasonOf(row),
        created_at: row.created_at,
        node_count: 0,
        added: 0,
        removed: 0,
        files: [],
        batchId: row.batch_id,
        batchKey: `b${row.batch_id}`,
        at: row.created_at,
      };
      grouped.set(row.batch_id, item);
    }
    item.node_count += 1;
    item.added += row.added;
    item.removed += row.removed;
    if (!item.maid && row.maid) item.maid = row.maid;
    if (!item.reason) item.reason = reasonOf(row);
    if (!item.created_at) item.created_at = row.created_at;
    if (row.file_path && !item.files.includes(row.file_path)) item.files.push(row.file_path);
  }
  const list = [...grouped.values()].sort((a, b) => b.id - a.id);
  return limit > 0 ? list.slice(0, limit) : list;
}

/** 按 file_path 聚合改动文件（最近改动优先）。 */
export function aggregateFiles(nodes: unknown[]): PfFileChange[] {
  const grouped = new Map<string, PfFileChange>();
  for (const row of validRows(nodes)) {
    if (!row.file_path) continue;
    const item = grouped.get(row.file_path) ?? {
      file_path: row.file_path,
      edits: 0,
      added: 0,
      removed: 0,
      last_node: 0,
    };
    item.edits += 1;
    item.added += row.added;
    item.removed += row.removed;
    item.last_node = Math.max(item.last_node, row.id);
    grouped.set(row.file_path, item);
  }
  return [...grouped.values()].sort((a, b) => b.last_node - a.last_node);
}

/** 按 maid 聚合施工角色（外部/未署名归入 maid=null；批次数按 batch_id 去重）。 */
export function aggregateContributors(nodes: unknown[]): PfContributor[] {
  const grouped = new Map<string, PfContributor & { batchIds: Set<number> }>();
  for (const row of validRows(nodes)) {
    const key = row.maid ?? '';
    const item = grouped.get(key) ?? {
      maid: row.maid ?? null,
      batches: 0,
      added: 0,
      removed: 0,
      last_at: '',
      batchIds: new Set<number>(),
    };
    item.batchIds.add(row.batch_id);
    item.added += row.added;
    item.removed += row.removed;
    if (row.created_at && row.created_at > item.last_at) item.last_at = row.created_at;
    grouped.set(key, item);
  }
  return [...grouped.values()]
    .map((item) => ({
      maid: item.maid,
      batches: item.batchIds.size,
      added: item.added,
      removed: item.removed,
      last_at: item.last_at,
    }))
    .sort((a, b) => b.batches - a.batches || String(b.last_at).localeCompare(String(a.last_at)));
}

/** 用历史行兜底计算工程统计（GetProject.details.stats 缺失时）。 */
export function statsFromHistory(nodes: unknown[]): PfStats {
  const rows = validRows(nodes);
  let added = 0;
  let removed = 0;
  let lastNodeId = 0;
  let lastAt: string | null = null;
  const files = new Set<string>();
  for (const row of rows) {
    added += row.added;
    removed += row.removed;
    lastNodeId = Math.max(lastNodeId, row.id);
    if (row.created_at && (!lastAt || row.created_at > lastAt)) lastAt = row.created_at;
    if (row.file_path) files.add(row.file_path);
  }
  return { nodeCount: rows.length, fileCount: files.size, added, removed, lastNodeId: lastNodeId || null, lastAt };
}

/** GetProject.stats 优先，异常/缺失时退回历史聚合并补齐字段。 */
function mergeStats(primary: unknown, nodes: unknown[]): PfStats {
  const fallback = statsFromHistory(nodes);
  const raw = (primary ?? null) as Record<string, unknown> | null;
  if (!raw || typeof raw !== 'object') return fallback;
  const stats = normalizeStats(raw);
  if (stats.nodeCount === 0 && fallback.nodeCount > 0) return fallback;
  return {
    nodeCount: stats.nodeCount,
    fileCount: stats.fileCount || fallback.fileCount,
    added: stats.added || fallback.added,
    removed: stats.removed || fallback.removed,
    lastNodeId: stats.lastNodeId ?? fallback.lastNodeId ?? null,
    lastAt: stats.lastAt ?? fallback.lastAt ?? null,
  };
}

function splitTodoTitles(input: string): string[] {
  return String(input ?? '')
    .split(/[\n,，;；]+/)
    .map((item) => item.trim())
    .filter(Boolean);
}

function firstTitleLine(text: string): string {
  for (const line of String(text ?? '').split(/\r?\n/)) {
    const trimmed = line.trim();
    if (!trimmed) continue;
    return trimmed.replace(/^#+\s*/, '').trim();
  }
  return '';
}

// ============================ 引用格式化（数字 / nX / bX 都接受） ============================

/** 节点引用 → `n12`；接受 12 / '12' / 'n12' / 'N12'。 */
export function normalizeNodeRef(value: number | string): string {
  const raw = String(value ?? '').trim();
  const num = Number(raw.replace(/^[nN]\s*/, ''));
  if (!Number.isInteger(num) || num < 1) {
    throw new ProjectForgeError('validation', `节点引用无效：“${raw}”，应形如 n12（或数字 12）`);
  }
  return `n${num}`;
}

/** 批次引用 → `b12`；接受 12 / '12' / 'b12' / 'last'（撤销最近一次）。 */
export function normalizeBatchRef(value: number | string): string {
  const raw = String(value ?? '').trim();
  if (raw.toLowerCase() === 'last') return 'last';
  const num = Number(raw.replace(/^[bB]\s*/, ''));
  if (!Number.isInteger(num) || num < 1) {
    throw new ProjectForgeError('validation', `批次引用无效：“${raw}”，应形如 b12（或数字 12，或 last）`);
  }
  return `b${num}`;
}

function normalizeProjectIds(input: string | string[] | null | undefined): string[] {
  const list = Array.isArray(input) ? input : String(input ?? '').split(/[\r\n,，;；]+/);
  return [...new Set(list.map((item) => String(item ?? '').trim()).filter(Boolean))];
}

// ============================ Store ============================

type TodoExpectation =
  | { kind: 'status'; seq: number; status: string }
  | { kind: 'added'; titles: string[] }
  | { kind: 'fields'; seq: number; fields: Record<string, unknown> }
  | { kind: 'removed'; seq: number }
  | null;

type ReadCommandResult<T> = { details: T; text: string };

export const useProjectForgeStore = defineStore('projectForge', () => {
  refreshBridgeConfig();

  // —— 桥接设置（统一由 core/bridge/bridgeConfig 管理，三个功能页面共享） ——
  const baseUrl = computed(() => bridgeBaseUrl.value);
  const token = computed(() => bridgeToken.value);
  const configSource = computed(() => bridgeConfigSource.value);

  // —— 署名（回退 / 写操作记录用；独立 localStorage key） ——
  const signature = ref(readStoredSignature());
  const hasSignature = computed(() => signature.value.trim().length > 0);

  // —— 列表 / 工作区 ——
  const workspaces = ref<PfWorkspace[]>([]);
  const projects = ref<PfProject[]>([]);
  const includeDeleted = ref(false);
  const projectQuery = ref('');
  /** 旧版页面双向绑定的搜索框（与 projectQuery 同一份状态）。 */
  const query = computed({
    get: () => projectQuery.value,
    set: (value: string) => {
      projectQuery.value = String(value ?? '');
    },
  });
  const workspaceFilter = ref('all');
  const projectsLoadedAt = ref(0);

  // —— 详情 / 历史 ——
  const detail = ref<PfProjectDetail | null>(null);
  const currentProjectId = ref<string | null>(null);
  const history = ref<PfHistoryRow[]>([]);
  const detailCache = new Map<string, PfProjectDetail>();
  const batchNodesCache = new Map<string, PfHistoryRow[]>();

  // —— 加载态 ——
  const workspacesLoading = ref(false);
  const listLoading = ref(false);
  const detailLoading = ref(false);
  const historyLoading = ref(false);
  const mutating = ref(false);

  const loading = computed(() => ({
    workspaces: workspacesLoading.value,
    projects: listLoading.value,
    detail: detailLoading.value,
    history: historyLoading.value,
    action: mutating.value,
  }));

  // —— 错误态 ——
  const errorMessage = ref('');
  const errorKind = ref<PfErrorKind | ''>('');
  const listError = ref('');
  const listErrorKind = ref<PfErrorKind | ''>('');
  const detailError = ref('');
  const detailErrorKind = ref<PfErrorKind | ''>('');

  // —— 连接态 ——
  const connectionState = ref<PfConnectionStatus>('unknown');
  const connectionMessage = ref('');
  const isConnected = computed(() => connectionState.value === 'ok');
  const connection = computed(() => ({
    ok: connectionState.value === 'ok',
    message: connectionMessage.value,
    /** 连接状态机：unknown / connecting / ok / fail。 */
    state: connectionState.value,
    /** 兼容别名（与 state 相同）。 */
    status: connectionState.value,
  }));

  // —— 派生 ——
  const enabledWorkspaces = computed(() => workspaces.value.filter((item) => item.enabled !== false));
  const currentProject = computed<PfProject | null>(() => {
    const loaded = detail.value?.project;
    if (loaded && (!currentProjectId.value || loaded.id === currentProjectId.value)) return loaded;
    return projects.value.find((item) => item.id === currentProjectId.value) ?? null;
  });
  const todoProgress = computed(() => {
    const todos = detail.value?.todos || [];
    const total = todos.length;
    const done = todos.filter((todo) => todo.status === 'done').length;
    const doing = todos.filter((todo) => todo.status === 'doing').length;
    return { done, total, doing, percent: total ? Math.round((done / total) * 100) : 0 };
  });

  // —— 并发防重：同一类读请求合并为一个在途 Promise ——
  const inflight = new Map<string, Promise<unknown>>();
  function singleFlight<T>(key: string, task: () => Promise<T>): Promise<T> {
    const running = inflight.get(key) as Promise<T> | undefined;
    if (running) return running;
    const promise = task().finally(() => {
      if (inflight.get(key) === promise) inflight.delete(key);
    });
    inflight.set(key, promise);
    return promise;
  }

  // —— 错误记录 ——
  function recordError(input: unknown): ProjectForgeError {
    const error = asProjectForgeError(input);
    errorMessage.value = error.message;
    errorKind.value = error.kind;
    return error;
  }

  function clearError(): void {
    errorMessage.value = '';
    errorKind.value = '';
  }

  function markConnectionFailure(error: ProjectForgeError): void {
    if (error.kind === 'network' || error.kind === 'auth') {
      connectionState.value = 'fail';
      connectionMessage.value = error.message;
    }
  }

  function markConnectionOk(): void {
    connectionState.value = 'ok';
    const count = workspaces.value.length;
    connectionMessage.value = count ? `已连接 · ${count} 个工作区` : '已连接';
  }

  function requireProjectId(input?: string | null): string {
    const id = String(input ?? currentProjectId.value ?? '').trim();
    if (!id) throw new ProjectForgeError('validation', '缺少工程 ID：请先打开工程详情');
    return id;
  }

  function requireCurrentProjectId(): string {
    const id = String(currentProjectId.value ?? detail.value?.project.id ?? '').trim();
    if (!id) throw new ProjectForgeError('validation', '请先打开工程详情');
    return id;
  }

  /** 写操作串行：同一时间只允许一个写命令，防止连点造成重复提交。 */
  async function runMutation<T>(task: () => Promise<T>): Promise<T> {
    if (mutating.value) {
      throw new ProjectForgeError('validation', '上一个操作尚未完成，请稍候');
    }
    mutating.value = true;
    try {
      return await task();
    } catch (input) {
      throw recordError(input);
    } finally {
      mutating.value = false;
    }
  }

  /** 读操作统一错误记录（rethrow，调用方决定是否 toast）。 */
  async function track<T>(task: () => Promise<T>): Promise<T> {
    try {
      return await task();
    } catch (input) {
      throw recordError(input);
    }
  }

  function notify(message: string, type: 'info' | 'success' | 'warning' | 'error' = 'info'): void {
    const text = String(message ?? '').trim();
    if (!text) return;
    try {
      const notificationStore = useNotificationStore();
      notificationStore.addNotification({
        type,
        title: 'V工程',
        message: text,
        duration: type === 'error' ? 4200 : 2400,
        toastOnly: true,
      });
    } catch {
      // Pinia 未激活等场景静默忽略：toast 永远不能阻断主流程
    }
  }

  /** 桥接端点变化后，旧缓存（工程列表/详情/工作区/连接状态）不再可信。 */
  function resetBridgeCaches(): void {
    projects.value = [];
    projectsLoadedAt.value = 0;
    detail.value = null;
    detailCache.clear();
    batchNodesCache.clear();
    workspaces.value = [];
    history.value = [];
    currentProjectId.value = null;
    connectionState.value = 'unknown';
    connectionMessage.value = '';
    listError.value = '';
    listErrorKind.value = '';
    detailError.value = '';
    detailErrorKind.value = '';
    clearError();
    inflight.clear();
  }

  // 其它功能页（远程桌面 / 文坊）或同步设置改动桥接地址时，本页立即失效缓存
  watch([bridgeBaseUrl, bridgeToken], resetBridgeCaches);

  function saveSettings(next: PfBridgeSettings): void {
    saveBridgeConfig({ baseUrl: String(next?.baseUrl ?? ''), token: String(next?.token ?? '') });
    resetBridgeCaches();
  }

  /** 恢复默认：清掉本机显式保存的桥接配置（含旧 key 镜像），重新按同步设置/构建默认值推导。 */
  function resetSettings(): void {
    try {
      localStorage.removeItem(BRIDGE_STORAGE_KEY);
      for (const key of LEGACY_STORAGE_KEYS) localStorage.removeItem(key);
    } catch {
      // localStorage 不可用时忽略
    }
    refreshBridgeConfig();
    resetBridgeCaches();
  }

  function setSignature(value: string): string {
    const next = String(value ?? '').trim().slice(0, 100);
    signature.value = next;
    try {
      if (next) localStorage.setItem(SIGNATURE_STORAGE_KEY, next);
      else localStorage.removeItem(SIGNATURE_STORAGE_KEY);
    } catch {
      // ignore
    }
    return next;
  }

  function syncProjectRow(project: PfProject): void {
    const row = projects.value.find((item) => item.id === project.id);
    if (!row) return;
    row.name = project.name;
    row.status = project.status;
    row.updated_at = project.updated_at || row.updated_at;
    row.report = project.report ?? row.report;
    row.hasReport = project.hasReport || row.hasReport;
    row.todoDone = project.todoDone ?? row.todoDone;
    row.todoTotal = project.todoTotal ?? row.todoTotal;
  }

  function extractHistoryRows(envelope: PfBridgeEnvelope): PfHistoryRow[] {
    const details = (envelope.result?.details ?? null) as Record<string, unknown> | null;
    const nodes = Array.isArray(details?.nodes) ? (details?.nodes as unknown[]) : [];
    return nodes.map(normalizeHistoryRow);
  }

  // ============================ 工作区 ============================

  async function loadWorkspaces(): Promise<PfWorkspace[]> {
    return singleFlight('workspaces', async () => {
      workspacesLoading.value = true;
      listError.value = '';
      listErrorKind.value = '';
      try {
        const details = await pfInvoke<Record<string, unknown>>('ListWorkspaces');
        const raw = Array.isArray(details?.workspaces)
          ? (details.workspaces as unknown[])
          : Array.isArray(details)
            ? (details as unknown[])
            : [];
        workspaces.value = raw.map(normalizeWorkspace);
        clearError();
        markConnectionOk();
        return workspaces.value;
      } catch (input) {
        const error = asProjectForgeError(input);
        listError.value = error.message;
        listErrorKind.value = error.kind;
        recordError(error);
        markConnectionFailure(error);
        throw error;
      } finally {
        workspacesLoading.value = false;
      }
    });
  }

  // ============================ 工程列表 ============================

  /** 列表查询条件 → ListProjects 参数。 */
  function projectListArgs(): Record<string, unknown> {
    const args: Record<string, unknown> = {
      workspace: workspaceFilter.value || 'all',
      limit: LIST_LIMIT,
      includeDeleted: includeDeleted.value,
    };
    const keyword = projectQuery.value.trim();
    if (keyword) args.query = keyword;
    return args;
  }

  async function loadProjects(options: { includeDeleted?: boolean; silent?: boolean } = {}): Promise<PfProject[]> {
    if (typeof options.includeDeleted === 'boolean') includeDeleted.value = options.includeDeleted;
    const args = projectListArgs();
    return singleFlight(`projects:${JSON.stringify(args)}`, async () => {
      listLoading.value = true;
      if (!options.silent) {
        listError.value = '';
        listErrorKind.value = '';
        clearError();
      }
      try {
        const envelope = await pfInvokeEnvelope('ListProjects', args);
        const details = (envelope.result?.details ?? {}) as Record<string, unknown>;
        const raw = Array.isArray(details?.projects) ? (details.projects as unknown[]) : [];
        const progress = parseProjectProgress(bridgeContentText(envelope));
        projects.value = raw.map((item) => {
          const project = normalizeProject(item);
          const parsed = progress.get(project.id);
          if (parsed) {
            project.todoDone = parsed.done;
            project.todoTotal = parsed.total;
            project.hasReport = project.hasReport || parsed.hasReport;
          } else if (project.progress) {
            project.todoDone = project.progress.done;
            project.todoTotal = project.progress.total;
          }
          if (project.stats) project.hasReport = project.hasReport || !!project.report;
          return project;
        });
        projectsLoadedAt.value = Date.now();
        markConnectionOk();
        return projects.value;
      } catch (input) {
        const error = asProjectForgeError(input);
        listError.value = error.message;
        listErrorKind.value = error.kind;
        recordError(error);
        markConnectionFailure(error);
        throw error;
      } finally {
        listLoading.value = false;
      }
    });
  }

  /** 当前搜索工作区：优先工作区筛选，其次当前工程所属工作区，最后唯一启用工作区。 */
  function resolveSearchWorkspace(): string {
    const filter = String(workspaceFilter.value || '').trim();
    if (filter && filter.toLowerCase() !== 'all') return filter;
    const project = currentProject.value;
    if (project?.workspace_alias) return project.workspace_alias;
    if (project?.workspace_id) return project.workspace_id;
    if (enabledWorkspaces.value.length === 1) return enabledWorkspaces.value[0].alias;
    return '';
  }

  /**
   * 搜索工程：有明确工作区时用 `SearchProjects`（桌面要求 workspace 必填且不能为 all）；
   * 没有可用工作区时退回 `ListProjects` 的全工作区 query 搜索。
   */
  async function searchProjects(
    keyword = projectQuery.value,
    options: { workspace?: string; limit?: number; includeDeleted?: boolean } = {},
  ): Promise<PfProject[]> {
    const queryText = String(keyword ?? '').trim();
    if (!queryText) throw new ProjectForgeError('validation', '请输入工程名称关键字或完整工程 ID');
    const workspace = String(options.workspace ?? resolveSearchWorkspace()).trim();
    const useSearchCommand = Boolean(workspace && workspace.toLowerCase() !== 'all');
    const args: Record<string, unknown> = {
      query: queryText,
      limit: options.limit ?? LIST_LIMIT,
      includeDeleted: options.includeDeleted ?? includeDeleted.value,
    };
    if (useSearchCommand) args.workspace = workspace;
    else args.workspace = 'all';
    return track(async () => {
      const envelope = await pfInvokeEnvelope(useSearchCommand ? 'SearchProjects' : 'ListProjects', args);
      const details = (envelope.result?.details ?? {}) as Record<string, unknown>;
      const raw = Array.isArray(details?.projects) ? (details.projects as unknown[]) : [];
      const progress = parseProjectProgress(bridgeContentText(envelope));
      const list = raw.map((item) => {
        const project = normalizeProject(item);
        const parsed = progress.get(project.id);
        if (parsed) {
          project.todoDone = parsed.done;
          project.todoTotal = parsed.total;
          project.hasReport = project.hasReport || parsed.hasReport;
        }
        return project;
      });
      projects.value = list;
      projectsLoadedAt.value = Date.now();
      clearError();
      return list;
    });
  }

  // ============================ 工程详情（GetProject + SearchHistory 聚合） ============================

  async function openProject(projectId: string, options: { force?: boolean } = {}): Promise<PfProjectDetail> {
    const id = String(projectId ?? '').trim();
    if (!id) throw new ProjectForgeError('validation', '缺少工程 ID');
    const cached = detailCache.get(id) ?? null;
    // 缓存先行展示（force 刷新时保留当前画面，避免闪白）；切换工程且无缓存时清空，避免串数据
    if (cached) {
      if (!options.force) detail.value = cached;
    } else {
      detail.value = null;
    }
    currentProjectId.value = id;
    return singleFlight(`detail:${id}:${options.force ? 'force' : 'auto'}`, async () => {
      detailLoading.value = true;
      detailError.value = '';
      detailErrorKind.value = '';
      try {
        const details = await pfInvoke<Record<string, unknown>>('GetProject', { projectId: id, timeline: 8 });
        const project = normalizeProject(details?.project ?? {});
        const todos = Array.isArray(details?.todos) ? (details.todos as unknown[]).map(normalizeTodo) : [];
        project.todoDone = todos.filter((todo) => todo.status === 'done').length;
        project.todoTotal = todos.length;
        project.hasReport = project.hasReport || !!project.report;

        // 开发脉络 / 改动文件 / 施工角色：GetProject 不给，用 SearchHistory 聚合。
        // SearchHistory 失败不影响详情主体（timeline/files/contributors 留空）。
        let nodes: PfHistoryRow[] = [];
        try {
          const historyEnvelope = await pfInvokeEnvelope('SearchHistory', {
            projectId: id,
            limit: DETAIL_HISTORY_LIMIT,
          });
          nodes = extractHistoryRows(historyEnvelope);
        } catch {
          nodes = [];
        }

        const snapshot: PfProjectDetail = {
          project,
          todos,
          stats: mergeStats(details?.stats, nodes),
          timeline: groupTimeline(nodes, DETAIL_TIMELINE_LIMIT),
          files: aggregateFiles(nodes),
          contributors: aggregateContributors(nodes),
          loadedAt: Date.now(),
        };
        detailCache.set(id, snapshot);
        detail.value = snapshot;
        currentProjectId.value = id;
        syncProjectRow(project);
        clearError();
        markConnectionOk();
        return snapshot;
      } catch (input) {
        const error = asProjectForgeError(input);
        detailError.value = error.message;
        detailErrorKind.value = error.kind;
        recordError(error);
        markConnectionFailure(error);
        if (!cached) detail.value = null;
        throw error;
      } finally {
        detailLoading.value = false;
      }
    });
  }

  function closeProject(): void {
    detail.value = null;
    currentProjectId.value = null;
    detailError.value = '';
    detailErrorKind.value = '';
  }

  /** 写操作成功后刷新列表与当前详情（静默失败，不影响主操作结果）。 */
  async function refreshProjectViews(id?: string | null, options: { list?: boolean; detail?: boolean } = {}): Promise<void> {
    const tasks: Promise<unknown>[] = [];
    if (options.list !== false) tasks.push(loadProjects({ silent: true }));
    const target = String(id ?? currentProjectId.value ?? '').trim();
    if (options.detail !== false && target) tasks.push(openProject(target, { force: true }));
    await Promise.allSettled(tasks);
  }

  /** 首次进入页面：拉工作区 + 工程列表（列表 60 秒内复用缓存，避免每次打开都打桥接）。 */
  async function bootstrap(options: { force?: boolean } = {}): Promise<void> {
    const stale = !projects.value.length || Date.now() - projectsLoadedAt.value > LIST_CACHE_TTL;
    const tasks: Promise<unknown>[] = [loadWorkspaces()];
    if (options.force || stale) tasks.push(loadProjects());
    await Promise.allSettled(tasks);
  }

  /** 顶栏刷新：工作区 + 工程列表 +（如果打开了工程）强制重读详情。 */
  async function refreshAll(): Promise<void> {
    const tasks: Promise<unknown>[] = [loadWorkspaces(), loadProjects({ silent: true })];
    const id = currentProjectId.value;
    if (id) tasks.push(openProject(id, { force: true }));
    const results = await Promise.allSettled(tasks);
    const failed = results.find((item) => item.status === 'rejected');
    if (failed && failed.status === 'rejected') throw failed.reason;
    clearError();
  }

  /** 设置面板「测试连接」：探一次 ListWorkspaces，成功与否都写回连接状态。 */
  async function testConnection(): Promise<boolean> {
    connectionState.value = 'connecting';
    connectionMessage.value = '正在连接桌面端…';
    try {
      await loadWorkspaces();
      return true;
    } catch {
      return false;
    }
  }

  // ============================ 工程创建 / 删除 / 恢复 / 清除 ============================

  async function createProject(input: {
    name: string;
    workspace?: string;
    dir?: string;
    todos?: string;
  }): Promise<PfProject> {
    const name = String(input?.name ?? '').trim();
    if (!name) throw new ProjectForgeError('validation', '请填写工程名称');
    const args: Record<string, unknown> = { name };
    const workspace = String(input?.workspace ?? '').trim();
    if (workspace) args.workspace = workspace;
    const dir = String(input?.dir ?? '').trim();
    if (dir) args.dir = dir;
    const todos = splitTodoTitles(String(input?.todos ?? ''));
    if (todos.length) args.todos = todos.join('\n');
    const maid = signature.value;
    if (maid) args.maid = maid;
    return runMutation(async () => {
      const details = await pfInvoke<Record<string, unknown>>('CreateProject', args);
      const project = normalizeProject(details?.project ?? {});
      if (!project.id) throw new ProjectForgeError('protocol', '桌面端返回成功但缺少工程 ID');
      detailCache.delete(project.id);
      projectsLoadedAt.value = 0;
      await loadProjects({ silent: true }).catch(() => undefined);
      await openProject(project.id);
      return project;
    });
  }

  async function deleteProjects(projectIds: string | string[]): Promise<PfDeleteResult> {
    const ids = normalizeProjectIds(projectIds);
    if (!ids.length) throw new ProjectForgeError('validation', '请选择要删除的工程');
    const payload: Record<string, unknown> = { projectIds: ids.join(',') };
    if (signature.value) payload.maid = signature.value;
    return runMutation(async () => {
      const details = await pfInvoke<Record<string, unknown>>('DeleteProjects', payload);
      const deleted = Array.isArray(details?.deleted) ? (details.deleted as unknown[]).map(String) : ids;
      const skipped = Array.isArray(details?.skipped) ? (details.skipped as unknown[]).map(String) : [];
      const currentId = currentProjectId.value;
      if (currentId && deleted.includes(currentId)) {
        if (includeDeleted.value) {
          await openProject(currentId, { force: true }).catch(() => undefined);
        } else {
          closeProject();
        }
      }
      await loadProjects({ silent: true }).catch(() => undefined);
      return { deleted, skipped };
    });
  }

  async function restoreProjects(projectIds: string | string[]): Promise<PfRestoreResult> {
    const ids = normalizeProjectIds(projectIds);
    if (!ids.length) throw new ProjectForgeError('validation', '请选择要恢复的工程');
    const payload: Record<string, unknown> = { projectIds: ids.join(',') };
    if (signature.value) payload.maid = signature.value;
    return runMutation(async () => {
      const details = await pfInvoke<Record<string, unknown>>('RestoreProjects', payload);
      const restored = Array.isArray(details?.restored) ? (details.restored as unknown[]).map(String) : [];
      const currentId = currentProjectId.value;
      if (currentId && restored.includes(currentId)) {
        await openProject(currentId, { force: true }).catch(() => undefined);
      }
      await loadProjects({ silent: true }).catch(() => undefined);
      return { restored };
    });
  }

  /** 永久清除（只针对已软删工程；磁盘文件不受影响）。 */
  async function purgeProjects(projectIds: string | string[]): Promise<PfPurgeResult> {
    const ids = normalizeProjectIds(projectIds);
    if (!ids.length) throw new ProjectForgeError('validation', '请选择要永久清除的工程');
    const payload: Record<string, unknown> = { projectIds: ids.join(','), confirm: true };
    if (signature.value) payload.maid = signature.value;
    return runMutation(async () => {
      const details = await pfInvoke<Record<string, unknown>>('PurgeProjects', payload);
      const purged = Array.isArray(details?.purged) ? (details.purged as unknown[]).map(String) : [];
      for (const id of purged) {
        detailCache.delete(id);
        for (const key of [...batchNodesCache.keys()]) {
          if (key.includes(`:${id}:`)) batchNodesCache.delete(key);
        }
        if (currentProjectId.value === id) closeProject();
      }
      await loadProjects({ silent: true }).catch(() => undefined);
      return { purged, freedBlobs: toInt(details?.freedBlobs) };
    });
  }

  // ============================ Todo 编辑（写后回读确认） ============================

  function verifyTodoExpectation(latest: PfProjectDetail, expectation: TodoExpectation): void {
    if (!expectation) return;
    if (expectation.kind === 'status') {
      const todo = latest.todos.find((item) => item.seq === expectation.seq);
      if (!todo || todo.status !== expectation.status) {
        throw new ProjectForgeError(
          'protocol',
          `回读校验失败：待办 #${expectation.seq} 未变更为「${todoStatusLabel(expectation.status)}」`,
        );
      }
      return;
    }
    if (expectation.kind === 'added') {
      for (const title of expectation.titles) {
        if (!latest.todos.some((item) => item.title === title)) {
          throw new ProjectForgeError('protocol', `回读校验失败：新增待办「${title}」未出现在工程中`);
        }
      }
      return;
    }
    if (expectation.kind === 'fields') {
      const todo = latest.todos.find((item) => item.seq === expectation.seq);
      if (!todo) throw new ProjectForgeError('protocol', `回读校验失败：待办 #${expectation.seq} 不存在`);
      for (const [key, value] of Object.entries(expectation.fields)) {
        const actual = (todo as unknown as Record<string, unknown>)[key];
        const expected = value === null || value === undefined ? null : String(value);
        const normalizedActual = actual === null || actual === undefined ? null : String(actual);
        if (expected !== normalizedActual) {
          throw new ProjectForgeError('protocol', `回读校验失败：待办 #${expectation.seq} 的 ${key} 未更新`);
        }
      }
      return;
    }
    if (expectation.kind === 'removed') {
      if (latest.todos.some((item) => item.seq === expectation.seq)) {
        throw new ProjectForgeError('protocol', `回读校验失败：待办 #${expectation.seq} 未被删除`);
      }
    }
  }

  async function applyTodos(
    patch: Record<string, unknown>,
    expectation: TodoExpectation = null,
  ): Promise<PfProjectDetail> {
    const current = detail.value;
    if (!current) throw new ProjectForgeError('validation', '请先打开工程详情');
    const id = current.project.id;
    const keys = Object.keys(patch ?? {});
    if (!keys.length) throw new ProjectForgeError('validation', '没有要更新的待办内容');
    return runMutation(async () => {
      const payload: Record<string, unknown> = { projectId: id, ...patch };
      if (signature.value) payload.maid = signature.value;
      await pfInvoke<Record<string, unknown>>('UpdateTodos', payload);
      // 桌面端写入以数据库为准；强制回读一次，保证「await 结束时 detail 已是新数据」
      const latest = await openProject(id, { force: true });
      verifyTodoExpectation(latest, expectation);
      return latest;
    });
  }

  /** 通用 UpdateTodos：add / done / doing / pending / blocked / remove / seq+title/note。 */
  function updateTodos(patch: Record<string, unknown>): Promise<PfProjectDetail> {
    return applyTodos(patch, null);
  }

  function toggleTodo(todo: PfTodo): Promise<PfProjectDetail> {
    const nextDone = todo.status === 'done' ? 'pending' : 'done';
    return applyTodos(
      nextDone === 'done' ? { done: String(todo.seq) } : { pending: String(todo.seq) },
      { kind: 'status', seq: todo.seq, status: nextDone },
    );
  }

  function markTodoDoing(todo: PfTodo): Promise<PfProjectDetail> {
    return applyTodos({ doing: String(todo.seq) }, { kind: 'status', seq: todo.seq, status: 'doing' });
  }

  function setTodoStatus(seq: number, status: PfTodoStatus): Promise<PfProjectDetail> {
    if (!TODO_STATUSES.includes(status)) {
      throw new ProjectForgeError('validation', `待办状态无效：“${status}”，可用 pending / doing / done / blocked`);
    }
    const target = toInt(seq);
    if (target < 1) throw new ProjectForgeError('validation', '待办序号无效');
    return applyTodos({ [status]: String(target) }, { kind: 'status', seq: target, status });
  }

  async function addTodos(input: string): Promise<string[]> {
    const titles = splitTodoTitles(input);
    if (!titles.length) throw new ProjectForgeError('validation', '请输入待办内容');
    await applyTodos({ add: titles.join('\n') }, { kind: 'added', titles });
    return titles;
  }

  function removeTodo(seq: number): Promise<PfProjectDetail> {
    const target = toInt(seq);
    if (target < 1) throw new ProjectForgeError('validation', '待办序号无效');
    return applyTodos({ remove: String(target) }, { kind: 'removed', seq: target });
  }

  function updateTodo(seq: number, patch: { title?: string; note?: string | null }): Promise<PfProjectDetail> {
    const target = toInt(seq);
    if (target < 1) throw new ProjectForgeError('validation', '待办序号无效');
    const fields: Record<string, unknown> = {};
    if (patch?.title !== undefined) fields.title = String(patch.title);
    if (patch?.note !== undefined) fields.note = patch.note === null ? '' : String(patch.note);
    if (!Object.keys(fields).length) throw new ProjectForgeError('validation', '没有要修改的待办字段');
    return applyTodos({ seq: String(target), ...fields }, { kind: 'fields', seq: target, fields });
  }

  // ============================ 历史搜索 / 节点 diff ============================

  function historyArgs(filters: PfHistoryFilters, projectId?: string): Record<string, unknown> {
    const args: Record<string, unknown> = {};
    const id = String(projectId ?? filters?.projectId ?? '').trim();
    if (id) args.projectId = id;
    const keyword = String(filters?.keyword ?? '').trim();
    if (keyword) args.keyword = keyword;
    const content = String(filters?.content ?? '').trim();
    if (content) args.content = content;
    const file = String(filters?.file ?? '').trim();
    if (file) args.file = file;
    const byMaid = String(filters?.byMaid ?? '').trim();
    if (byMaid) args.byMaid = byMaid;
    const op = String(filters?.op ?? '').trim();
    if (op) args.op = op;
    if (filters?.todo !== undefined && filters?.todo !== null && String(filters.todo).trim() !== '') {
      args.todo = filters.todo;
    }
    if (filters?.batch !== undefined && filters?.batch !== null && String(filters.batch).trim() !== '') {
      args.batch = normalizeBatchRef(filters.batch);
    }
    const since = String(filters?.since ?? '').trim();
    if (since) args.since = since;
    const until = String(filters?.until ?? '').trim();
    if (until) args.until = until;
    args.limit = filters?.limit && Number(filters.limit) > 0 ? toInt(filters.limit) : 100;
    return args;
  }

  /** 搜索施工历史；`date`/`keyword` 等过滤条件原样透传 SearchHistory。返回 PfHistoryRow[]。 */
  async function searchHistory(
    filters: PfHistoryFilters = {},
    options: { projectId?: string } = {},
  ): Promise<PfHistoryRow[]> {
    const projectId = String(options?.projectId ?? currentProjectId.value ?? '').trim();
    const args = historyArgs(filters ?? {}, projectId || undefined);
    return singleFlight(`history:${JSON.stringify(args)}`, async () => {
      historyLoading.value = true;
      try {
        const envelope = await pfInvokeEnvelope('SearchHistory', args);
        const rows = extractHistoryRows(envelope);
        history.value = rows;
        clearError();
        return rows;
      } catch (input) {
        throw recordError(input);
      } finally {
        historyLoading.value = false;
      }
    });
  }

  /** 展开批次时读取该批次的节点明细（每个文件一条），带内存缓存。 */
  async function getBatchNodes(projectId: string, batchId: number | string): Promise<PfHistoryRow[]> {
    const id = requireProjectId(projectId);
    const batch = normalizeBatchRef(batchId);
    const key = `batch:${id}:${batch}`;
    const cached = batchNodesCache.get(key);
    if (cached) return cached;
    return singleFlight(key, async () => {
      const envelope = await pfInvokeEnvelope('SearchHistory', { projectId: id, batch, limit: 500 });
      const rows = extractHistoryRows(envelope);
      batchNodesCache.set(key, rows);
      clearError();
      return rows;
    });
  }

  /** 读取节点/批次的统一 diff（node 与 batch 二选一；数字或 nX/bX 均可）。 */
  async function getNodeDiff(projectId: string, selector: PfDiffSelector = {}): Promise<PfDiffResult> {
    const id = requireProjectId(projectId);
    const args: Record<string, unknown> = { projectId: id, maxLines: selector?.maxLines ?? 400 };
    const hasNode = selector?.node !== undefined && selector?.node !== null && String(selector.node).trim() !== '';
    const hasBatch = selector?.batch !== undefined && selector?.batch !== null && String(selector.batch).trim() !== '';
    if (hasNode) args.node = normalizeNodeRef(selector.node as number | string);
    else if (hasBatch) args.batch = normalizeBatchRef(selector.batch as number | string);
    else throw new ProjectForgeError('validation', 'GetNodeDiff 需要 node（nX）或 batch（bX）');
    return track(async () => {
      const envelope = await pfInvokeEnvelope('GetNodeDiff', args, { timeoutMs: 120_000 });
      const text = bridgeContentText(envelope).trim();
      const fallback = hasNode ? `节点 ${args.node}` : `批次 ${args.batch}`;
      const title = firstTitleLine(text) || `${fallback} · 改动`;
      return { title, text: text || '（该节点/批次没有可显示的 diff）' };
    });
  }

  /** 旧版兼容：按批次查看 diff。 */
  function getBatchDiff(projectId: string, batchId: number | string): Promise<PfDiffResult> {
    return getNodeDiff(projectId, { batch: batchId });
  }

  // ============================ 回退 ============================

  async function rollback(args: PfRollbackArgs): Promise<PfRollbackResult> {
    const id = requireProjectId(args?.projectId);
    const payload: Record<string, unknown> = { projectId: id };
    const hasBatch = args?.batch !== undefined && args?.batch !== null && String(args.batch).trim() !== '';
    const hasNode = args?.toNode !== undefined && args?.toNode !== null && String(args.toNode).trim() !== '';
    if (hasBatch) payload.batch = normalizeBatchRef(args.batch as number | string);
    if (hasNode) payload.toNode = normalizeNodeRef(args.toNode as number | string);
    if (!hasBatch && !hasNode) {
      throw new ProjectForgeError('validation', 'Rollback 需要 batch（bX / last）或 toNode（nX）');
    }
    const path = String(args?.path ?? '').trim();
    if (path) payload.path = path;
    if (args?.force) payload.force = true;
    if (args?.dryRun) payload.dryRun = true;
    const reason = String(args?.reason ?? '').trim();
    if (reason) payload.reason = reason;
    const maid = signature.value;
    if (maid) payload.maid = maid;
    return runMutation(async () => {
      const envelope = await pfInvokeEnvelope('Rollback', payload, { timeoutMs: 120_000 });
      const details = (envelope.result?.details ?? {}) as Record<string, unknown>;
      const text = bridgeContentText(envelope);
      const items: PfRollbackItem[] | undefined = Array.isArray(details?.items)
        ? (details.items as PfRollbackItem[])
        : undefined;
      const status = String(details?.status ?? (text.includes('无需回退') ? 'noop' : 'ok'));
      const result: PfRollbackResult = {
        status,
        batchId: details?.batchId === undefined ? undefined : toInt(details.batchId),
        count: details?.count === undefined ? undefined : toInt(details.count),
        items,
        maid: maid || undefined,
        text: text || undefined,
      };
      if (status === 'ok') await refreshProjectViews(id);
      return result;
    });
  }

  // ============================ 验收报告 ============================

  async function submitReport(input: PfSubmitReportInput): Promise<PfProjectDetail> {
    const id = requireCurrentProjectId();
    const conclusion = String(input?.conclusion ?? '').trim();
    if (!conclusion) throw new ProjectForgeError('validation', '请填写验收结论（conclusion）');
    const status = String(input?.status ?? 'review').trim().toLowerCase() || 'review';
    if (!['review', 'accepted', 'active'].includes(status)) {
      throw new ProjectForgeError('validation', 'status 只能是 review、accepted 或 active');
    }
    const payload: Record<string, unknown> = { projectId: id, conclusion, status };
    const issues = String(input?.issues ?? '').trim();
    if (issues) payload.issues = issues;
    if (signature.value) payload.maid = signature.value;
    return runMutation(async () => {
      await pfInvoke<Record<string, unknown>>('SubmitReport', payload);
      const latest = await openProject(id, { force: true });
      await loadProjects({ silent: true }).catch(() => undefined);
      return latest;
    });
  }

  // ============================ ReadCode / Outline / FindSymbol / Trace ============================

  type ReadCodeInput = string | PfReadCodeOptions | undefined;

  /** 只读查看工程文件（ReadCode）：正文取 content[1..]（带行号），details 提供 read/skipped/failed/symbols。 */
  async function readCode(projectId: string, path: string, options: ReadCodeInput = {}): Promise<PfReadCodeResult> {
    const id = requireProjectId(projectId);
    const target = String(path ?? '').trim();
    const opts: PfReadCodeOptions = typeof options === 'string' ? { lines: options } : options ?? {};
    const paths = Array.isArray(opts.paths) ? opts.paths.map((item) => String(item ?? '').trim()).filter(Boolean) : [];
    if (!target && !paths.length && !opts.glob) {
      throw new ProjectForgeError('validation', '请输入工程内文件路径（path / paths / glob 至少一个）');
    }
    const args: Record<string, unknown> = { projectId: id };
    if (target) args.path = target;
    if (paths.length) args.paths = paths.join('\n');
    if (opts.lines) args.lines = opts.lines;
    if (opts.glob) args.glob = opts.glob;
    if (opts.symbol) args.symbol = opts.symbol;
    if (opts.showLine !== undefined) args.showLine = opts.showLine;
    if (opts.find !== undefined) args.find = opts.find;
    if (opts.range) args.range = opts.range;
    if (opts.context !== undefined) args.context = opts.context;
    if (opts.pick !== undefined) args.pick = opts.pick;
    if (opts.line !== undefined) args.line = opts.line;
    return track(async () => {
      const envelope = await pfInvokeEnvelope('ReadCode', args, { timeoutMs: opts.timeoutMs ?? 120_000 });
      const parts = bridgeContentParts(envelope);
      // content[0] 是摘要头（标题/工程头/符号提示），content[1..] 才是文件正文
      const body = parts
        .slice(1)
        .map((part) => part.text)
        .filter((text) => text.trim() !== '')
        .join('\n\n');
      const details = (envelope.result?.details ?? {}) as Record<string, unknown>;
      return {
        title: target || paths[0] || String(opts.glob ?? '').trim(),
        text: body || bridgeContentText(envelope) || '（没有可显示的内容）',
        files: Array.isArray(details?.read) ? (details.read as PfReadCodeResult['files']) : [],
        skipped: Array.isArray(details?.skipped) ? (details.skipped as unknown[]) : [],
        failed: Array.isArray(details?.failed) ? (details.failed as unknown[]) : [],
        symbols: Array.isArray(details?.symbols) ? (details.symbols as PfReadCodeResult['symbols']) : [],
        mode: typeof details?.mode === 'string' ? details.mode : undefined,
      };
    });
  }

  /** 通用只读命令（Outline / FindSymbol / Trace 等）：同时返回 details 与 content Markdown。 */
  async function readProjectCommand<T extends Record<string, unknown>>(
    command: string,
    args: Record<string, unknown>,
  ): Promise<ReadCommandResult<T>> {
    return track(async () => {
      const envelope = await pfInvokeEnvelope(command, args, { timeoutMs: 120_000 });
      return {
        details: (envelope.result?.details ?? {}) as T,
        text: bridgeContentText(envelope),
      };
    });
  }

  /** 文件符号大纲（Outline）。paths 可为单文件或数组。 */
  async function outline(
    projectId: string,
    paths: string | string[],
    options: PfOutlineOptions = {},
  ): Promise<PfOutlineResult> {
    const id = requireProjectId(projectId);
    const list = (Array.isArray(paths) ? paths : [paths]).map((item) => String(item ?? '').trim()).filter(Boolean);
    if (!list.length) throw new ProjectForgeError('validation', 'Outline 需要 path 或 paths');
    const args: Record<string, unknown> = {
      projectId: id,
      paths: list.join('\n'),
    };
    if (options.depth !== undefined) args.depth = options.depth;
    if (options.kind) args.kind = options.kind;
    const { details, text } = await readProjectCommand<Record<string, unknown>>('Outline', args);
    return {
      files: Array.isArray(details?.files) ? (details.files as PfOutlineResult['files']) : [],
      text,
    };
  }

  /** 工程内按名称查找符号定义（FindSymbol，需要桌面 AST 索引器）。 */
  async function findSymbol(
    projectId: string,
    name: string,
    options: PfFindSymbolOptions = {},
  ): Promise<PfFindSymbolResult> {
    const symbolName = String(name ?? '').trim();
    if (!symbolName) throw new ProjectForgeError('validation', 'FindSymbol 需要符号名或限定名');
    const args: Record<string, unknown> = { projectId: requireProjectId(projectId), name: symbolName };
    if (options.kind) args.kind = options.kind;
    if (options.glob) args.glob = options.glob;
    if (options.exact !== undefined) args.exact = options.exact;
    if (options.limit !== undefined) args.limit = options.limit;
    if (options.scope) args.scope = options.scope;
    const { details, text } = await readProjectCommand<Record<string, unknown>>('FindSymbol', args);
    return {
      total: toInt(details?.total),
      scanned: details?.scanned === undefined ? undefined : toInt(details.scanned),
      parsed: details?.parsed === undefined ? undefined : toInt(details.parsed),
      truncated: details?.truncated === true,
      hits: Array.isArray(details?.hits) ? (details.hits as PfFindSymbolResult['hits']) : [],
      text,
    };
  }

  /** JS 前端 / Electron 工程链路地图（Trace）。target 形如 ipc:xx / global:xx / page:x.html / file:path。 */
  async function trace(projectId: string, target: string, options: PfTraceOptions = {}): Promise<PfTraceResult> {
    const value = String(target ?? '').trim();
    if (!value) throw new ProjectForgeError('validation', 'Trace 需要 target（如 ipc:通道名 / file:相对路径）');
    const args: Record<string, unknown> = { projectId: requireProjectId(projectId), target: value };
    if (options.scope) args.scope = options.scope;
    if (options.glob) args.glob = options.glob;
    const { details, text } = await readProjectCommand<Record<string, unknown>>('Trace', args);
    return { ...details, text };
  }

  // ============================ 写命令（EditCode / CreateFile / …，T3 使用） ============================

  function buildWritePayload(projectId: string, extra: Record<string, unknown>): Record<string, unknown> {
    const payload: Record<string, unknown> = { projectId, ...extra };
    if (signature.value) payload.maid = signature.value;
    return payload;
  }

  /** 执行写命令并归一化结果；status=ok 时自动静默刷新列表与当前详情。 */
  async function runWriteCommand(
    command: string,
    projectId: string,
    payload: Record<string, unknown>,
  ): Promise<PfEditResult> {
    return runMutation(async () => {
      const envelope = await pfInvokeEnvelope(command, payload, { timeoutMs: 120_000 });
      const details = (envelope.result?.details ?? {}) as Record<string, unknown>;
      const result: PfEditResult = { ...details, text: bridgeContentText(envelope) };
      if (String(details?.status ?? '') === 'ok' && currentProjectId.value === projectId) {
        await refreshProjectViews(projectId);
      }
      return result;
    });
  }

  async function editCode(args: PfEditCodeArgs): Promise<PfEditResult> {
    const path = String(args?.path ?? '').trim();
    if (!path) throw new ProjectForgeError('validation', '请填写工程内文件路径');
    const reason = String(args?.reason ?? '').trim();
    if (!reason) throw new ProjectForgeError('validation', '请填写本次改动的原因（reason）');
    const id = requireProjectId(args?.projectId);
    const { projectId: _ignoredProjectId, ...rest } = args;
    void _ignoredProjectId;
    return runWriteCommand('EditCode', id, buildWritePayload(id, { ...rest, path, reason }));
  }

  async function resolveEdit(args: PfResolveEditArgs): Promise<PfEditResult> {
    const ticketId = String(args?.ticketId ?? '').trim();
    if (!ticketId) throw new ProjectForgeError('validation', '缺少歧义票据 ticketId');
    if (args?.pick === undefined || args?.pick === null || String(args.pick).trim() === '') {
      throw new ProjectForgeError('validation', '缺少候选序号 pick');
    }
    const id = requireProjectId(args?.projectId);
    return runWriteCommand('ResolveEdit', id, buildWritePayload(id, { ticketId, pick: String(args.pick) }));
  }

  async function createFile(args: PfCreateFileArgs): Promise<PfEditResult> {
    const path = String(args?.path ?? '').trim();
    if (!path) throw new ProjectForgeError('validation', '请填写新文件路径');
    const reason = String(args?.reason ?? '').trim();
    if (!reason) throw new ProjectForgeError('validation', '请填写本次改动的原因（reason）');
    const content = String(args?.content ?? '');
    const id = requireProjectId(args?.projectId);
    const payload = buildWritePayload(id, { path, content, reason });
    if (args?.overwrite !== undefined) payload.overwrite = args.overwrite;
    if (args?.revertOnSyntaxError !== undefined) payload.revertOnSyntaxError = args.revertOnSyntaxError;
    if (args?.todo !== undefined) payload.todo = args.todo;
    return runWriteCommand('CreateFile', id, payload);
  }

  async function removeFile(args: PfRemoveFileArgs): Promise<PfEditResult> {
    const reason = String(args?.reason ?? '').trim();
    if (!reason) throw new ProjectForgeError('validation', '请填写本次改动的原因（reason）');
    const paths = Array.isArray(args?.paths) ? args.paths.map((item) => String(item ?? '').trim()).filter(Boolean) : [];
    const single = String(args?.path ?? '').trim();
    if (!single && !paths.length) throw new ProjectForgeError('validation', '请填写要删除的文件路径');
    const id = requireProjectId(args?.projectId);
    const payload = buildWritePayload(id, { reason });
    if (single) payload.path = single;
    if (paths.length) payload.paths = paths.join('\n');
    if (args?.todo !== undefined) payload.todo = args.todo;
    return runWriteCommand('RemoveFile', id, payload);
  }

  async function moveFile(args: PfMoveFileArgs): Promise<PfEditResult> {
    const reason = String(args?.reason ?? '').trim();
    if (!reason) throw new ProjectForgeError('validation', '请填写本次改动的原因（reason）');
    const from = String(args?.from ?? '').trim();
    const to = String(args?.to ?? '').trim();
    if (!from || !to) throw new ProjectForgeError('validation', 'MoveFile 需要 from 与 to');
    const id = requireProjectId(args?.projectId);
    return runWriteCommand('MoveFile', id, buildWritePayload(id, { from, to, reason }));
  }

  /** 剪切一段整行代码到同文件/另一文件（MoveCode）。 */
  async function moveCode(args: PfTransferCodeArgs): Promise<PfEditResult> {
    const reason = String(args?.reason ?? '').trim();
    if (!reason) throw new ProjectForgeError('validation', '请填写本次改动的原因（reason）');
    const from = String(args?.from ?? '').trim();
    if (!from) throw new ProjectForgeError('validation', 'MoveCode 需要 from');
    const id = requireProjectId(args?.projectId);
    const { projectId: _ignoredProjectId, ...rest } = args;
    void _ignoredProjectId;
    return runWriteCommand('MoveCode', id, buildWritePayload(id, { ...rest, from, reason }));
  }

  /** 复制一段整行代码到同文件/另一文件（CopyCode）。 */
  async function copyCode(args: PfTransferCodeArgs): Promise<PfEditResult> {
    const reason = String(args?.reason ?? '').trim();
    if (!reason) throw new ProjectForgeError('validation', '请填写本次改动的原因（reason）');
    const from = String(args?.from ?? '').trim();
    if (!from) throw new ProjectForgeError('validation', 'CopyCode 需要 from');
    const id = requireProjectId(args?.projectId);
    const { projectId: _ignoredProjectId, ...rest } = args;
    void _ignoredProjectId;
    return runWriteCommand('CopyCode', id, buildWritePayload(id, { ...rest, from, reason }));
  }

  /**
   * 任意 ProjectForge 命令直通（T3/T4 需要未封装字段时用）：
   * 返回 details（details 为空时解析 content[0].text JSON）。写命令请自行保证二次确认与署名。
   */
  async function runCommand<T = Record<string, unknown>>(
    command: string,
    args: Record<string, unknown> = {},
  ): Promise<T> {
    const name = String(command ?? '').trim();
    if (!name) throw new ProjectForgeError('validation', '缺少 command');
    return track(() => pfInvoke<T>(name, args));
  }

  return {
    // —— 设置 / 连接 ——
    baseUrl,
    token,
    configSource,
    connection,
    connectionState,
    connectionMessage,
    isConnected,
    saveSettings,
    resetSettings,
    testConnection,
    signature,
    hasSignature,
    setSignature,
    notify,
    toast: notify,
    // —— 列表 / 工作区 ——
    workspaces,
    enabledWorkspaces,
    projects,
    includeDeleted,
    projectQuery,
    query,
    workspaceFilter,
    projectsLoadedAt,
    loadWorkspaces,
    loadProjects,
    searchProjects,
    bootstrap,
    refreshAll,
    // —— 详情 ——
    detail,
    currentProjectId,
    currentProject,
    openProject,
    closeProject,
    todoProgress,
    // —— 错误 / 加载 ——
    error: errorMessage,
    errorKind,
    listError,
    listErrorKind,
    detailError,
    detailErrorKind,
    loading,
    workspacesLoading,
    listLoading,
    detailLoading,
    historyLoading,
    mutating,
    // —— 工程写操作 ——
    createProject,
    deleteProjects,
    restoreProjects,
    purgeProjects,
    // —— Todo ——
    updateTodos,
    applyTodos,
    toggleTodo,
    markTodoDoing,
    addTodos,
    removeTodo,
    updateTodo,
    setTodoStatus,
    // —— 历史 / diff / 回退 / 报告 ——
    history,
    searchHistory,
    getBatchNodes,
    getNodeDiff,
    getBatchDiff,
    rollback,
    submitReport,
    // —— 读取 / 源码 ——
    readCode,
    outline,
    findSymbol,
    trace,
    // —— 编辑写命令（T3） ——
    editCode,
    resolveEdit,
    createFile,
    removeFile,
    moveFile,
    moveCode,
    copyCode,
    // —— 直通 ——
    runCommand,
  };
});

/**
 * taskTypes.ts — 任务调度中心类型与归一化纯函数（无框架依赖，便于 L4 单测）。
 *
 * 上游契约：Plugin/VCPTaskAssistant/vcp-task-assistant.js（normalizeTask 等），
 * 字段语义见 plan/vcpmobile-more-tools-research/02 §2。
 */

export type TaskType = 'forum_patrol' | 'custom_prompt';
export type ScheduleMode = 'interval' | 'once' | 'manual' | 'cron';
export type RunStatus = 'success' | 'partial_success' | 'error';

export interface TaskSchedule {
  mode: ScheduleMode;
  intervalMinutes: number;
  runAt: string | null;
  cronValue: string | null;
  jitterSeconds: number;
}

export interface TaskRuntime {
  running: boolean;
  lastRunTime: string | null;
  lastFinishTime: string | null;
  lastResult: string | null;
  lastError: string | null;
  lastDurationMs: number | null;
  runCount: number;
  successCount: number;
  errorCount: number;
  nextRunTime: string | null;
}

export interface TaskItem {
  id: string;
  name: string;
  type: TaskType;
  enabled: boolean;
  schedule: TaskSchedule;
  agents: string[];
  maid: string;
  taskDelegation: boolean;
  runtime: TaskRuntime;
}

export interface RunRecord {
  id: string;
  taskId: string;
  taskName: string;
  triggerSource: string;
  startedAt: string;
  finishedAt: string;
  durationMs: number | null;
  status: RunStatus;
  agents: string[];
  failedAgents: string[];
  message: string;
}

export const MIN_INTERVAL_MINUTES = 10;

function asString(value: unknown, fallback = ''): string {
  return typeof value === 'string' ? value : fallback;
}

function asNumber(value: unknown, fallback = 0): number {
  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed : fallback;
}

function asStringArray(value: unknown): string[] {
  if (!Array.isArray(value)) return [];
  return value.map((item) => String(item ?? '').trim()).filter(Boolean);
}

export function defaultRuntime(): TaskRuntime {
  return {
    running: false,
    lastRunTime: null,
    lastFinishTime: null,
    lastResult: null,
    lastError: null,
    lastDurationMs: null,
    runCount: 0,
    successCount: 0,
    errorCount: 0,
    nextRunTime: null,
  };
}

function normalizeSchedule(raw: unknown): TaskSchedule {
  const input = (raw ?? {}) as Record<string, unknown>;
  const modeRaw = asString(input.mode);
  const mode: ScheduleMode = ['interval', 'once', 'manual', 'cron'].includes(modeRaw)
    ? (modeRaw as ScheduleMode)
    : 'interval';
  return {
    mode,
    intervalMinutes: Math.max(asNumber(input.intervalMinutes, 60), MIN_INTERVAL_MINUTES),
    runAt: typeof input.runAt === 'string' ? input.runAt : null,
    cronValue: typeof input.cronValue === 'string' ? input.cronValue : null,
    jitterSeconds: Math.max(asNumber(input.jitterSeconds, 0), 0),
  };
}

function normalizeRuntime(raw: unknown): TaskRuntime {
  const input = (raw ?? {}) as Record<string, unknown>;
  const base = defaultRuntime();
  return {
    running: !!input.running,
    lastRunTime: typeof input.lastRunTime === 'string' ? input.lastRunTime : base.lastRunTime,
    lastFinishTime:
      typeof input.lastFinishTime === 'string' ? input.lastFinishTime : base.lastFinishTime,
    lastResult: typeof input.lastResult === 'string' ? input.lastResult : null,
    lastError: typeof input.lastError === 'string' ? input.lastError : null,
    lastDurationMs: Number.isFinite(input.lastDurationMs)
      ? (input.lastDurationMs as number)
      : null,
    runCount: asNumber(input.runCount, 0),
    successCount: asNumber(input.successCount, 0),
    errorCount: asNumber(input.errorCount, 0),
    nextRunTime: typeof input.nextRunTime === 'string' ? input.nextRunTime : null,
  };
}

/** 归一化后端 Task（防御未知/缺失字段）。 */
export function normalizeTask(raw: unknown): TaskItem | null {
  if (!raw || typeof raw !== 'object') return null;
  const input = raw as Record<string, unknown>;
  const typeRaw = asString(input.type);
  const type: TaskType = typeRaw === 'custom_prompt' ? 'custom_prompt' : 'forum_patrol';
  const targets = (input.targets ?? {}) as Record<string, unknown>;
  const dispatch = (input.dispatch ?? {}) as Record<string, unknown>;
  const id = asString(input.id).trim();
  if (!id) return null;
  return {
    id,
    name: asString(input.name).trim() || '未命名任务',
    type,
    enabled: input.enabled !== false,
    schedule: normalizeSchedule(input.schedule),
    agents: asStringArray(targets.agents ?? input.agents),
    maid: asString(dispatch.maid, 'VCP系统'),
    taskDelegation: !!dispatch.taskDelegation,
    runtime: normalizeRuntime(input.runtime),
  };
}

export function normalizeTaskList(raw: unknown): TaskItem[] {
  if (!Array.isArray(raw)) return [];
  return raw.map(normalizeTask).filter((task): task is TaskItem => task !== null);
}

export function normalizeHistory(raw: unknown): RunRecord[] {
  if (!Array.isArray(raw)) return [];
  return raw
    .map((item): RunRecord | null => {
      if (!item || typeof item !== 'object') return null;
      const record = item as Record<string, unknown>;
      const statusRaw = asString(record.status);
      return {
        id: asString(record.id) || `run_${asNumber(record.startedAt, 0)}`,
        taskId: asString(record.taskId),
        taskName: asString(record.taskName) || '未命名任务',
        triggerSource: asString(record.triggerSource, 'scheduler'),
        startedAt: asString(record.startedAt),
        finishedAt: asString(record.finishedAt),
        durationMs: Number.isFinite(record.durationMs) ? (record.durationMs as number) : null,
        status:
          statusRaw === 'partial_success' || statusRaw === 'error'
            ? (statusRaw as RunStatus)
            : 'success',
        agents: asStringArray(record.agents),
        failedAgents: asStringArray(record.failedAgents),
        message: asString(record.message),
      };
    })
    .filter((record): record is RunRecord => record !== null);
}

/** 把 status 轮询返回的 runtime/enabled 合并进 config 任务列表（按 id）。 */
export function mergeRuntimeIntoTasks(tasks: TaskItem[], statusTasks: unknown): TaskItem[] {
  if (!Array.isArray(statusTasks)) return tasks;
  const byId = new Map(
    statusTasks
      .map((item) => {
        if (!item || typeof item !== 'object') return null;
        const record = item as Record<string, unknown>;
        const id = asString(record.id);
        return id ? ([id, record] as const) : null;
      })
      .filter((entry): entry is readonly [string, Record<string, unknown>] => entry !== null),
  );
  return tasks.map((task) => {
    const patch = byId.get(task.id);
    if (!patch) return task;
    return {
      ...task,
      enabled: patch.enabled !== undefined ? patch.enabled !== false : task.enabled,
      runtime: normalizeRuntime(patch.runtime),
    };
  });
}

/** agents 数组中的 randomN 魔法标签（后端 Fisher-Yates 随机抽取）。 */
export function splitRandomTag(agents: string[]): { agents: string[]; randomCount: number | null } {
  const tag = agents.find((agent) => /^random(\d+)$/i.test(agent));
  if (!tag) return { agents, randomCount: null };
  const match = /^random(\d+)$/i.exec(tag);
  return {
    agents: agents.filter((agent) => agent !== tag),
    randomCount: match ? Number.parseInt(match[1], 10) : null,
  };
}

/** 调度摘要（任务卡片第二行）。 */
export function scheduleSummary(task: TaskItem): string {
  const { schedule } = task;
  switch (schedule.mode) {
    case 'interval':
      return `每 ${schedule.intervalMinutes} 分钟`;
    case 'once':
      return schedule.runAt ? `定时 ${formatDateTime(schedule.runAt)}` : '定时（未设置时间）';
    case 'cron':
      return `CRON ${schedule.cronValue || '（缺少表达式）'}`;
    case 'manual':
      return '仅手动触发';
  }
}

export function formatDateTime(value: string): string {
  const timestamp = Date.parse(value);
  if (!Number.isFinite(timestamp)) return value || '时间未知';
  return new Intl.DateTimeFormat('zh-CN', {
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    hour12: false,
  }).format(timestamp);
}

export function formatDuration(ms: number | null): string {
  if (ms === null || !Number.isFinite(ms)) return '—';
  if (ms < 1000) return `${Math.round(ms)}ms`;
  const seconds = ms / 1000;
  if (seconds < 60) return `${seconds.toFixed(1)}s`;
  const minutes = Math.floor(seconds / 60);
  return `${minutes}m${Math.round(seconds % 60)}s`;
}

export const TASK_TYPE_LABEL: Record<TaskType, string> = {
  forum_patrol: '论坛巡航',
  custom_prompt: '通用提示词',
};

export const TRIGGER_SOURCE_LABEL: Record<string, string> = {
  scheduler: '调度器',
  'manual-trigger': '手动触发',
  'once-scheduler': '定时',
  'interval-scheduler': '间隔',
  'cron-scheduler': 'CRON',
};

export const RUN_STATUS_LABEL: Record<RunStatus, string> = {
  success: '成功',
  partial_success: '部分成功',
  error: '失败',
};

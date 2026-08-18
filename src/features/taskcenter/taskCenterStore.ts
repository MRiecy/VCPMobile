/**
 * taskCenterStore.ts — 任务调度中心状态机（S2a：只读 + 启停 + 触发 + 历史 + 全局开关）。
 *
 * 轮询策略：首次 GET /config（全量），之后 GET /status（轻量）；
 * 有任务 running 时 2.5s，否则 5s；退后台停、回前台立即补拉（复用 appLifecycleStore）。
 * 写操作全部走细粒度端点（PATCH/POST trigger），启停为乐观更新 + 失败回滚。
 */
import { defineStore } from 'pinia';
import { computed, ref, watch } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { useAppLifecycleStore } from '../../core/stores/appLifecycle';
import { useNotificationStore } from '../../core/stores/notification';
import {
  mergeRuntimeIntoTasks,
  normalizeHistory,
  normalizeTaskList,
  type RunRecord,
  type TaskItem,
} from './taskTypes';

const POLL_IDLE_MS = 5000;
const POLL_RUNNING_MS = 2500;

/** 503（VCPTaskAssistant 插件未加载）的错误前缀（Rust 侧语义化）。 */
export const PLUGIN_UNAVAILABLE_PREFIX = 'PLUGIN_UNAVAILABLE:';

interface TaskCenterError extends Error {
  pluginUnavailable?: boolean;
}

function toError(raw: unknown): TaskCenterError {
  const message = raw instanceof Error ? raw.message : String(raw);
  const error: TaskCenterError = new Error(
    message.startsWith(PLUGIN_UNAVAILABLE_PREFIX)
      ? message.slice(PLUGIN_UNAVAILABLE_PREFIX.length)
      : message,
  );
  if (message.startsWith(PLUGIN_UNAVAILABLE_PREFIX)) {
    error.pluginUnavailable = true;
  }
  return error;
}

export const useTaskCenterStore = defineStore('taskCenter', () => {
  const notificationStore = useNotificationStore();

  const toast = (type: 'info' | 'success' | 'warning' | 'error', message: string) => {
    notificationStore.addNotification({
      type,
      title: '任务调度中心',
      message,
      toastOnly: true,
    });
  };

  // ---------- 状态 ----------
  const tasks = ref<TaskItem[]>([]);
  const history = ref<RunRecord[]>([]);
  const globalEnabled = ref(false);
  const activeTimerCount = ref(0);
  const configLoaded = ref(false);
  const isLoading = ref(false);
  const error = ref<string | null>(null);
  const pluginUnavailable = ref(false);

  /** 触发中的任务 id 集合（in-flight 去重 + pending 态）。 */
  const triggeringIds = ref<Set<string>>(new Set());
  /** 启停切换中的任务 id 集合。 */
  const togglingIds = ref<Set<string>>(new Set());
  const globalToggling = ref(false);

  // ---------- 轮询引擎 ----------
  const sessionActive = ref(false);
  let pollTimer: ReturnType<typeof setTimeout> | null = null;
  let pollInFlight = false;

  const hasRunningTask = computed(() => tasks.value.some((task) => task.runtime.running));
  const runningCount = computed(
    () => tasks.value.filter((task) => task.runtime.running).length,
  );
  const enabledCount = computed(() => tasks.value.filter((task) => task.enabled).length);
  const errorCount = computed(
    () =>
      tasks.value.filter(
        (task) => !task.runtime.running && task.runtime.lastResult?.startsWith('error'),
      ).length,
  );

  function clearTimer(): void {
    if (pollTimer !== null) {
      clearTimeout(pollTimer);
      pollTimer = null;
    }
  }

  async function pollTick(): Promise<void> {
    if (pollInFlight) return;
    pollInFlight = true;
    try {
      if (!configLoaded.value) {
        const config = await invoke<Record<string, unknown>>('task_get_config');
        applyConfig(config);
      } else {
        const status = await invoke<Record<string, unknown>>('task_get_status');
        applyStatus(status);
      }
      error.value = null;
      pluginUnavailable.value = false;
    } catch (raw) {
      const err = toError(raw);
      pluginUnavailable.value = !!err.pluginUnavailable;
      error.value = err.message;
    } finally {
      pollInFlight = false;
      isLoading.value = false;
    }
  }

  function scheduleNext(): void {
    clearTimer();
    const lifecycle = useAppLifecycleStore();
    if (!sessionActive.value || lifecycle.isBackground) return;
    const interval = hasRunningTask.value ? POLL_RUNNING_MS : POLL_IDLE_MS;
    pollTimer = setTimeout(async () => {
      await pollTick();
      scheduleNext();
    }, interval);
  }

  // ---------- 快照应用 ----------
  function applyConfig(payload: Record<string, unknown>): void {
    const config = (payload.config ?? {}) as Record<string, unknown>;
    tasks.value = normalizeTaskList(config.tasks);
    history.value = normalizeHistory(config.history).slice(-50).reverse();
    globalEnabled.value = !!config.globalEnabled;
    configLoaded.value = true;
  }

  function applyStatus(payload: Record<string, unknown>): void {
    globalEnabled.value = !!payload.globalEnabled;
    activeTimerCount.value = Number(payload.activeTimerCount) || 0;
    tasks.value = mergeRuntimeIntoTasks(tasks.value, payload.tasks);
    const statusHistory = normalizeHistory(payload.history);
    if (statusHistory.length > 0) history.value = statusHistory;
  }

  // ---------- 会话 ----------
  async function startSession(): Promise<void> {
    if (sessionActive.value) return;
    sessionActive.value = true;
    isLoading.value = true;
    await pollTick();
    scheduleNext();
  }

  function stopSession(): void {
    sessionActive.value = false;
    clearTimer();
  }

  /** 单次轮询 tick（轮询引擎每拍调用；测试与调试亦可直接驱动）。 */
  async function pollOnce(): Promise<void> {
    await pollTick();
  }

  /** 卸载时彻底复位（重开重新全量）。 */
  function resetSession(): void {
    stopSession();
    tasks.value = [];
    history.value = [];
    configLoaded.value = false;
    error.value = null;
    pluginUnavailable.value = false;
    triggeringIds.value = new Set();
    togglingIds.value = new Set();
  }

  async function refresh(): Promise<void> {
    isLoading.value = true;
    await pollTick();
    scheduleNext();
  }

  // ---------- 写操作 ----------
  /** 启用/禁用任务：乐观更新 + 失败回滚。 */
  async function setTaskEnabled(taskId: string, enabled: boolean): Promise<void> {
    if (togglingIds.value.has(taskId)) return;
    const index = tasks.value.findIndex((task) => task.id === taskId);
    if (index === -1) return;

    const previous = tasks.value[index].enabled;
    tasks.value[index] = { ...tasks.value[index], enabled };
    togglingIds.value = new Set(togglingIds.value).add(taskId);
    try {
      await invoke('task_set_enabled', { taskId, enabled });
      toast('success', `任务已${enabled ? '启用' : '禁用'}：${tasks.value[index].name}`);
    } catch (raw) {
      tasks.value[index] = { ...tasks.value[index], enabled: previous };
      const err = toError(raw);
      pluginUnavailable.value = pluginUnavailable.value || !!err.pluginUnavailable;
      toast('error', `操作失败：${err.message}`);
    } finally {
      const next = new Set(togglingIds.value);
      next.delete(taskId);
      togglingIds.value = next;
    }
  }

  /** 手动触发（in-flight 去重；后端同步等待 Agent 响应，可能长达 3 分钟）。 */
  async function triggerTask(taskId: string): Promise<void> {
    if (triggeringIds.value.has(taskId)) return;
    const task = tasks.value.find((item) => item.id === taskId);
    if (!task) return;

    triggeringIds.value = new Set(triggeringIds.value).add(taskId);
    toast('info', `正在派发任务：${task.name}（Agent 响应可能需要数分钟）`);
    try {
      const result = await invoke<Record<string, unknown>>('task_trigger', { taskId });
      const message =
        typeof result?.message === 'string' ? result.message : '任务已触发';
      toast('success', message);
      // 触发后立即补一次状态，让 runtime/history 尽快刷新。
      void pollTick();
    } catch (raw) {
      const err = toError(raw);
      toast('error', `触发失败：${err.message}`);
    } finally {
      const next = new Set(triggeringIds.value);
      next.delete(taskId);
      triggeringIds.value = next;
    }
  }

  /** 全局调度开关（Rust 侧 read-modify-write，无覆盖任务风险）。 */
  async function setGlobalEnabled(enabled: boolean): Promise<void> {
    if (globalToggling.value) return;
    globalToggling.value = true;
    const previous = globalEnabled.value;
    globalEnabled.value = enabled;
    try {
      await invoke('task_set_global_enabled', { enabled });
      toast(enabled ? 'success' : 'warning', enabled ? '全局调度已开启' : '全局调度已暂停');
    } catch (raw) {
      globalEnabled.value = previous;
      const err = toError(raw);
      pluginUnavailable.value = pluginUnavailable.value || !!err.pluginUnavailable;
      toast('error', `全局开关失败：${err.message}`);
    } finally {
      globalToggling.value = false;
    }
  }

  // ---------- 前后台感知 ----------
  const lifecycleStore = useAppLifecycleStore();
  watch(
    () => lifecycleStore.isBackground,
    (isBackground) => {
      if (!sessionActive.value) return;
      if (isBackground) {
        clearTimer();
      } else {
        void pollTick().then(scheduleNext);
      }
    },
  );

  return {
    tasks,
    history,
    globalEnabled,
    activeTimerCount,
    configLoaded,
    isLoading,
    error,
    pluginUnavailable,
    triggeringIds,
    togglingIds,
    globalToggling,
    hasRunningTask,
    runningCount,
    enabledCount,
    errorCount,
    startSession,
    stopSession,
    pollOnce,
    resetSession,
    refresh,
    setTaskEnabled,
    triggerTask,
    setGlobalEnabled,
  };
});

/**
 * logCenterStore.ts — 日志中心状态机。
 *
 * 职责：server-log 增量协议的状态宿主（offset / trailingFragment / 行缓冲），
 * 生命周期感知轮询（页面开关 + 前后台 + 失败退避）。
 * Rust 侧只做认证代理（logcenter_fetch / logcenter_clear_server）。
 *
 * 设计来源：plan/vcpmobile-more-tools-research/01（§3 数据流、§5 轮询）。
 */
import { defineStore } from 'pinia';
import { computed, ref, watch } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { useAppLifecycleStore } from '../../core/stores/appLifecycle';
import {
  LINE_LIMIT_DEFAULT,
  clampLineLimit,
  splitLogChunk,
  stripAnsi,
} from './logText';

const STORAGE_KEYS = {
  lineLimit: 'vcp_log_limit',
  isReverse: 'vcp_log_reverse',
  autoScroll: 'vcp_log_autoscroll',
} as const;

const POLL_INTERVAL_MS = 3000;
const MAX_BACKOFF_MS = 30000;

interface LogFetchResult {
  content: string;
  offset: number;
  path: string;
  fileSize: number;
  needFullReload: boolean;
}

function readStorage(key: string): string | null {
  try {
    return window.localStorage.getItem(key);
  } catch {
    return null;
  }
}

function writeStorage(key: string, value: string): void {
  try {
    window.localStorage.setItem(key, value);
  } catch {
    /* 私密模式等场景下静默降级为会话内偏好 */
  }
}

export const useLogCenterStore = defineStore('logCenter', () => {
  // ---------- 原始缓冲 ----------
  const lines = ref<string[]>([]);
  const pendingFragment = ref('');
  const offset = ref(0);
  const logPath = ref('');
  const fileSize = ref(0);

  // ---------- 轮询引擎 ----------
    const sessionActive = ref(false); // 页面打开
  const isPaused = ref(false); // 用户显式暂停
  const isLoading = ref(false);
  const error = ref<string | null>(null);
  const consecutiveFailures = ref(0);
  let pollTimer: ReturnType<typeof setTimeout> | null = null;
  let fetchInFlight = false;

  // ---------- 偏好（持久化） ----------
  const lineLimit = ref(clampLineLimit(Number(readStorage(STORAGE_KEYS.lineLimit)) || LINE_LIMIT_DEFAULT));
  const isReverse = ref(readStorage(STORAGE_KEYS.isReverse) === '1');
  const autoScroll = ref(readStorage(STORAGE_KEYS.autoScroll) !== '0');

  // ---------- 筛选 ----------
  const filterText = ref('');

  // ---------- 新行徽标（离开底部时累计） ----------
  const newLineCount = ref(0);
  /** 视图订阅此计数器驱动吸底/徽标（每次有内容变化 +1）。 */
  const logVersion = ref(0);

  // ---------- 派生 ----------
  const trimmedLines = computed(() => {
    const limit = lineLimit.value;
    return lines.value.length > limit ? lines.value.slice(-limit) : lines.value;
  });

  const matchedLines = computed(() => {
    const keyword = filterText.value.trim().toLowerCase();
    if (!keyword) return trimmedLines.value;
    return trimmedLines.value.filter((line) => line.toLowerCase().includes(keyword));
  });

  const displayedLines = computed(() =>
    isReverse.value ? [...matchedLines.value].reverse() : matchedLines.value,
  );

  const totalBuffered = computed(() => lines.value.length);
  const matchedCount = computed(() => matchedLines.value.length);
  const isPolling = computed(
    () => sessionActive.value && !isPaused.value && pollTimer !== null,
  );

  // ---------- 内部：应用快照 ----------
  function applyFullSnapshot(data: LogFetchResult): void {
    const chunk = splitLogChunk(stripAnsi(data.content || ''));
    lines.value = chunk.lines.slice(-lineLimit.value);
    pendingFragment.value = chunk.trailing;
    offset.value = data.offset ?? 0;
    logPath.value = data.path || '';
    fileSize.value = data.fileSize ?? 0;
    logVersion.value += 1;
  }

  /** @returns 是否有新增内容 */
  function applyIncremental(data: LogFetchResult): boolean {
    if (data.path) logPath.value = data.path;
    if (typeof data.offset === 'number') offset.value = data.offset;
    if (typeof data.fileSize === 'number') fileSize.value = data.fileSize;

    const content = stripAnsi(data.content || '');
    if (!content) return false;

    // 存在半行时，旧数组最后一行就是上次的半行，丢弃后由本次拼接原位补全。
    const base =
      pendingFragment.value && lines.value.length > 0
        ? lines.value.slice(0, -1)
        : lines.value;
    const chunk = splitLogChunk(content, pendingFragment.value);
    pendingFragment.value = chunk.trailing;

    const added = chunk.lines.length;
    const merged = [...base, ...chunk.lines];
    lines.value =
      merged.length > lineLimit.value ? merged.slice(-lineLimit.value) : merged;

    if (added > 0) {
      newLineCount.value += added;
      logVersion.value += 1;
    }
    return added > 0;
  }

  // ---------- 内部：拉取 ----------
  async function fetchFull(): Promise<void> {
    const data = await invoke<LogFetchResult>('logcenter_fetch', {
      incremental: false,
      offset: 0,
    });
    applyFullSnapshot(data);
  }

  async function fetchIncremental(): Promise<void> {
    const data = await invoke<LogFetchResult>('logcenter_fetch', {
      incremental: true,
      offset: offset.value,
    });
    if (data.needFullReload) {
      await fetchFull();
      return;
    }
    applyIncremental(data);
  }

  async function fetchOnce(): Promise<void> {
    if (fetchInFlight) return;
    fetchInFlight = true;
    isLoading.value = true;
    try {
      if (offset.value > 0 || logPath.value) {
        await fetchIncremental();
      } else {
        await fetchFull();
      }
      error.value = null;
      consecutiveFailures.value = 0;
    } catch (e) {
      consecutiveFailures.value += 1;
      error.value = e instanceof Error ? e.message : String(e);
    } finally {
      fetchInFlight = false;
      isLoading.value = false;
    }
  }

  function currentInterval(): number {
    if (consecutiveFailures.value === 0) return POLL_INTERVAL_MS;
    // 指数退避：3s → 6s → 12s → 24s → 封顶 30s
    return Math.min(
      MAX_BACKOFF_MS,
      POLL_INTERVAL_MS * 2 ** Math.min(consecutiveFailures.value, 4),
    );
  }

  function clearTimer(): void {
    if (pollTimer !== null) {
      clearTimeout(pollTimer);
      pollTimer = null;
    }
  }

  function scheduleNext(): void {
    clearTimer();
    const lifecycle = useAppLifecycleStore();
    if (!sessionActive.value || isPaused.value || lifecycle.isBackground) return;
    pollTimer = setTimeout(async () => {
      await fetchOnce();
      scheduleNext();
    }, currentInterval());
  }

  // ---------- 对外动作 ----------
  /** 页面打开：立即全量/增量拉取并启动轮询。 */
  async function startSession(): Promise<void> {
    if (sessionActive.value) return;
    sessionActive.value = true;
    await fetchOnce();
    scheduleNext();
  }

  /** 单次拉取tick（轮询引擎每拍调用；测试与调试亦可直接驱动）。 */
  async function pollOnce(): Promise<void> {
    await fetchOnce();
  }

  /** 页面关闭：停止轮询（缓冲保留，重开时增量补拉）。 */
  function stopSession(): void {
    sessionActive.value = false;
    clearTimer();
  }

  /** 卸载页面时彻底清空（重开从全量开始）。 */
  function resetSession(): void {
    stopSession();
    lines.value = [];
    pendingFragment.value = '';
    offset.value = 0;
    logPath.value = '';
    fileSize.value = 0;
    newLineCount.value = 0;
    error.value = null;
    consecutiveFailures.value = 0;
  }

  function togglePause(): void {
    isPaused.value = !isPaused.value;
    if (isPaused.value) {
      clearTimer();
    } else {
      scheduleNext();
    }
  }

  /** 手动刷新：全量重拉。 */
  async function refresh(): Promise<void> {
    offset.value = 0;
    logPath.value = '';
    await fetchOnce();
    scheduleNext();
  }

  function setLineLimit(raw: number): void {
    lineLimit.value = clampLineLimit(raw);
    writeStorage(STORAGE_KEYS.lineLimit, String(lineLimit.value));
  }

  function toggleReverse(): void {
    isReverse.value = !isReverse.value;
    writeStorage(STORAGE_KEYS.isReverse, isReverse.value ? '1' : '0');
  }

  function toggleAutoScroll(): void {
    autoScroll.value = !autoScroll.value;
    writeStorage(STORAGE_KEYS.autoScroll, autoScroll.value ? '1' : '0');
  }

  /** 清空本地显示（不动服务器文件）。 */
  function clearLocal(): void {
    lines.value = [];
    pendingFragment.value = '';
    newLineCount.value = 0;
    logVersion.value += 1;
  }

  /** 清空服务器日志文件（危险，前端需二次确认后调用）。 */
  async function clearServer(): Promise<void> {
    await invoke('logcenter_clear_server');
    offset.value = 0;
    logPath.value = '';
    clearLocal();
    await fetchFull();
  }

  function acknowledgeNewLines(): void {
    newLineCount.value = 0;
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
        // 回前台：立即补拉一次再恢复节奏。
        void fetchOnce().then(scheduleNext);
      }
    },
  );

  return {
    // state
    lines,
    logPath,
    fileSize,
    isPaused,
    isLoading,
    error,
    consecutiveFailures,
    lineLimit,
    isReverse,
    autoScroll,
    filterText,
    newLineCount,
    logVersion,
    // derived
    displayedLines,
    totalBuffered,
    matchedCount,
    isPolling,
    sessionActive,
    // actions
    startSession,
    pollOnce,
    stopSession,
    resetSession,
    togglePause,
    refresh,
    setLineLimit,
    toggleReverse,
    toggleAutoScroll,
    clearLocal,
    clearServer,
    acknowledgeNewLines,
  };
});

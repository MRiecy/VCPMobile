import { defineStore } from "pinia";
import { ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

type SyncStatus =
  | "idle"
  | "connecting"
  | "connected"
  | "error"
  | "completed"
  | "completed_with_warnings";

interface SyncSummary {
  successfulTopics: number;
  totalTopics: number;
  failedTopics: number;
  legacyAttachmentWarnings: number;
  failedTopicIds: string[];
}

interface SyncTerminalError {
  code: string;
  message: string;
  failedTopicIds: string[];
}

type BufferedSessionEvent = {
  kind: "status" | "progress" | "completed";
  payload: Record<string, unknown>;
};

const MOBILE_VERSION = "1.1.4";
const DESKTOP_PLUGIN_VERSION = "1.1.0";
const WIRE_PROTOCOL_VERSION = "1.1";
const MAX_BUFFERED_SESSION_EVENTS = 32;

const emptySummary = (): SyncSummary => ({
  successfulTopics: 0,
  totalTopics: 0,
  failedTopics: 0,
  legacyAttachmentWarnings: 0,
  failedTopicIds: [],
});

const sanitizeDiagnosticText = (value: string) =>
  value
    .replace(/Bearer\s+\S+/gi, "Bearer [redacted]")
    .replace(/\b(?:sync_)?token\s*[:=]\s*\S+/gi, "token=[redacted]")
    .replace(/[A-Za-z]:\\[^\s]+/g, "[path]")
    .replace(/\/(?:home|data|storage|sdcard|tmp|var)\/[^\s]+/g, "[path]");

const errorMessage = (error: unknown) =>
  error instanceof Error ? error.message : String(error);

export const useSyncSessionStore = defineStore("syncSession", () => {
  // --- 视图状态 ---
  const isOpen = ref(false);
  const canDismiss = ref(true);

  // --- 连接状态机 ---
  const status = ref<SyncStatus>("idle");
  const activeSessionId = ref<number | null>(null);
  const summary = ref<SyncSummary>(emptySummary());
  const terminalError = ref<SyncTerminalError | null>(null);
  const retryInFlight = ref(false);

  // --- 面板视图 ---
  const activeTab = ref<"live" | "history">("live");

  // --- 同步完成后需刷新标志（once-set，不受断连等异常状态影响） ---
  const needsReload = ref(false);

  // --- 日志与进度 ---
  const logs = ref<
    { id: string; level: string; message: string; time: string }[]
  >([]);
  const progressData = ref({
    phase: "initialization",
    total: 0,
    completed: 0,
    message: "",
  });

  // --- 监听器引用 ---
  let unlistenFns: UnlistenFn[] = [];
  let listenerSetup: Promise<void> | null = null;
  let viewGeneration = 0;
  let startAttempt = 0;
  let awaitingSessionId = false;
  let bufferedSessionEvents: BufferedSessionEvent[] = [];

  const isCurrentView = (generation: number) =>
    isOpen.value && generation === viewGeneration;

  const isCurrentAttempt = (generation: number, attempt: number) =>
    isCurrentView(generation) && attempt === startAttempt;

  const isTerminal = () =>
    status.value === "error" ||
    status.value === "completed" ||
    status.value === "completed_with_warnings";

  const open = () => {
    const generation = ++viewGeneration;
    startAttempt += 1;
    cleanupListeners();
    isOpen.value = true;
    canDismiss.value = true;
    status.value = "idle";
    activeSessionId.value = null;
    summary.value = emptySummary();
    terminalError.value = null;
    retryInFlight.value = false;
    awaitingSessionId = false;
    bufferedSessionEvents = [];
    activeTab.value = "live";
    logs.value = [];
    progressData.value = {
      phase: "initialization",
      total: 0,
      completed: 0,
      message: "",
    };
    listenerSetup = registerListeners(generation);
  };

  const setLocalError = (code: string, message: string) => {
    terminalError.value = { code, message, failedTopicIds: [] };
    status.value = "error";
    canDismiss.value = true;
  };

  const beginSync = async (preserveLogs: boolean) => {
    if (status.value !== "idle") return;
    const generation = viewGeneration;
    const attempt = ++startAttempt;

    if (!preserveLogs) logs.value = [];
    progressData.value = {
      phase: "initialization",
      total: 0,
      completed: 0,
      message: "",
    };

    // 启动命令一旦进入异步链路，后端就可能在任意 await 后建立会话。
    // 必须在第一个 await 前锁定页面，避免系统返回卸载视图后留下隐形同步。
    status.value = "connecting";
    canDismiss.value = false;

    try {
      await listenerSetup;
    } catch (e: any) {
      if (isCurrentAttempt(generation, attempt)) {
        pushLog("error", `同步事件监听注册失败: ${e}`);
        setLocalError("LISTENER_SETUP_FAILED", errorMessage(e));
      }
      return;
    }
    if (!isCurrentAttempt(generation, attempt)) return;

    // 原生设备电量与省电检测保障
    try {
      const battery = await invoke<{ level: number; isPowerSaveMode: boolean }>(
        "plugin:vcp-mobile|get_battery_status",
      );
      if (!isCurrentAttempt(generation, attempt)) return;
      if (battery) {
        // 绿色日志（success级别）以便排查
        pushLog(
          "success",
          `[设备健康检测] 电量百分比: ${battery.level}%, 省电模式: ${battery.isPowerSaveMode ? "开启" : "关闭"}`,
        );

        if (battery.isPowerSaveMode) {
          pushLog(
            "error",
            "当前设备处于系统省电模式，已智能拦截同步，请关闭省电模式或充电后重试。",
          );
          setLocalError(
            "POWER_SAVE_MODE",
            "当前设备处于系统省电模式，请关闭省电模式或充电后重试。",
          );

          return;
        }
        if (battery.level > 0 && battery.level < 30) {
          pushLog(
            "error",
            `当前设备电量过低 (${battery.level}%)，低于 30% 限制，已智能拦截同步以保护电池与数据安全。`,
          );
          setLocalError(
            "BATTERY_TOO_LOW",
            `当前设备电量过低 (${battery.level}%)，低于 30% 限制。`,
          );

          return;
        }
      }
    } catch (e: any) {
      if (!isCurrentAttempt(generation, attempt)) return;
      // 容错：将真实错误打印到日志面板中以便真机排查！
      pushLog("error", `[电量检测异常] 无法获取设备电量状态: ${e}`);
      console.warn("Get battery status failed, bypassing security block:", e);
    }

    if (!isCurrentAttempt(generation, attempt)) return;
    awaitingSessionId = true;
    bufferedSessionEvents = [];
    try {
      const sessionId = await invoke<number>("start_manual_sync");
      if (!isCurrentAttempt(generation, attempt)) return;
      if (!Number.isSafeInteger(sessionId) || sessionId <= 0) {
        throw new Error("start_manual_sync 未返回有效 session ID");
      }
      activeSessionId.value = sessionId;
      awaitingSessionId = false;
      const pending = bufferedSessionEvents;
      bufferedSessionEvents = [];
      for (const event of pending) {
        if (event.payload.sessionId === sessionId) {
          applySessionEvent(event.kind, event.payload);
        }
      }
    } catch (e: unknown) {
      awaitingSessionId = false;
      bufferedSessionEvents = [];
      if (isCurrentAttempt(generation, attempt)) {
        const message = errorMessage(e);
        pushLog("error", `启动失败: ${message}`);
        setLocalError("START_SYNC_FAILED", message);
      }
    }
  };

  const startSync = () => beginSync(false);

  const retrySync = async () => {
    if (retryInFlight.value || !isTerminal()) return;
    const generation = viewGeneration;
    const retryAttempt = ++startAttempt;
    retryInFlight.value = true;
    canDismiss.value = false;
    activeSessionId.value = null;
    awaitingSessionId = false;
    bufferedSessionEvents = [];
    pushLog("info", "──────── 新同步尝试 ────────");
    try {
      await invoke("stop_sync");
      if (!isCurrentAttempt(generation, retryAttempt)) return;
      status.value = "idle";
      terminalError.value = null;
      summary.value = emptySummary();
      progressData.value = {
        phase: "initialization",
        total: 0,
        completed: 0,
        message: "",
      };
      await beginSync(true);
    } catch (error: unknown) {
      if (isCurrentView(generation)) {
        const message = errorMessage(error);
        pushLog("error", `停止旧同步失败: ${message}`);
        setLocalError("STOP_SYNC_FAILED", message);
      }
    } finally {
      if (isCurrentView(generation)) retryInFlight.value = false;
    }
  };

  const close = async () => {
    if (!isOpen.value || !canDismiss.value) return;
    viewGeneration += 1;
    startAttempt += 1;
    isOpen.value = false;
    activeSessionId.value = null;
    awaitingSessionId = false;
    bufferedSessionEvents = [];
    retryInFlight.value = false;
    activeTab.value = "live";
    cleanupListeners();
    listenerSetup = null;

    try {
      await invoke("stop_sync");
    } catch (e) {
      console.warn("[SyncSession] Failed to stop backend sync session:", e);
    }
  };

  const copyDiagnostics = async () => {
    try {
      const diagnostic = [
        `VCP Mobile: ${MOBILE_VERSION}`,
        `VCPMobileSync: ${DESKTOP_PLUGIN_VERSION}`,
        `Wire protocol: ${WIRE_PROTOCOL_VERSION}`,
        `Session: ${activeSessionId.value ?? "none"}`,
        `Status: ${status.value}`,
        `Topics: ${summary.value.successfulTopics}/${summary.value.totalTopics}`,
        `Failed topics: ${summary.value.failedTopics}`,
        `Legacy attachment warnings: ${summary.value.legacyAttachmentWarnings}`,
        terminalError.value
          ? `Error: ${terminalError.value.code} ${sanitizeDiagnosticText(terminalError.value.message)}`
          : "Error: none",
        `Failed topic IDs: ${[
          ...new Set([
            ...summary.value.failedTopicIds,
            ...(terminalError.value?.failedTopicIds ?? []),
          ]),
        ]
          .slice(0, 8)
          .join(", ") || "none"}`,
      ].join("\n");
      await navigator.clipboard.writeText(diagnostic);
      pushLog("success", "脱敏诊断信息已复制到剪贴板");
    } catch (error: unknown) {
      pushLog("error", `复制失败: ${errorMessage(error)}`);
    }
  };

  const readSummary = (value: unknown): SyncSummary => {
    const source =
      value && typeof value === "object"
        ? (value as Record<string, unknown>)
        : {};
    const readCount = (key: string) => {
      const count = source[key];
      return typeof count === "number" && Number.isSafeInteger(count) && count >= 0
        ? count
        : 0;
    };
    const failedTopicIds = Array.isArray(source.failedTopicIds)
      ? source.failedTopicIds
          .filter((id): id is string => typeof id === "string" && id.length > 0)
          .slice(0, 8)
      : [];
    return {
      successfulTopics: readCount("successfulTopics"),
      totalTopics: readCount("totalTopics"),
      failedTopics: readCount("failedTopics"),
      legacyAttachmentWarnings: readCount("legacyAttachmentWarnings"),
      failedTopicIds,
    };
  };

  const readTerminalError = (
    payload: Record<string, unknown>,
  ): SyncTerminalError => {
    const source =
      payload.error && typeof payload.error === "object"
        ? (payload.error as Record<string, unknown>)
        : {};
    return {
      code:
        typeof source.code === "string" && source.code.length > 0
          ? source.code
          : "SYNC_ATTEMPT_FAILED",
      message:
        typeof source.message === "string" && source.message.length > 0
          ? source.message
          : typeof payload.message === "string"
            ? payload.message
            : "同步失败",
      failedTopicIds: Array.isArray(source.failedTopicIds)
        ? source.failedTopicIds
            .filter(
              (id): id is string => typeof id === "string" && id.length > 0,
            )
            .slice(0, 8)
        : [],
    };
  };

  const applySessionEvent = (
    kind: BufferedSessionEvent["kind"],
    payload: Record<string, unknown>,
  ) => {
    if (kind === "progress") {
      if (isTerminal()) return;
      progressData.value = {
        phase:
          typeof payload.phase === "string"
            ? payload.phase
            : progressData.value.phase,
        total:
          typeof payload.total === "number" ? payload.total : progressData.value.total,
        completed:
          typeof payload.completed === "number"
            ? payload.completed
            : progressData.value.completed,
        message:
          typeof payload.message === "string"
            ? payload.message
            : progressData.value.message,
      };
      return;
    }

    if (kind === "status") {
      const nextStatus = payload.status;
      if (nextStatus === "error") {
        if (
          status.value === "completed" ||
          status.value === "completed_with_warnings"
        ) {
          return;
        }
        terminalError.value = readTerminalError(payload);
        summary.value = {
          ...summary.value,
          failedTopics: Math.max(
            summary.value.failedTopics,
            terminalError.value.failedTopicIds.length,
          ),
          failedTopicIds: terminalError.value.failedTopicIds,
        };
        status.value = "error";
        canDismiss.value = true;
        return;
      }
      if (isTerminal()) return;
      if (nextStatus === "open") {
        status.value = "connected";
        canDismiss.value = false;
      } else if (nextStatus === "connecting") {
        status.value = "connecting";
        canDismiss.value = false;
      } else if (
        nextStatus === "completed" ||
        nextStatus === "completed_with_warnings"
      ) {
        status.value = nextStatus;
        canDismiss.value = true;
        needsReload.value = true;
      }
      return;
    }

    if (status.value === "error") return;
    summary.value = readSummary(payload.summary);
    status.value =
      payload.status === "completed_with_warnings"
        ? "completed_with_warnings"
        : "completed";
    canDismiss.value = true;
    needsReload.value = true;
    pushLog(
      status.value === "completed_with_warnings" ? "warning" : "success",
      status.value === "completed_with_warnings"
        ? "同步完成，但存在旧附件警告"
        : "同步已全部完成，点击关闭以刷新数据",
    );
  };

  const routeSessionEvent = (
    kind: BufferedSessionEvent["kind"],
    rawPayload: unknown,
  ) => {
    if (!rawPayload || typeof rawPayload !== "object") return;
    const payload = rawPayload as Record<string, unknown>;
    const sessionId = payload.sessionId;
    if (!Number.isSafeInteger(sessionId) || (sessionId as number) <= 0) return;
    if (activeSessionId.value === sessionId) {
      applySessionEvent(kind, payload);
      return;
    }
    if (activeSessionId.value === null && awaitingSessionId) {
      bufferedSessionEvents.push({ kind, payload });
      if (bufferedSessionEvents.length > MAX_BUFFERED_SESSION_EVENTS) {
        bufferedSessionEvents.shift();
      }
    }
  };

  const registerListener = async (
    generation: number,
    eventName: string,
    callback: (event: any) => void,
  ) => {
    const fn = await listen(eventName, (event: any) => {
      if (isCurrentView(generation)) callback(event);
    });
    if (!isCurrentView(generation)) {
      fn();
      return;
    }
    unlistenFns.push(fn);
  };

  const registerListeners = async (generation: number) => {
    await Promise.all([
      registerListener(generation, "vcp-log", (event: any) => {
        const { level, category, message } = event.payload;
        if (category === "sync") pushLog(level || "info", message);
      }),

      registerListener(generation, "vcp-sync-progress", (event: any) =>
        routeSessionEvent("progress", event.payload),
      ),

      registerListener(generation, "vcp-sync-status", (event: any) =>
        routeSessionEvent("status", event.payload),
      ),

      registerListener(generation, "vcp-sync-completed", (event: any) =>
        routeSessionEvent("completed", event.payload),
      ),
    ]);
  };

  const cleanupListeners = () => {
    unlistenFns.forEach((fn) => fn());
    unlistenFns = [];
  };

  const pushLog = (level: string, message: string) => {
    const id = `${Date.now()}_${Math.random().toString(36).substring(2, 9)}`;
    logs.value.push({
      id,
      level,
      message,
      time: new Date().toLocaleTimeString(),
    });
    if (logs.value.length > 200) logs.value.shift();
  };

  const markReloaded = () => {
    needsReload.value = false;
  };

  const switchTab = (tab: "live" | "history") => {
    if (status.value === "connecting" || status.value === "connected") return;
    activeTab.value = tab;
  };

  return {
    isOpen,
    canDismiss,
    status,
    activeSessionId,
    summary,
    terminalError,
    retryInFlight,
    needsReload,
    logs,
    progressData,
    activeTab,
    open,
    close,
    startSync,
    retrySync,
    copyDiagnostics,
    markReloaded,
    switchTab,
  };
});

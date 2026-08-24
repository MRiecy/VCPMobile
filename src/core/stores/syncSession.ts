import { defineStore } from "pinia";
import { ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useDataReload } from "../composables/useDataReload";

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
  category:
    | "device"
    | "configuration"
    | "connection"
    | "compatibility"
    | "protocol"
    | "data"
    | "storage"
    | "internal";
  origin:
    | "mobile_ui"
    | "mobile_native"
    | "mobile_sync"
    | "desktop_plugin"
    | "desktop_cds";
  stage:
    | "preflight"
    | "startup"
    | "connect"
    | "handshake"
    | "owner_metadata"
    | "topic_metadata"
    | "topic_validation"
    | "messages"
    | "finalize"
    | "shutdown"
    | "history";
  retryAction: "automatic" | "after_user_action" | "manual" | "never";
  message: string;
  guidance: string;
  failedTopicIds: string[];
  logFile: string | null;
}

type BufferedSessionEvent = {
  kind: "status" | "progress" | "completed" | "log";
  payload: Record<string, unknown>;
};

const MOBILE_VERSION = "1.1.4";
const DESKTOP_PLUGIN_VERSION = "1.3.0";
const WIRE_PROTOCOL_VERSION = "1.3";
const MAX_BUFFERED_SESSION_EVENTS = 32;
const ERROR_CATEGORIES = new Set<SyncTerminalError["category"]>([
  "device",
  "configuration",
  "connection",
  "compatibility",
  "protocol",
  "data",
  "storage",
  "internal",
]);
const ERROR_ORIGINS = new Set<SyncTerminalError["origin"]>([
  "mobile_ui",
  "mobile_native",
  "mobile_sync",
  "desktop_plugin",
  "desktop_cds",
]);
const ERROR_STAGES = new Set<SyncTerminalError["stage"]>([
  "preflight",
  "startup",
  "connect",
  "handshake",
  "owner_metadata",
  "topic_metadata",
  "topic_validation",
  "messages",
  "finalize",
  "shutdown",
  "history",
]);
const RETRY_ACTIONS = new Set<SyncTerminalError["retryAction"]>([
  "automatic",
  "after_user_action",
  "manual",
  "never",
]);

const LOCAL_ERROR_COPY: Record<
  string,
  Pick<
    SyncTerminalError,
    | "category"
    | "origin"
    | "stage"
    | "retryAction"
    | "message"
    | "guidance"
  >
> = {
  POWER_SAVE_MODE: {
    category: "device",
    origin: "mobile_native",
    stage: "preflight",
    retryAction: "after_user_action",
    message: "系统省电模式已阻止本次同步",
    guidance: "关闭系统省电模式后再试。",
  },
  BATTERY_TOO_LOW: {
    category: "device",
    origin: "mobile_native",
    stage: "preflight",
    retryAction: "after_user_action",
    message: "当前电量不足，已暂停同步",
    guidance: "电量达到 30% 后再试。",
  },
  LISTENER_SETUP_FAILED: {
    category: "internal",
    origin: "mobile_ui",
    stage: "startup",
    retryAction: "manual",
    message: "同步面板未能正常接收进度",
    guidance: "关闭并重新打开同步面板后再试。",
  },
  INVALID_COMPLETION_EVENT: {
    category: "protocol",
    origin: "mobile_ui",
    stage: "finalize",
    retryAction: "after_user_action",
    message: "同步响应不符合 Wire 1.2 规范，已安全停止",
    guidance: "确认两端版本一致并重启电脑端同步插件；若仍出现，请保留最新日志。",
  },
  START_SYNC_FAILED: {
    category: "internal",
    origin: "mobile_ui",
    stage: "startup",
    retryAction: "manual",
    message: "同步组件未能正常启动",
    guidance: "重启应用后再试；若仍失败，请保留最新同步日志。",
  },
  STOP_SYNC_FAILED: {
    category: "internal",
    origin: "mobile_ui",
    stage: "shutdown",
    retryAction: "manual",
    message: "上一同步任务未能正常结束",
    guidance: "重启应用后再试；若仍失败，请保留最新同步日志。",
  },
  SYNC_ATTEMPT_FAILED: {
    category: "internal",
    origin: "mobile_ui",
    stage: "startup",
    retryAction: "manual",
    message: "同步未能完成",
    guidance: "可重试一次；若仍失败，请保留最新同步日志。",
  },
};

const localTerminalError = (code: string): SyncTerminalError => {
  const copy = LOCAL_ERROR_COPY[code] ?? LOCAL_ERROR_COPY.SYNC_ATTEMPT_FAILED;
  return {
    code,
    ...copy,
    failedTopicIds: [],
    logFile: null,
  };
};

const readSyncError = (value: unknown): SyncTerminalError | null => {
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  const source = value as Record<string, unknown>;
  if (
    typeof source.code !== "string" ||
    !/^[A-Z][A-Z0-9_]{0,63}$/.test(source.code) ||
    typeof source.category !== "string" ||
    !ERROR_CATEGORIES.has(source.category as SyncTerminalError["category"]) ||
    typeof source.origin !== "string" ||
    !ERROR_ORIGINS.has(source.origin as SyncTerminalError["origin"]) ||
    typeof source.stage !== "string" ||
    !ERROR_STAGES.has(source.stage as SyncTerminalError["stage"]) ||
    typeof source.retryAction !== "string" ||
    !RETRY_ACTIONS.has(
      source.retryAction as SyncTerminalError["retryAction"],
    ) ||
    typeof source.message !== "string" ||
    source.message.trim().length === 0 ||
    source.message.length > 200 ||
    typeof source.guidance !== "string" ||
    source.guidance.trim().length === 0 ||
    source.guidance.length > 300
  ) {
    return null;
  }
  const failedTopicIds = Array.isArray(source.failedTopicIds)
    ? source.failedTopicIds
        .filter(
          (id): id is string =>
            typeof id === "string" && id.length > 0 && id.length <= 512,
        )
        .slice(0, 8)
    : [];
  const logFile =
    typeof source.logFile === "string" &&
    source.logFile.length > 0 &&
    source.logFile.length <= 255 &&
    !source.logFile.includes("/") &&
    !source.logFile.includes("\\")
      ? source.logFile
      : null;
  return {
    code: source.code,
    category: source.category as SyncTerminalError["category"],
    origin: source.origin as SyncTerminalError["origin"],
    stage: source.stage as SyncTerminalError["stage"],
    retryAction: source.retryAction as SyncTerminalError["retryAction"],
    message: source.message.trim(),
    guidance: source.guidance.trim(),
    failedTopicIds,
    logFile,
  };
};

const parseCommandError = (
  error: unknown,
  fallbackCode: string,
): SyncTerminalError => {
  const raw = error instanceof Error ? error.message : String(error);
  const marker = "SYNC_ERROR:";
  const markerIndex = raw.indexOf(marker);
  if (markerIndex >= 0) {
    try {
      const parsed = readSyncError(
        JSON.parse(raw.slice(markerIndex + marker.length)),
      );
      if (parsed) return parsed;
    } catch {
      // Invalid command errors stay behind the fixed user-facing fallback.
    }
  }
  return localTerminalError(fallbackCode);
};

const PHASE_LABELS: Record<string, string> = {
  initialization: "初始化",
  owner_metadata: "元数据比对",
  topic_metadata: "会话主题同步",
  topic_validation: "会话校验",
  messages: "历史消息同步",
  finalize: "数据收尾",
};

const emptySummary = (): SyncSummary => ({
  successfulTopics: 0,
  totalTopics: 0,
  failedTopics: 0,
  legacyAttachmentWarnings: 0,
  failedTopicIds: [],
});

const sanitizeDiagnosticText = (value: string) =>
  value
    .replace(
      /Bearer\s+(?:"[^"\r\n]*"|'[^'\r\n]*'|[^\s,;]+)/gi,
      "Bearer [redacted]",
    )
    .replace(
      /(?:sync[_-]?)?token\s*[:=]\s*(?:"[^"]*"|'[^']*'|[^\s,;]+)/gi,
      "token=[redacted]",
    )
    // Diagnostics favor over-redaction: once an absolute path starts, redact the
    // remainder of that comma/semicolon-delimited fragment, including spaces.
    .replace(/[A-Za-z]:[\\/][^\r\n,;]*/g, "[path]")
    .replace(/file:\/\/\/[^\r\n,;]*/gi, "file:///[path]")
    .replace(/(^|[^/])\/(?!\/)[^\r\n,;]*/g, "$1[path]");

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

  // --- 同步进入写入阶段后需刷新标志（once-set，失败也可能已有部分提交） ---
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
  let lastLoggedPhase = "";
  let lastCompletedPhase = "";
  let lastConnectionStatus = "";
  // 活进度行：阶段内原地刷新的单条日志，避免刷屏又保留进度感知
  let progressLineId: string | null = null;

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
    needsReload.value = false;
    awaitingSessionId = false;
    bufferedSessionEvents = [];
    lastLoggedPhase = "";
    lastCompletedPhase = "";
    lastConnectionStatus = "";
    progressLineId = null;
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

  const setTerminalError = (error: SyncTerminalError) => {
    terminalError.value = error;
    status.value = "error";
    canDismiss.value = true;
  };

  const setLocalError = (code: string) => {
    setTerminalError(localTerminalError(code));
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
    lastLoggedPhase = "";
    lastCompletedPhase = "";
    progressLineId = null;

    // 启动命令一旦进入异步链路，后端就可能在任意 await 后建立会话。
    // 必须在第一个 await 前锁定页面，避免系统返回卸载视图后留下隐形同步。
    status.value = "connecting";
    canDismiss.value = false;

    try {
      await listenerSetup;
    } catch (error: unknown) {
      if (isCurrentAttempt(generation, attempt)) {
        console.error("[SyncSession] Failed to register sync listeners:", error);
        const terminal = localTerminalError("LISTENER_SETUP_FAILED");
        pushLog("error", terminal.message);
        setTerminalError(terminal);
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
        pushLog("success", "设备状态检查完成");

        if (battery.isPowerSaveMode) {
          const terminal = localTerminalError("POWER_SAVE_MODE");
          pushLog("error", terminal.message);
          setTerminalError(terminal);

          return;
        }
        if (battery.level > 0 && battery.level < 30) {
          const terminal = localTerminalError("BATTERY_TOO_LOW");
          pushLog("error", terminal.message);
          setTerminalError(terminal);

          return;
        }
      }
    } catch (error: unknown) {
      if (!isCurrentAttempt(generation, attempt)) return;
      pushLog("warning", "无法确认设备状态，将继续同步");
      console.warn("Get battery status failed, continuing sync:", error);
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
        const terminal = parseCommandError(e, "START_SYNC_FAILED");
        pushLog("error", terminal.message);
        setTerminalError(terminal);
      }
    }
  };

  const startSync = () => beginSync(false);

  const retrySync = async () => {
    if (retryInFlight.value || !isTerminal()) return;
    if (
      status.value === "error" &&
      terminalError.value &&
      !["manual", "after_user_action"].includes(
        terminalError.value.retryAction,
      )
    ) {
      return;
    }
    const generation = viewGeneration;
    const retryAttempt = ++startAttempt;
    retryInFlight.value = true;
    canDismiss.value = false;
    activeSessionId.value = null;
    awaitingSessionId = false;
    bufferedSessionEvents = [];
    try {
      await invoke("stop_sync");
      if (!isCurrentAttempt(generation, retryAttempt)) return;
      pushLog("info", "──────── 新同步尝试 ────────");
      status.value = "idle";
      terminalError.value = null;
      summary.value = emptySummary();
      lastLoggedPhase = "";
      lastCompletedPhase = "";
      lastConnectionStatus = "";
      progressLineId = null;
      progressData.value = {
        phase: "initialization",
        total: 0,
        completed: 0,
        message: "",
      };
      await beginSync(true);
    } catch (error: unknown) {
      if (isCurrentView(generation)) {
        const terminal = parseCommandError(error, "STOP_SYNC_FAILED");
        pushLog("error", terminal.message);
        setTerminalError(terminal);
      }
    } finally {
      if (isCurrentView(generation)) retryInFlight.value = false;
    }
  };

  const close = async () => {
    if (!isOpen.value || !canDismiss.value) return;
    const shouldReload = needsReload.value;
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
    if (shouldReload) {
      try {
        await useDataReload().performFullReload();
      } catch (error) {
        console.error("[SyncSession] Failed to reload synchronized data:", error);
      } finally {
        needsReload.value = false;
      }
    }
  };

  const copyDiagnostics = async () => {
    try {
      const failedTopicIds = [
        ...summary.value.failedTopicIds,
        ...(terminalError.value?.failedTopicIds ?? []),
      ];
      const safeFailedTopicIds = [...new Set(failedTopicIds)]
        .slice(0, 8)
        .map(sanitizeDiagnosticText)
        .join(", ");
      const diagnostic = [
        `VCP Mobile: ${MOBILE_VERSION}`,
        `VCPMobileSync: ${DESKTOP_PLUGIN_VERSION}`,
        `Wire protocol: ${WIRE_PROTOCOL_VERSION}`,
        `Session: ${activeSessionId.value ?? "none"}`,
        `Status: ${status.value}`,
        `Topics: ${summary.value.successfulTopics}/${summary.value.totalTopics}`,
        `Failed topics: ${summary.value.failedTopics}`,
        `Failed topic IDs: ${safeFailedTopicIds || "none"}`,
        `Legacy attachment warnings: ${summary.value.legacyAttachmentWarnings}`,
        terminalError.value
          ? `Error code: ${sanitizeDiagnosticText(terminalError.value.code)}`
          : "Error: none",
        `Error origin: ${terminalError.value?.origin ?? "unavailable"}`,
        `Error stage: ${terminalError.value?.stage ?? "unavailable"}`,
        `Retry action: ${terminalError.value?.retryAction ?? "unavailable"}`,
        `Log file: ${terminalError.value?.logFile ?? "unavailable"}`,
      ].join("\n");
      await navigator.clipboard.writeText(diagnostic);
      pushLog("success", "脱敏诊断信息已复制到剪贴板");
    } catch (error: unknown) {
      console.error("[SyncSession] Failed to copy diagnostics:", error);
      pushLog("error", "复制诊断失败，请稍后再试");
    }
  };

  const readSummary = (value: unknown): SyncSummary | null => {
    if (!value || typeof value !== "object" || Array.isArray(value)) return null;
    const source = value as Record<string, unknown>;
    const readCount = (key: string) => {
      const count = source[key];
      return typeof count === "number" && Number.isSafeInteger(count) && count >= 0
        ? count
        : null;
    };
    const successfulTopics = readCount("successfulTopics");
    const totalTopics = readCount("totalTopics");
    const failedTopics = readCount("failedTopics");
    const legacyAttachmentWarnings = readCount("legacyAttachmentWarnings");
    if (
      successfulTopics === null ||
      totalTopics === null ||
      failedTopics === null ||
      legacyAttachmentWarnings === null ||
      !Array.isArray(source.failedTopicIds) ||
      source.failedTopicIds.some(
        (id) => typeof id !== "string" || id.length === 0,
      )
    ) {
      return null;
    }
    const allFailedTopicIds = source.failedTopicIds as string[];
    if (
      new Set(allFailedTopicIds).size !== allFailedTopicIds.length ||
      allFailedTopicIds.length > failedTopics ||
      successfulTopics + failedTopics !== totalTopics
    ) {
      return null;
    }
    return {
      successfulTopics,
      totalTopics,
      failedTopics,
      legacyAttachmentWarnings,
      failedTopicIds: allFailedTopicIds.slice(0, 8),
    };
  };

  const readProgressSummary = (
    payload: Record<string, unknown>,
  ): Omit<SyncSummary, "failedTopicIds"> | null => {
    const keys = [
      "successfulTopics",
      "totalTopics",
      "failedTopics",
      "legacyAttachmentWarnings",
    ] as const;
    const counts = keys.map((key) => payload[key]);
    if (
      counts.some(
        (count) =>
          typeof count !== "number" ||
          !Number.isSafeInteger(count) ||
          count < 0,
      )
    ) {
      return null;
    }
    const [successfulTopics, totalTopics, failedTopics, legacyAttachmentWarnings] =
      counts as number[];
    if (successfulTopics + failedTopics > totalTopics) return null;
    return {
      successfulTopics,
      totalTopics,
      failedTopics,
      legacyAttachmentWarnings,
    };
  };

  const readTerminalError = (
    payload: Record<string, unknown>,
  ): SyncTerminalError => {
    return readSyncError(payload.error) ?? localTerminalError("SYNC_ATTEMPT_FAILED");
  };

  // 阶段内进度以「活进度行」呈现：原地刷新同一条日志，既保留进度感知又不刷屏。
  // 阶段完成或切换时定格该行，使其作为普通历史行留在日志中。
  const updateLiveProgressLine = (
    phase: string,
    completed: number,
    total: number,
  ) => {
    const text =
      phase === "messages"
        ? `已同步会话 ${completed}/${total}`
        : `${PHASE_LABELS[phase] ?? "同步处理"}进度 ${completed}/${total}`;
    const existing = progressLineId
      ? logs.value.find((l) => l.id === progressLineId)
      : undefined;
    if (existing) {
      if (existing.message !== text) existing.message = text;
    } else {
      pushLog("info", text);
      progressLineId = logs.value[logs.value.length - 1]?.id ?? null;
    }
  };

  const applySessionEvent = (
    kind: BufferedSessionEvent["kind"],
    payload: Record<string, unknown>,
  ) => {
    if (kind === "log") {
      if (
        payload.audience === "operator" &&
        typeof payload.message === "string" &&
        payload.message.trim().length > 0
      ) {
        pushLog(
          typeof payload.level === "string" ? payload.level : "info",
          payload.message.trim().slice(0, 200),
        );
      }
      return;
    }

    if (kind === "progress") {
      if (isTerminal()) return;
      const reportedPhase =
        typeof payload.phase === "string"
          ? payload.phase
          : progressData.value.phase;
      if (!Object.prototype.hasOwnProperty.call(PHASE_LABELS, reportedPhase)) {
        return;
      }
      const nextPhase = reportedPhase;
      const nextTotal =
        typeof payload.total === "number" &&
        Number.isSafeInteger(payload.total) &&
        payload.total >= 0
          ? payload.total
          : progressData.value.total;
      const nextCompleted =
        typeof payload.completed === "number" &&
        Number.isSafeInteger(payload.completed) &&
        payload.completed >= 0
          ? payload.completed
          : progressData.value.completed;
      progressData.value = {
        phase: nextPhase,
        total: nextTotal,
        completed: nextCompleted,
        // 后端阶段消息用于诊断，不进入用户界面；前端只展示稳定阶段文案。
        message: "",
      };
      if (nextPhase !== lastLoggedPhase) {
        progressLineId = null; // 定格上一阶段的进度行
        pushLog("info", `开始${PHASE_LABELS[nextPhase] ?? "同步处理"}`);
        lastLoggedPhase = nextPhase;
      }
      if (nextTotal > 0) {
        updateLiveProgressLine(nextPhase, nextCompleted, nextTotal);
      }
      if (
        nextTotal > 0 &&
        nextCompleted >= nextTotal &&
        nextPhase !== lastCompletedPhase
      ) {
        progressLineId = null; // 定格在 n/n，随后输出阶段完成行
        pushLog("success", `${PHASE_LABELS[nextPhase] ?? "当前阶段"}完成`);
        lastCompletedPhase = nextPhase;
      }
      const nextSummary = readProgressSummary(payload);
      if (nextSummary) {
        summary.value = { ...summary.value, ...nextSummary };
      }
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
        pushLog("error", terminalError.value.message);
        const failedTopics = Math.max(
          summary.value.failedTopics,
          terminalError.value.failedTopicIds.length,
        );
        summary.value = {
          ...summary.value,
          failedTopics,
          totalTopics: Math.max(
            summary.value.totalTopics,
            summary.value.successfulTopics + failedTopics,
          ),
          failedTopicIds: terminalError.value.failedTopicIds,
        };
        status.value = "error";
        canDismiss.value = true;
        return;
      }
      if (isTerminal()) return;
      if (nextStatus === "open") {
        needsReload.value = true;
        status.value = "connected";
        canDismiss.value = false;
        if (lastConnectionStatus !== "open") {
          pushLog("success", "已连接电脑端，开始同步");
          lastConnectionStatus = "open";
        }
      } else if (nextStatus === "connecting") {
        status.value = "connecting";
        canDismiss.value = false;
        if (lastConnectionStatus !== "connecting") {
          pushLog("info", "正在连接电脑端同步服务");
          lastConnectionStatus = "connecting";
        }
      } else if (
        nextStatus === "completed" ||
        nextStatus === "completed_with_warnings"
      ) {
        // The completed event carries the mandatory summary and is the only
        // authoritative success transition. A status-only terminal frame cannot
        // manufacture success or bypass summary validation.
        return;
      }
      return;
    }

    if (status.value === "error") return;
    if (
      payload.status !== "completed" &&
      payload.status !== "completed_with_warnings"
    ) {
      pushLog("error", "完成事件协议错误: status 非法");
      setLocalError("INVALID_COMPLETION_EVENT");
      return;
    }
    const completedSummary = readSummary(payload.summary);
    if (!completedSummary) {
      pushLog("error", "完成事件协议错误: summary 非法");
      setLocalError("INVALID_COMPLETION_EVENT");
      return;
    }
    if (
      completedSummary.failedTopics !== 0 ||
      completedSummary.failedTopicIds.length !== 0 ||
      (payload.status === "completed" &&
        completedSummary.legacyAttachmentWarnings !== 0) ||
      (payload.status === "completed_with_warnings" &&
        completedSummary.legacyAttachmentWarnings === 0)
    ) {
      pushLog("error", "完成事件协议错误: status 与 summary 不一致");
      setLocalError("INVALID_COMPLETION_EVENT");
      return;
    }
    summary.value = completedSummary;
    status.value = payload.status;
    canDismiss.value = true;
    needsReload.value = true;
    pushLog(
      status.value === "completed_with_warnings" ? "warning" : "success",
      status.value === "completed_with_warnings"
        ? "同步已完成，但有部分旧版附件未能解析"
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
        const { audience, category } = event.payload ?? {};
        if (category === "sync" && audience === "operator") {
          routeSessionEvent("log", event.payload);
        }
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
    switchTab,
  };
});

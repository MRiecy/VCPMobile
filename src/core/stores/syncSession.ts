import { defineStore } from "pinia";
import { ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export const useSyncSessionStore = defineStore("syncSession", () => {
  // --- 视图状态 ---
  const isOpen = ref(false);
  const canDismiss = ref(true);

  // --- 连接状态机 ---
  const status = ref<
    "idle" | "connecting" | "connected" | "error" | "completed"
  >("idle");

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

  const isCurrentView = (generation: number) =>
    isOpen.value && generation === viewGeneration;

  const isCurrentAttempt = (generation: number, attempt: number) =>
    isCurrentView(generation) && attempt === startAttempt;

  const open = () => {
    const generation = ++viewGeneration;
    startAttempt += 1;
    cleanupListeners();
    isOpen.value = true;
    canDismiss.value = true;
    status.value = "idle";
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

  const startSync = async () => {
    if (status.value !== "idle") return;
    const generation = viewGeneration;
    const attempt = ++startAttempt;

    // 首先清空上一轮的面板日志
    logs.value = [];
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
        status.value = "error";
        canDismiss.value = true;
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
          status.value = "error";
          canDismiss.value = true;

          return;
        }
        if (battery.level > 0 && battery.level < 30) {
          pushLog(
            "error",
            `当前设备电量过低 (${battery.level}%)，低于 30% 限制，已智能拦截同步以保护电池与数据安全。`,
          );
          status.value = "error";
          canDismiss.value = true;

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
    try {
      await invoke("start_manual_sync");
    } catch (e: any) {
      if (isCurrentAttempt(generation, attempt)) {
        pushLog("error", `启动失败: ${e}`);
        status.value = "error";
        canDismiss.value = true;
      }
    }
  };

  const close = async () => {
    if (!isOpen.value || !canDismiss.value) return;
    viewGeneration += 1;
    startAttempt += 1;
    isOpen.value = false;
    activeTab.value = "live";
    cleanupListeners();
    listenerSetup = null;

    try {
      await invoke("stop_sync");
    } catch (e) {
      console.warn("[SyncSession] Failed to stop backend sync session:", e);
    }
  };

  const copyLogs = async () => {
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      const files = await invoke<Array<{ filename: string }>>(
        "list_sync_log_files",
      );
      if (files && files.length > 0) {
        const content = await invoke<string>("read_sync_log_file", {
          filename: files[0].filename,
        });
        await navigator.clipboard.writeText(content);
        pushLog("success", "完整日志已复制到剪贴板");
      } else {
        const text = logs.value
          .map((l) => `[${l.time}] ${l.message}`)
          .join("\n");
        await navigator.clipboard.writeText(text);
        pushLog("success", "会话日志已复制到剪贴板");
      }
    } catch (e: any) {
      pushLog("error", `复制失败: ${e}`);
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

      registerListener(generation, "vcp-sync-progress", (event: any) => {
        progressData.value = event.payload;
      }),

      registerListener(generation, "vcp-sync-status", (event: any) => {
        const s = event.payload.status;
        if (s === "open") {
          status.value = "connected";
          canDismiss.value = false;
        }
        if (s === "error") {
          status.value = "error";
          canDismiss.value = true;
        }
      }),

      registerListener(generation, "vcp-sync-completed", () => {
        status.value = "completed";
        canDismiss.value = true;
        needsReload.value = true;

        pushLog("success", "同步已全部完成，点击关闭以刷新数据");
      }),
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
    if (status.value === "connected") return;
    activeTab.value = tab;
  };

  return {
    isOpen,
    canDismiss,
    status,
    needsReload,
    logs,
    progressData,
    activeTab,
    open,
    close,
    startSync,
    copyLogs,
    markReloaded,
    switchTab,
  };
});

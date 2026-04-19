import { ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useNotificationStore } from "../stores/notification";

type SyncStatus = "connected" | "connecting" | "disconnected";

const isSyncConnected = ref(false);

const applySyncStatus = (
  notificationStore: ReturnType<typeof useNotificationStore>,
  status: string,
  source: "event" | "snapshot" = "event",
) => {
  const normalizedStatus: SyncStatus =
    status === "connected"
      ? "connected"
      : status === "connecting"
        ? "connecting"
        : "disconnected";

  const wasConnected = isSyncConnected.value;

  if (normalizedStatus === "connected") {
    isSyncConnected.value = true;
    notificationStore.updateStatus({
      status: "connected",
      source: "Sync",
      message: "已建立与桌面端的实时数据通道",
    });

    if (!wasConnected) {
      notificationStore.addNotification({
        type: "success",
        title: "同步连接成功",
        message:
          source === "snapshot"
            ? "应用启动时已检测到与桌面端的实时数据通道已建立"
            : "已建立与桌面端的实时数据通道",
        duration: 3000,
      });
    }
    return;
  }

  if (normalizedStatus === "connecting") {
    isSyncConnected.value = false;
    notificationStore.updateStatus({
      status: "connecting",
      source: "Sync",
      message: "正在连接桌面端同步服务...",
    });
    return;
  }

  isSyncConnected.value = false;
  notificationStore.updateStatus({
    status: "disconnected",
    source: "Sync",
    message: "与桌面端同步服务断开，正在尝试重连...",
  });

  if (wasConnected) {
    notificationStore.addNotification({
      type: "warning",
      title: "同步连接中断",
      message: "与桌面端的连接已断开，正在尝试重连...",
      duration: 5000,
    });
  }
};

export const initSyncStatus = async () => {
  const notificationStore = useNotificationStore();

  // Existing sync status listener
  const unlistenStatus = await listen("vcp-sync-status", (event: any) => {
    const status = String(event.payload.status || "disconnected");
    console.log(`[SyncStatus] Received vcp-sync-status event -> ${status}`);
    applySyncStatus(notificationStore, status, "event");
  });

  // NEW: VCP-log listener for sync errors
  const unlistenLog = await listen("vcp-log", (event: any) => {
    const { level, category, message, phase } = event.payload;

    // Only handle sync-related logs
    if (category !== "sync") return;

    // Only show error notifications to user
    if (level === "error") {
      notificationStore.addNotification({
        type: "error",
        title: `同步错误 [${phase || "unknown"}]`,
        message: message,
        duration: 5000,
      });
    }

    // Always log to console for debugging
    console.log(`[VCPLog][${level}][${category}] ${message}`);
  });

  try {
    const currentStatus = await invoke<string>("get_sync_status");
    console.log(`[SyncStatus] get_sync_status -> ${currentStatus}`);
    applySyncStatus(notificationStore, currentStatus, "snapshot");
  } catch (error) {
    console.error("[SyncStatus] Failed to query sync status snapshot:", error);
  }

  // Return both unlisten functions
  return () => {
    unlistenStatus();
    unlistenLog();
  };
};

export type SyncStatusUnlisten = UnlistenFn;

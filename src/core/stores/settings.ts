import { defineStore } from "pinia";
import { ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { useNotificationStore } from "./notification";

export interface AppSettings {
  userName: string;
  vcpServerUrl: string;
  vcpApiKey: string;
  vcpLogUrl: string;
  vcpLogKey: string;
  syncServerUrl: string;
  syncHttpUrl: string;
  syncToken: string;
  adminUsername?: string;
  adminPassword?: string;
  fileKey?: string;
  topicSummaryModel: string;
  syncLogLevel: string;
  agentOrder: string[];
  groupOrder: string[];
  currentThemeMode?: string;
  syncPrerenderEnabled?: boolean;
  // [SUSPENDED BETA] 浮动助手（划词悬浮球）功能当前已暂停使用，保留字段供后续重启
  enableAssistant?: boolean;
  assistantAgentId?: string;
  distributedEnabled?: boolean;
  distributedWsUrl?: string;
  distributedVcpKey?: string;
  distributedDeviceName?: string;
  [key: string]: any;
}

export function diffSettingsPatch(
  baseline: AppSettings,
  edited: AppSettings,
): Partial<AppSettings> {
  const patch: Partial<AppSettings> = {};
  for (const key of Object.keys(edited)) {
    if (JSON.stringify(edited[key]) !== JSON.stringify(baseline[key])) {
      patch[key] = edited[key];
    }
  }
  return patch;
}

interface SettingsRecoveryStatus {
  recoveredCorrupt: boolean;
  backupKey?: string | null;
  message?: string | null;
}

export const useSettingsStore = defineStore("settings", () => {
  const settings = ref<AppSettings | null>(null);
  const loading = ref(false);
  const error = ref<string | null>(null);
  const recoveryNotified = ref(false);
  const notificationStore = useNotificationStore();
  let activeOperations = 0;
  let operationTail: Promise<void> = Promise.resolve();
  const enqueueOperation = <T>(operation: () => Promise<T>): Promise<T> => {
    activeOperations += 1;
    loading.value = true;
    const queued = operationTail.catch(() => undefined).then(operation);
    operationTail = queued.then(
      () => undefined,
      () => undefined,
    );
    return queued.finally(() => {
      activeOperations = Math.max(0, activeOperations - 1);
      loading.value = activeOperations > 0;
    });
  };

  const fetchSettings = () => enqueueOperation(async () => {
    error.value = null;
    try {
      const fetchedSettings = await invoke<AppSettings>("read_settings");
      settings.value = fetchedSettings;
      const recovery = await invoke<SettingsRecoveryStatus>("get_settings_recovery_status");
      if (recovery.recoveredCorrupt && !recoveryNotified.value) {
        recoveryNotified.value = true;
        notificationStore.addNotification({
          type: "warning",
          title: "设置已从损坏数据恢复",
          message: recovery.message || "原始设置已在数据库中备份，当前使用默认设置。",
          toastOnly: true,
        });
      }
    } catch (e: any) {
      error.value = e.toString();
      console.error("[SettingsStore] Failed to fetch settings:", e);
      throw e;
    }
  });

  const updateSettings = (updates: Record<string, any>) => enqueueOperation(async () => {
    error.value = null;
    try {
      const updated = await invoke<AppSettings>("update_settings", { updates });
      settings.value = updated;

      notificationStore.addNotification({
        type: "success",
        title: "配置同步成功",
        message: "变更已生效",
        toastOnly: true,
      });
      return updated;
    } catch (e: any) {
      error.value = e.toString();
      console.error("[SettingsStore] Failed to update settings:", e);
      throw e;
    }
  });

  return {
    settings,
    loading,
    error,
    fetchSettings,
    updateSettings,
  };
});

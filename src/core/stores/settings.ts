import { defineStore } from "pinia";
import { computed, shallowRef, ref, onScopeDispose } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useNotificationStore } from "./notification";
import type { ConfigSnapshot, ConfigUpdate, ConfigSaveResult, Settings } from "../config/generated/config";
import { buildSettingsUpdate, cloneConfig, type SettingsFieldValues } from "../config/fields";
export type { ChatEndpointMode } from "../config/generated/config";
export type AppSettings = Settings;

export const useSettingsStore = defineStore("settings", () => {
  const snapshot = shallowRef<ConfigSnapshot | null>(null);
  const settings = computed(() => snapshot.value?.data ?? null);
  const loading = ref(false);
  const error = ref<string | null>(null);
  const notifiedIssues = new Set<string>();
  const notificationStore = useNotificationStore();
  let disposed = false;
  let unlisten: UnlistenFn | undefined;
  let installingListener: Promise<void> | null = null;
  const acceptSnapshot = (incoming: ConfigSnapshot) => {
    if (disposed) return;
    if (!snapshot.value) { snapshot.value = incoming; return; }
    const next = cloneConfig(snapshot.value);
    for (const domain of Object.keys(incoming.domains) as (keyof ConfigSnapshot['domains'])[]) {
      if (BigInt(incoming.domains[domain].revision) < BigInt(next.domains[domain].revision)) continue;
      next.domains[domain] = incoming.domains[domain];
      Object.assign(next.data, { [domain]: incoming.data[domain] });
    }
    snapshot.value = next;
  };
  const ensureListener = () => {
    if (!installingListener) installingListener = listen<ConfigSnapshot>('vcp-config-changed', event => acceptSnapshot(event.payload))
      .then(stop => { if (disposed) stop(); else unlisten = stop; })
      .catch(() => { installingListener = null; });
    return installingListener;
  };
  onScopeDispose(() => { disposed = true; unlisten?.(); });
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
      await ensureListener();
      const fetched = await invoke<ConfigSnapshot>("read_settings");
      acceptSnapshot(fetched);
      for (const [domain, status] of Object.entries(fetched.domains)) {
        if (!status.issues.length) continue;
        const signature = `${domain}:${status.revision}`;
        if (notifiedIssues.has(signature)) continue;
        notifiedIssues.add(signature);
        notificationStore.addNotification({
          type: "warning", title: "部分配置需要检查",
          message: status.issues[0].message, toastOnly: true,
        });
      }
    } catch (e: any) {
      error.value = e.toString();
      console.error("[SettingsStore] Failed to fetch settings:", e);
      throw e;
    }
  });

  const updateSettings = (request: ConfigUpdate): Promise<ConfigSaveResult> => {
    const captured = cloneConfig(request);
    return enqueueOperation(async () => {
      error.value = null;
      try {
        const result = await invoke<ConfigSaveResult>("update_settings", { request: captured });
        if (result.status === "saved" || result.status === "conflict") acceptSnapshot(result.snapshot);
        if (result.status === "saved") {
          notificationStore.addNotification({ type: "success", title: "设置已保存", message: "配置变更已保存", toastOnly: true });
        } else {
          const issues = result.status === "rejected" ? result.issues : result.conflicts;
          error.value = issues.map(issue => issue.message).join("；");
        }
        return result;
      } catch {
        error.value = "设置保存失败，请重试";
        throw new Error(error.value);
      }
    });
  };

  const updateFields = (changes: Partial<SettingsFieldValues>): Promise<ConfigSaveResult> => {
    if (!snapshot.value) return Promise.reject(new Error("设置尚未加载"));
    return updateSettings(buildSettingsUpdate(snapshot.value, changes)).then(result => {
      if (result.status !== 'saved') throw new Error(error.value || '配置已变化，请检查后重试');
      return result;
    });
  };

  return {
    settings,
    snapshot,
    loading,
    error,
    fetchSettings,
    updateSettings,
    updateFields,
  };
});

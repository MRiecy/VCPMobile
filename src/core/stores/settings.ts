import { defineStore } from "pinia";
import { ref } from "vue";
import { invoke } from "@tauri-apps/api/core";

export interface AppSettings {
  userName: string;
  vcpServerUrl: string;
  vcpApiKey: string;
  vcpLogUrl: string;
  vcpLogKey: string;
  syncServerUrl: string;
  syncHttpUrl: string;
  syncToken: string;
  topicSummaryModel: string;
  syncLogLevel: string;
  agentOrder: string[];
  groupOrder: string[];
  currentThemeMode?: string;
  [key: string]: any;
}

export const useSettingsStore = defineStore("settings", () => {
  const settings = ref<AppSettings | null>(null);
  const loading = ref(false);
  const error = ref<string | null>(null);

  const fetchSettings = async () => {
    loading.value = true;
    error.value = null;
    try {
      const fetchedSettings = await invoke<AppSettings>("read_settings");
      settings.value = fetchedSettings;
    } catch (e: any) {
      error.value = e.toString();
      console.error("[SettingsStore] Failed to fetch settings:", e);
      throw e;
    } finally {
      loading.value = false;
    }
  };

  const saveSettings = async (newSettings: AppSettings) => {
    loading.value = true;
    error.value = null;
    try {
      await invoke("write_settings", { settings: newSettings });
      settings.value = newSettings;
    } catch (e: any) {
      error.value = e.toString();
      console.error("[SettingsStore] Failed to save settings:", e);
      throw e;
    } finally {
      loading.value = false;
    }
  };

  const updateSettings = async (updates: Record<string, any>) => {
    loading.value = true;
    error.value = null;
    try {
      const updated = await invoke<AppSettings>("update_settings", { updates });
      settings.value = updated;
    } catch (e: any) {
      error.value = e.toString();
      console.error("[SettingsStore] Failed to update settings:", e);
      throw e;
    } finally {
      loading.value = false;
    }
  };

  return {
    settings,
    loading,
    error,
    fetchSettings,
    saveSettings,
    updateSettings,
  };
});

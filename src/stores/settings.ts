import { defineStore } from 'pinia';
import { ref } from 'vue';
import { invoke, convertFileSrc } from '@tauri-apps/api/core';

export interface AppSettings {
  sidebarWidth: number;
  notificationsSidebarWidth: number;
  userName: string;
  vcpServerUrl: string;
  vcpApiKey: string;
  vcpLogUrl: string;
  vcpLogKey: string;
  networkNotesPaths: any[];
  enableAgentBubbleTheme: boolean;
  enableSmoothStreaming: boolean;
  minChunkBufferSize: number;
  smoothStreamIntervalMs: number;
  assistantAgent: string;
  enableDistributedServer: boolean;
  agentMusicControl: boolean;
  enableDistributedServerLogs: boolean;
  enableVcpToolInjection: boolean;
  lastOpenItemId?: string;
  lastOpenItemType?: string;
  lastOpenTopicId?: string;
  combinedItemOrder: any[];
  agentOrder: string[];
  userAvatarUrl?: string;
  resolvedUserAvatarUrl?: string;
  userAvatarCalculatedColor?: string;
  currentThemeMode?: string;
  themeLastUpdated?: number;
  flowlockContinueDelay: number;
  syncServerIp: string;
  syncServerPort: number;
  syncToken: string;
  topicSummaryModel?: string;
  topicSummaryModelTemperature?: number;
  [key: string]: any;
}

export const useSettingsStore = defineStore('settings', () => {
  const settings = ref<AppSettings | null>(null);
  const loading = ref(false);
  const error = ref<string | null>(null);

  const fetchSettings = async () => {
    loading.value = true;
    error.value = null;
    try {
      const fetchedSettings = await invoke<AppSettings>('read_app_settings');
      
      // 转换用户头像路径
      if (fetchedSettings.userAvatarUrl && !fetchedSettings.userAvatarUrl.startsWith('http') && !fetchedSettings.userAvatarUrl.startsWith('data:')) {
        try {
          fetchedSettings.resolvedUserAvatarUrl = convertFileSrc(fetchedSettings.userAvatarUrl);
        } catch (err) {
          console.warn('[SettingsStore] Failed to convert user avatar path:', err);
        }
      }
      
      settings.value = fetchedSettings;
    } catch (e: any) {
      error.value = e.toString();
      console.error('[SettingsStore] Failed to fetch settings:', e);
    } finally {
      loading.value = false;
    }
  };

  const saveSettings = async (newSettings: AppSettings) => {
    loading.value = true;
    error.value = null;
    try {
      await invoke('write_app_settings', { settings: newSettings });
      settings.value = newSettings;
    } catch (e: any) {
      error.value = e.toString();
      console.error('[SettingsStore] Failed to save settings:', e);
      throw e;
    } finally {
      loading.value = false;
    }
  };

  const updateSettings = async (updates: Record<string, any>) => {
    loading.value = true;
    error.value = null;
    try {
      const updated = await invoke<AppSettings>('update_app_settings', { updates });
      settings.value = updated;
    } catch (e: any) {
      error.value = e.toString();
      console.error('[SettingsStore] Failed to update settings:', e);
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
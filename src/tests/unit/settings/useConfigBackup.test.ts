import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from 'pinia';
import {
  buildBackupPayload,
  applyConfigBackup,
  captureBackupPreferences,
  configBackupScopes,
} from "../../../features/settings/useConfigBackup";
import type { AppSettings } from "../../../core/stores/settings";
import { useThemeStore } from '../../../core/stores/theme';
import { useChatPreferencesStore } from '../../../features/chat/preferences/store';
import { DEFAULT_VOICE_SETTINGS } from '../../../features/chat/voice/settings';


beforeEach(() => {
  const storage = window.localStorage;
  vi.stubGlobal('localStorage', {
    getItem: storage.getItem.bind(storage), setItem: storage.setItem.bind(storage),
    removeItem: storage.removeItem.bind(storage), clear: storage.clear.bind(storage),
  });
  localStorage.clear();
  document.documentElement.style.cssText = '';
  document.documentElement.className = '';
  setActivePinia(createPinia());
});

afterEach(() => {
  useThemeStore().$dispose();
  useChatPreferencesStore().$dispose();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

function makeSettings(): AppSettings {
  return {
    ...DEFAULT_VOICE_SETTINGS,
    userName: "测试用户",
    vcpServerUrl: "https://vcp.example.com",
    chatEndpointMode: "vcpTools",
    vcpApiKey: "api-key-1",
    vcpLogUrl: "ws://192.168.1.10:6005",
    vcpLogKey: "log-key-1",
    syncServerUrl: "ws://192.168.1.10:5975",
    syncHttpUrl: "http://192.168.1.10:5974",
    syncToken: "sync-token-1",
    adminUsername: "admin",
    adminPassword: "secret",
    fileKey: "file-key-1",
    distributedEnabled: true,
    distributedWsUrl: "ws://192.168.1.10:6005",
    distributedVcpKey: "dist-key",
    distributedDeviceName: "Pixel",
    topicSummaryModel: "gemini-3.1-flash-lite",
    syncLogLevel: "INFO",
    agentOrder: ["agent-1"],
    groupOrder: [],
    currentThemeMode: null,
    syncPrerenderEnabled: false,
  };
}



describe('配置恢复的真实领域状态与失败边界', () => {
  it('导出语音连接及两个独立流式选项，恢复继续走SQLite设置owner', async () => {
    const settings = { ...makeSettings(), voiceApiKey: 'voice-key', voiceAsrStream: false, voiceTtsStream: true, voiceTtsVoice: '冰糖' as const };
    const payload = buildBackupPayload(settings, captureBackupPreferences());
    expect(payload.settings).toMatchObject({ voiceApiKey: 'voice-key', voiceAsrStream: false, voiceTtsStream: true, voiceTtsVoice: '冰糖' });
    const patch = { voiceApiKey: 'restored-key', voiceLanguage: 'en' as const };
    const applySettings = vi.fn(async () => {});
    const result = await applyConfigBackup({ settings: patch, exportedAt: '' }, applySettings);
    expect(applySettings).toHaveBeenCalledExactlyOnceWith(patch);
    expect(result.applied).toEqual(['语音服务']);
  });

  it('捕获已应用的主题与聊天偏好，不使用 SQLite 中未消费的主题字段', async () => {
    const theme = useThemeStore();
    await theme.restorePreferences({ themeKey: 'themes-simple-bw.ts', mode: 'system' });
    const chat = useChatPreferencesStore();
    chat.setPresentationMode('panel');
    chat.setMessageHapticsEnabled(false);
    const payload = buildBackupPayload(makeSettings(), captureBackupPreferences());
    expect(payload.preferences.theme).toEqual({ themeKey: 'themes-simple-bw.ts', mode: 'system' });
    expect(payload.preferences.chat.presentationMode).toBe('panel');
    expect(payload.preferences.chat.messageHapticsEnabled).toBe(false);
    expect(payload.settings).not.toHaveProperty('currentThemeMode');
  });

  it('旧备份缺失偏好和总结模型时保留当前状态，只交付备份中的设置', async () => {
    const theme = useThemeStore();
    await theme.restorePreferences({ themeKey: 'themes-simple-bw.ts', mode: 'dark' });
    const chat = useChatPreferencesStore();
    chat.setPresentationMode('panel');
    const applySettings = vi.fn(async () => {});
    const result = await applyConfigBackup({ settings: { vcpApiKey: 'new-key' }, exportedAt: '' }, applySettings);
    expect(applySettings).toHaveBeenCalledExactlyOnceWith({ vcpApiKey: 'new-key' });
    expect(theme.currentTheme).toBe('themes-simple-bw.ts');
    expect(theme.mode).toBe('dark');
    expect(chat.presentationMode).toBe('panel');
    expect(result).toEqual({ applied: ['服务器与同步连接'], skipped: [], failed: [] });
  });

  it('不可用主题保持原样，其他偏好和空总结模型仍可恢复', async () => {
    const theme = useThemeStore();
    await theme.restorePreferences({ mode: 'dark' });
    const applySettings = vi.fn(async () => {});
    const backup = {
      settings: { topicSummaryModel: '' }, exportedAt: '',
      preferences: { theme: { themeKey: 'future-theme.ts', mode: 'light' as const }, chat: { messageHapticsEnabled: false } },
    };
    expect(configBackupScopes(backup)).toEqual(['话题总结模型', '主题与深浅模式', '聊天偏好']);
    const result = await applyConfigBackup(backup, applySettings);
    expect(applySettings).toHaveBeenCalledExactlyOnceWith({ topicSummaryModel: '' });
    expect(theme.mode).toBe('dark');
    expect(theme.currentTheme).toBe('themes-star-abyss.ts');
    expect(useChatPreferencesStore().messageHapticsEnabled).toBe(false);
    expect(result.skipped).toHaveLength(1);
    expect(result.failed).toEqual([]);
  });

  it('局部存储失败如实返回已恢复/未恢复范围，并能从实际存储重新读取成功项', async () => {
    const chat = useChatPreferencesStore();
    const originalWrite = localStorage.setItem.bind(localStorage);
    vi.spyOn(localStorage, 'setItem').mockImplementation((key, value) => {
      if (key === 'vcp-message-haptics-enabled') throw new Error('storage full');
      originalWrite(key, value);
    });
    const applySettings = vi.fn(async () => {});
    const result = await applyConfigBackup({
      settings: { topicSummaryModel: 'new-model' }, exportedAt: '',
      preferences: { chat: { presentationMode: 'immersive', messageHapticsEnabled: false, collapseToolsWhileStreaming: true } },
    }, applySettings);
    expect(result.applied).toContain('话题总结模型');
    expect(result.applied).toContain('聊天偏好 · 消息呈现');
    expect(result.applied).toContain('聊天偏好 · 流式工具折叠');
    expect(result.failed).toEqual([{ scope: '聊天偏好 · 消息震动', message: expect.stringContaining('storage full') }]);
    expect(chat.messageHapticsEnabled).toBe(true);
    chat.$dispose();
    setActivePinia(createPinia());
    const reopened = useChatPreferencesStore();
    expect(reopened.presentationMode).toBe('immersive');
    expect(reopened.messageHapticsEnabled).toBe(true);
    expect(reopened.collapseToolsWhileStreaming).toBe(true);
    expect(reopened.smoothStreamingEnabled).toBe(false);
  });

  it('数据库写入失败不会伪报连接已恢复，也不阻止独立聊天偏好生效', async () => {
    const result = await applyConfigBackup({
      settings: { vcpApiKey: 'new-key' }, exportedAt: '', preferences: { chat: { messageHapticsEnabled: false } },
    }, async () => { throw new Error('database unavailable'); });
    expect(result.applied).toEqual(['聊天偏好 · 消息震动']);
    expect(result.failed).toEqual([{ scope: '服务器与同步连接', message: 'Error: database unavailable' }]);
    expect(localStorage.getItem('vcp-message-haptics-enabled')).toBe('false');
  });
});

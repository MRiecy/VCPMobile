import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from 'pinia';
import {
  buildBackupPayload,
  applyConfigBackup,
  captureBackupPreferences,
  configBackupScopes,
  normalizeParsedConfigBackup,
  type BackupDomains,
} from "../../../features/settings/useConfigBackup";
import { THEME_PREFERENCE_DEFAULTS, useThemeStore } from '../../../core/stores/theme';
import { useChatPreferencesStore } from '../../../features/chat/preferences/store';
import { CHAT_PREFERENCE_DEFAULTS } from '../../../features/chat/preferences/model';


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

function makeDomains(): BackupDomains {
  return {
    identity: { userName: "测试用户" },
    server: { url: "https://vcp.example.com", endpointMode: "vcpTools", apiKey: "api-key-1", adminUsername: "admin", adminPassword: "secret", fileKey: "file-key-1" },
    log: { url: "ws://192.168.1.10:6005", key: "log-key-1" },
    sync: { serverUrl: "ws://192.168.1.10:5975", httpUrl: "http://192.168.1.10:5974", token: "sync-token-1" },
    distributed: { enabled: true, wsUrl: "ws://192.168.1.10:6005", vcpKey: "dist-key", deviceName: "Pixel" },
    conversation: { topicSummaryModel: "gemini-3.1-flash-lite" },
  };
}



describe('配置恢复的真实领域状态与失败边界', () => {
  it('群设置只传递已提供的嵌套字段，显式空值仍保留给同一次数据库提交', async () => {
    const groupPreferences = {
      'group-a': { chatMode: 'sequential' as const, modeSettings: { jev: { speakerThreshold: 0.25, memberStyles: {} } } },
      'group-b': { enableContextMessageWindow: false, modeSettings: { sequential: { speakerOrder: [] } } },
    };
    const backup = { domains: { jev: { enabled: true } }, groupPreferences, exportedAt: '' };
    const applySettings = vi.fn(async () => {});
    expect(configBackupScopes(backup)).toEqual(['JEV 设置', '群聊设置']);
    const result = await applyConfigBackup(backup, applySettings);
    expect(applySettings).toHaveBeenCalledExactlyOnceWith({ 'jev.enabled': true }, groupPreferences);
    expect(result.applied).toEqual(['JEV 设置', '群聊设置']);
  });

  it('多个SQLite领域合为一次提交，缺失叶字段不补默认值', async () => {
    const applySettings = vi.fn(async () => {});
    const result = await applyConfigBackup({
      domains: { identity: { userName: '新用户名' }, server: { apiKey: 'new-key' }, voice: { guidance: { rules: '' } } }, exportedAt: '',
    }, applySettings);
    expect(applySettings).toHaveBeenCalledExactlyOnceWith({ 'identity.userName': '新用户名', 'server.apiKey': 'new-key', 'voice.guidance.rules': '' }, {});
    expect(result.applied).toEqual(['用户身份', '主服务器连接', '语音服务']);
  });

  it('复刻恢复明确提示音频需在本机存在', async () => {
    const fields = {
      tts: { mode: 'clone' as const, cloneSampleId: 'sample-id' },
      guidance: { rules: '按角色描述，必要时生成标签' },
    };
    const applySettings = vi.fn(async () => {});
    const result = await applyConfigBackup({ domains: { voice: fields }, exportedAt: '' }, applySettings);
    expect(applySettings).toHaveBeenCalledExactlyOnceWith({ 'voice.tts.mode': 'clone', 'voice.tts.cloneSampleId': 'sample-id', 'voice.guidance.rules': '按角色描述，必要时生成标签' }, {});
    expect(result.applied).toEqual(['语音服务']);
    expect(result.skipped).toEqual([expect.stringContaining('参考音频不包含在配置备份中')]);
  });

  it('捕获已应用的主题与聊天偏好，不使用 SQLite 中未消费的主题字段', async () => {
    const theme = useThemeStore();
    await theme.restorePreferences({ themeKey: 'themes-simple-bw.ts', mode: 'system' });
    const chat = useChatPreferencesStore();
    chat.setPresentationMode('panel');
    chat.setMessageHapticsEnabled(false);
    const payload = buildBackupPayload(makeDomains(), captureBackupPreferences());
    expect(payload.preferences.theme).toEqual({ themeKey: 'themes-simple-bw.ts', mode: 'system' });
    expect(payload.preferences.chat.presentationMode).toBe('panel');
    expect(payload.preferences.chat.messageHapticsEnabled).toBe(false);
  });

  it('备份缺失偏好和总结模型时保留当前状态，只交付备份中的设置', async () => {
    const theme = useThemeStore();
    await theme.restorePreferences({ themeKey: 'themes-simple-bw.ts', mode: 'dark' });
    const chat = useChatPreferencesStore();
    chat.setPresentationMode('panel');
    const applySettings = vi.fn(async () => {});
    const result = await applyConfigBackup({ domains: { server: { apiKey: 'new-key' } }, exportedAt: '' }, applySettings);
    expect(applySettings).toHaveBeenCalledExactlyOnceWith({ 'server.apiKey': 'new-key' }, {});
    expect(theme.currentTheme).toBe('themes-simple-bw.ts');
    expect(theme.mode).toBe('dark');
    expect(chat.presentationMode).toBe('panel');
    expect(result).toEqual({ applied: ['主服务器连接'], skipped: [], failed: [] });
  });

  it('不可用主题回退默认，合法深浅模式和其他偏好仍独立恢复', async () => {
    const theme = useThemeStore();
    await theme.restorePreferences({ themeKey: 'themes-simple-bw.ts', mode: 'dark' });
    const applySettings = vi.fn(async () => {});
    const backup = {
      domains: { conversation: { topicSummaryModel: '' } }, exportedAt: '',
      preferences: { theme: { themeKey: 'future-theme.ts', mode: 'light' as const }, chat: { messageHapticsEnabled: false } },
    };
    expect(configBackupScopes(backup)).toEqual(['话题总结模型', '主题与深浅模式', '聊天偏好']);
    const result = await applyConfigBackup(backup, applySettings);
    expect(applySettings).toHaveBeenCalledExactlyOnceWith({ 'conversation.topicSummaryModel': '' }, {});
    expect(theme.mode).toBe('light');
    expect(theme.currentTheme).toBe(THEME_PREFERENCE_DEFAULTS.themeKey);
    expect(useChatPreferencesStore().messageHapticsEnabled).toBe(false);
    expect(result.skipped).toEqual([]);
    expect(result.failed).toEqual([]);
  });

  it('导入确认前按各偏好owner规范化坏值，缺项不补而合法兄弟保留', async () => {
    const theme = useThemeStore();
    await theme.restorePreferences({ themeKey: 'themes-bear-holiday.ts', mode: 'dark' });
    const chat = useChatPreferencesStore();
    chat.setCollapseToolsWhileStreaming(true);
    const normalized = normalizeParsedConfigBackup({
      domains: {}, exportedAt: '', preferences: {
        theme: { mode: null, ignored: true },
        chat: { presentationMode: 'future-mode', smoothStreamingEnabled: 'true', messageHapticsEnabled: false },
        unknown: { enabled: true },
      },
    });
    expect(normalized.preferences).toEqual({
      theme: { mode: THEME_PREFERENCE_DEFAULTS.mode },
      chat: { presentationMode: CHAT_PREFERENCE_DEFAULTS.presentationMode, smoothStreamingEnabled: false, messageHapticsEnabled: false },
    });
    expect(configBackupScopes(normalized)).toEqual(['主题与深浅模式', '聊天偏好']);
    const applySettings = vi.fn(async () => {});
    const result = await applyConfigBackup(normalized, applySettings);
    expect(applySettings).not.toHaveBeenCalled();
    expect(result.failed).toEqual([]);
    expect(theme.currentTheme).toBe('themes-bear-holiday.ts');
    expect(theme.mode).toBe('light');
    expect(chat.presentationMode).toBe('bubble');
    expect(chat.smoothStreamingEnabled).toBe(false);
    expect(chat.messageHapticsEnabled).toBe(false);
    expect(chat.collapseToolsWhileStreaming).toBe(true);
  });

  it('坏偏好组回退该owner默认，缺失或仅含未知字段不生成恢复项', () => {
    expect(normalizeParsedConfigBackup({ domains: {}, exportedAt: '', preferences: { theme: false } }).preferences)
      .toEqual({ theme: THEME_PREFERENCE_DEFAULTS });
    expect(normalizeParsedConfigBackup({ domains: {}, exportedAt: '', preferences: { chat: null } }).preferences)
      .toEqual({ chat: CHAT_PREFERENCE_DEFAULTS });
    expect(normalizeParsedConfigBackup({ domains: {}, exportedAt: '', preferences: null }).preferences)
      .toEqual({ theme: THEME_PREFERENCE_DEFAULTS, chat: CHAT_PREFERENCE_DEFAULTS });
    const missing = { domains: {}, exportedAt: '' };
    expect(normalizeParsedConfigBackup(missing)).toEqual(missing);
    expect(configBackupScopes(normalizeParsedConfigBackup({
      domains: {}, exportedAt: '', preferences: { theme: { unknown: true }, chat: {}, unknown: false },
    }))).toEqual([]);
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
      domains: { conversation: { topicSummaryModel: 'new-model' } }, exportedAt: '',
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
      domains: { server: { apiKey: 'new-key' }, voice: { asr: { stream: false } } }, exportedAt: '', preferences: { chat: { messageHapticsEnabled: false } },
    }, async () => { throw new Error('database unavailable'); });
    expect(result.applied).toEqual(['聊天偏好 · 消息震动']);
    expect(result.failed).toEqual([{ scope: '主服务器连接、语音服务', message: 'Error: database unavailable' }]);
    expect(localStorage.getItem('vcp-message-haptics-enabled')).toBe('false');
  });
});

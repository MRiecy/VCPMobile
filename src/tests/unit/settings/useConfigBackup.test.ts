import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from 'pinia';
import {
  BACKUP_SETTINGS_KEYS,
  CONFIG_BACKUP_EXTENSION,
  CONFIG_BACKUP_KIND,
  CONFIG_BACKUP_VERSION,
  buildBackupFileName,
  buildBackupPayload,
  bytesToBase64,
  pickBackupSettings,
  applyConfigBackup,
  captureBackupPreferences,
  configBackupScopes,
  type ConfigBackupFile,
} from "../../../features/settings/useConfigBackup";
import type { AppSettings } from "../../../core/stores/settings";
import { useThemeStore } from '../../../core/stores/theme';
import { useChatPreferencesStore } from '../../../features/chat/preferences/store';

function makePreferences(): ConfigBackupFile['preferences'] {
  return {
    theme: { themeKey: 'themes-star-abyss.ts', mode: 'system' },
    chat: { presentationMode: 'immersive', smoothStreamingEnabled: true, messageHapticsEnabled: false, collapseToolsWhileStreaming: true },
  };
}

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
    enableAssistant: false,
    assistantAgentId: "",
  };
}

describe("useConfigBackup 白名单提取", () => {
  it("备份身份、连接、分布式与总结模型", () => {
    expect(BACKUP_SETTINGS_KEYS).toContain("userName");
    expect(BACKUP_SETTINGS_KEYS).toContain("fileKey");
    expect(BACKUP_SETTINGS_KEYS).toContain("distributedDeviceName");
    expect(BACKUP_SETTINGS_KEYS).toContain("topicSummaryModel");
  });

  it("只提取白名单字段，排除本地偏好与排序数据", () => {
    const picked = pickBackupSettings(makeSettings());
    expect(picked.userName).toBe("测试用户");
    expect(picked.chatEndpointMode).toBe("vcpTools");
    expect(picked.vcpApiKey).toBe("api-key-1");
    expect(picked.distributedEnabled).toBe(true);
    expect("agentOrder" in picked).toBe(false);
    expect("groupOrder" in picked).toBe(false);
    expect(picked.topicSummaryModel).toBe('gemini-3.1-flash-lite');
    expect("syncLogLevel" in picked).toBe(false);
    expect("currentThemeMode" in picked).toBe(false);
  });

  it("distributedEnabled 非布尔时归一为 false，缺失字符串字段归一为空串", () => {
    const malformed = {
      ...makeSettings(),
      distributedEnabled: "yes",
      adminPassword: undefined,
    } as unknown as AppSettings;
    const picked = pickBackupSettings(malformed);
    expect(picked.distributedEnabled).toBe(false);
    expect(picked.adminPassword).toBe("");
  });
});

describe("useConfigBackup 导出 payload", () => {
  it("文件名带时间戳与 .vcpcfg.json 专用后缀", () => {
    const name = buildBackupFileName(new Date(2026, 7, 18, 9, 5));
    expect(name).toBe(`vcp-mobile-config-20260818-0905${CONFIG_BACKUP_EXTENSION}`);
  });

  it("payload 结构包含种类/版本/时间戳/设置，不含头像", () => {
    const preferences = makePreferences();
    const payload = buildBackupPayload(makeSettings(), preferences, new Date(2026, 7, 18));
    expect(payload.app).toBe("vcp-mobile");
    expect(payload.kind).toBe(CONFIG_BACKUP_KIND);
    expect(payload.version).toBe(CONFIG_BACKUP_VERSION);
    expect(payload.exportedAt).toBe(new Date(2026, 7, 18).toISOString());
    expect(payload.settings.syncToken).toBe("sync-token-1");
    expect(payload.preferences).toEqual(preferences);
    preferences.theme.mode = 'dark';
    expect(payload.preferences.theme.mode).toBe('system');
    expect("avatar" in payload).toBe(false);
  });
});

describe('配置恢复的真实领域状态与失败边界', () => {
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


describe("useConfigBackup Base64 工具", () => {
  it("bytesToBase64 对 UTF-8 中文编码可逆", () => {
    const original = new TextEncoder().encode(JSON.stringify({ userName: "测试用户🔥" }));
    const binary = atob(bytesToBase64(original));
    const roundtripped = Uint8Array.from(binary, (c) => c.charCodeAt(0));
    expect(Array.from(roundtripped)).toEqual(Array.from(original));
  });
});

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from 'pinia';
import {
  buildBackupPayload,
  applyConfigBackup,
  captureBackupPreferences,
  configBackupScopes,
  type BackupDomains,
} from "../../../features/settings/useConfigBackup";
import { useThemeStore } from '../../../core/stores/theme';
import { useChatPreferencesStore } from '../../../features/chat/preferences/store';


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
  it('多个SQLite领域合为一次提交，缺失叶字段不补默认值', async () => {
    const applySettings = vi.fn(async () => {});
    const result = await applyConfigBackup({
      domains: { identity: { userName: '新用户名' }, server: { apiKey: 'new-key' }, voice: { guidance: { autoGuidance: false } } }, exportedAt: '',
    }, applySettings);
    expect(applySettings).toHaveBeenCalledExactlyOnceWith({ 'identity.userName': '新用户名', 'server.apiKey': 'new-key', 'voice.guidance.autoGuidance': false });
    expect(result.applied).toEqual(['用户身份', '主服务器连接', '语音服务']);
  });

  it('导出语音连接、输入编码及独立流式选项，恢复继续走SQLite设置owner', async () => {
    const domains: BackupDomains = { ...makeDomains(), voice: { connection: { apiKey: 'voice-key' }, asr: { inputEncoding: 'mp3_64', stream: false }, tts: { stream: true, voice: '冰糖' } } };
    const payload = buildBackupPayload(domains, captureBackupPreferences());
    expect(payload.domains.voice).toEqual(domains.voice);
    const patch = { voice: { connection: { apiKey: 'restored-key' }, asr: { inputEncoding: 'wav' as const, language: 'en' } } };
    const applySettings = vi.fn(async () => {});
    const result = await applyConfigBackup({ domains: patch, exportedAt: '' }, applySettings);
    expect(applySettings).toHaveBeenCalledExactlyOnceWith({ 'voice.connection.apiKey': 'restored-key', 'voice.asr.inputEncoding': 'wav', 'voice.asr.language': 'en' });
    expect(result.applied).toEqual(['语音服务']);
  });

  it('仅含输入编码的备份仍识别为语音服务并交给设置owner恢复', async () => {
    const backup = { domains: { voice: { asr: { inputEncoding: 'mp3_64' as const } } }, exportedAt: '' };
    const applySettings = vi.fn(async () => {});
    expect(configBackupScopes(backup)).toEqual(['语音服务']);
    const result = await applyConfigBackup(backup, applySettings);
    expect(applySettings).toHaveBeenCalledExactlyOnceWith({ 'voice.asr.inputEncoding': 'mp3_64' });
    expect(result).toEqual({ applied: ['语音服务'], skipped: [], failed: [] });
  });

  it('自定义VAD毫秒值以数字导出，独立恢复时保留未提供的语音配置', async () => {
    const vad = { sensitivity: 'high' as const, endSilenceMs: 777, playbackOnsetMs: 1001 };
    const payload = buildBackupPayload({ ...makeDomains(), voice: { vad } }, captureBackupPreferences());
    expect(payload.domains.voice?.vad).toEqual(vad);
    const backup = { domains: { voice: { vad } }, exportedAt: '' };
    expect(configBackupScopes(backup)).toEqual(['语音服务']);
    const applySettings = vi.fn(async () => {});
    await applyConfigBackup(backup, applySettings);
    expect(applySettings).toHaveBeenCalledExactlyOnceWith({ 'voice.vad.sensitivity': 'high', 'voice.vad.endSilenceMs': 777, 'voice.vad.playbackOnsetMs': 1001 });
  });

  it('备份保留音色模式与风格规则的类型，复刻恢复明确提示音频需在本机存在', async () => {
    const fields = {
      tts: { mode: 'clone' as const, cloneSampleId: 'sample-id', stylePrompt: '轻松自然' },
      guidance: { audioTagRules: '必要时生成标签', autoGuidance: false, guidanceRules: '按角色描述' },
    };
    const payload = buildBackupPayload({ ...makeDomains(), voice: fields }, captureBackupPreferences());
    expect(payload.domains.voice).toEqual(fields);
    const applySettings = vi.fn(async () => {});
    const result = await applyConfigBackup({ domains: { voice: fields }, exportedAt: '' }, applySettings);
    expect(applySettings).toHaveBeenCalledExactlyOnceWith({ 'voice.tts.mode': 'clone', 'voice.tts.cloneSampleId': 'sample-id', 'voice.tts.stylePrompt': '轻松自然', 'voice.guidance.audioTagRules': '必要时生成标签', 'voice.guidance.autoGuidance': false, 'voice.guidance.guidanceRules': '按角色描述' });
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
    expect(payload.domains).not.toHaveProperty('currentThemeMode');
    expect(payload).not.toHaveProperty('settings');
  });

  it('备份缺失偏好和总结模型时保留当前状态，只交付备份中的设置', async () => {
    const theme = useThemeStore();
    await theme.restorePreferences({ themeKey: 'themes-simple-bw.ts', mode: 'dark' });
    const chat = useChatPreferencesStore();
    chat.setPresentationMode('panel');
    const applySettings = vi.fn(async () => {});
    const result = await applyConfigBackup({ domains: { server: { apiKey: 'new-key' } }, exportedAt: '' }, applySettings);
    expect(applySettings).toHaveBeenCalledExactlyOnceWith({ 'server.apiKey': 'new-key' });
    expect(theme.currentTheme).toBe('themes-simple-bw.ts');
    expect(theme.mode).toBe('dark');
    expect(chat.presentationMode).toBe('panel');
    expect(result).toEqual({ applied: ['主服务器连接'], skipped: [], failed: [] });
  });

  it('不可用主题保持原样，其他偏好和空总结模型仍可恢复', async () => {
    const theme = useThemeStore();
    await theme.restorePreferences({ mode: 'dark' });
    const applySettings = vi.fn(async () => {});
    const backup = {
      domains: { conversation: { topicSummaryModel: '' } }, exportedAt: '',
      preferences: { theme: { themeKey: 'future-theme.ts', mode: 'light' as const }, chat: { messageHapticsEnabled: false } },
    };
    expect(configBackupScopes(backup)).toEqual(['话题总结模型', '主题与深浅模式', '聊天偏好']);
    const result = await applyConfigBackup(backup, applySettings);
    expect(applySettings).toHaveBeenCalledExactlyOnceWith({ 'conversation.topicSummaryModel': '' });
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

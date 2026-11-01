import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { flushPromises, mount, type VueWrapper } from '@vue/test-utils';
import type { Component } from 'vue';
import { deleteTempFile } from 'tauri-plugin-vcp-mobile';
import { useSettingsStore, type AppSettings } from '../../../core/stores/settings';
import { useAssistantStore } from '../../../core/stores/assistant';
import { useChatSessionStore } from '../../../core/stores/chatSessionStore';
import { readStagedEntityBackup } from '../../../features/settings/useConfigBackup';
import { useNotificationStore } from '../../../core/stores/notification';
import { THEME_PREFERENCE_DEFAULTS } from '../../../core/stores/theme';
import type { SettingsSaveResult } from '../../../features/settings/editing/useSettingsEditSession';
import ConfigBackupSection from '../../../features/settings/components/ConfigBackupSection.vue';
import VcpCoreSettingsSection from '../../../features/settings/components/VcpCoreSettingsSection.vue';
import SyncSettingsSection from '../../../features/settings/components/SyncSettingsSection.vue';
import ConnectionSettingsPage from '../../../features/settings/pages/ConnectionSettingsPage.vue';
import AdvancedSettingsPage from '../../../features/settings/pages/AdvancedSettingsPage.vue';
import SettingsActionWithStatus from '../../../components/settings/SettingsActionWithStatus.vue';
import SettingsTextField from '../../../components/settings/SettingsTextField.vue';
import VcpConfirm from '../../../components/ui/VcpConfirm.vue';
import { invokeMock, mockInvoke } from '../../mocks/tauri';
import { SETTINGS_DEFAULTS } from '../../../core/config/generated/defaults';
import type { ConfigSnapshot, ConfigSaveResult, ConfigBackupImportResult, ConfigUpdate } from '../../../core/config/generated/config';
import { applySettingsFields, cloneConfig, readSetting, type SettingsFieldValues } from '../../../core/config/fields';
import { CONNECTION_FIELDS } from '../../../features/settings/editing/fieldSets';

const harness = vi.hoisted(() => ({
  modals: new Set<string>(),
  openSyncSession: vi.fn(),
  openRebuildSession: vi.fn(),
}));

vi.mock('../../../core/utils/platform', () => ({ isAndroid: true, isIOS: false }));
vi.mock('../../../core/stores/overlay', () => ({
  useOverlayStore: () => ({ openSyncSession: harness.openSyncSession, openRebuildSession: harness.openRebuildSession }),
}));
vi.mock('../../../core/composables/useModalHistory', () => ({
  useModalHistory: () => ({
    registerModal: (id: string) => harness.modals.add(id),
    unregisterModal: (id: string) => harness.modals.delete(id),
  }),
}));

function makeSettings(overrides: Partial<SettingsFieldValues> = {}): AppSettings {
  return applySettingsFields(cloneConfig(SETTINGS_DEFAULTS), {
    'identity.userName': '用户', 'server.adminUsername': 'admin', 'server.adminPassword': 'password',
    'server.url': 'https://draft.example', 'server.endpointMode': 'standard', 'server.apiKey': 'draft-key',
    'conversation.topicSummaryModel': 'summary-model', ...overrides,
  });
}

function snapshot(data = makeSettings(), revision = '1'): ConfigSnapshot {
  return { data, domains: Object.fromEntries(Object.keys(data).map(domain => [domain, { revision, schemaVersion: 1 }])) as unknown as ConfigSnapshot['domains'] };
}

function connectionDraft(data = makeSettings()) {
  return Object.fromEntries(CONNECTION_FIELDS.map(path => [path, readSetting(data, path)]));
}

function applyRequest(current: ConfigSnapshot, request: ConfigUpdate): ConfigSnapshot {
  const next = cloneConfig(current);
  const changes: Record<string, unknown> = {};
  for (const patch of request.patches) {
    for (const [field, value] of Object.entries(patch.changes)) changes[`${patch.domain}.${field}`] = value;
    next.domains[patch.domain].revision = String(BigInt(next.domains[patch.domain].revision) + 1n);
  }
  next.data = applySettingsFields(next.data, changes as Partial<SettingsFieldValues>);
  return next;
}

function saved(snapshot = makeSettings()): SettingsSaveResult {
  return { ok: true, changed: true, dirty: false, snapshot };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((accept, decline) => { resolve = accept; reject = decline; });
  return { promise, resolve, reject };
}

const wrappers: VueWrapper[] = [];

function render(component: Component, props: Record<string, unknown>) {
  const wrapper = mount(component, {
    props,
    global: { stubs: { Teleport: true, Transition: true } },
  });
  wrappers.push(wrapper);
  return wrapper;
}

function action(wrapper: VueWrapper, label: string) {
  const found = wrapper.findAllComponents(SettingsActionWithStatus)
    .find(candidate => candidate.props('buttonLabel') === label);
  if (!found) throw new Error(`Missing settings action: ${label}`);
  return found;
}

function callsFor(command: string) {
  return invokeMock.mock.calls.filter(([name]) => name === command);
}

function field(wrapper: VueWrapper, label: string) {
  const found = wrapper.findAllComponents(SettingsTextField)
    .find(candidate => candidate.props('label') === label);
  if (!found) throw new Error(`Missing settings field: ${label}`);
  return found.get('input');
}

async function confirmImport(wrapper: VueWrapper) {
  const button = wrapper.get('[role="dialog"]').findAll('button').find(candidate => candidate.text() === '确认');
  if (!button) throw new Error('Missing import confirmation button');
  await button.trigger('click');
}

beforeEach(() => {
  localStorage.clear();
  setActivePinia(createPinia());
  mockInvoke('preview_chat_endpoint', () => ({ finalUrl: 'https://draft.example/v1/chat/completions', modelDiscoveryUrl: 'https://draft.example/v1/models' }));
  mockInvoke('test_vcp_connection', () => ({ success: true, status: 200, modelCount: 2, modelDiscoveryAvailable: true }));
  mockInvoke('regenerate_emoticon_library', () => 2);
  mockInvoke('plugin:vcp-mobile|save_to_downloads', () => ({ displayName: 'backup.vcpcfg' }));
  mockInvoke('export_config_domains', () => ({ identity: { userName: '已提交用户' }, server: { apiKey: 'saved-key' }, conversation: { topicSummaryModel: 'summary-model' } }));
  mockInvoke('export_group_preferences', () => ({}));
  mockInvoke('export_agent_preferences', () => ({}));
  mockInvoke('get_favorite_models', () => []);
  mockInvoke('get_tarven_rules', () => []);
});

afterEach(() => {
  for (const wrapper of wrappers.splice(0)) wrapper.unmount();
  harness.modals.clear();
});

describe('依赖配置提交的设置动作', () => {
  it.each(['export_group_preferences', 'export_agent_preferences', 'get_favorite_models', 'get_tarven_rules'])(
    '导出读取 %s 失败时显示错误，不保存缺项备份', async command => {
      mockInvoke(command, () => { throw new Error('backup read failed'); });
      const wrapper = render(ConfigBackupSection, { active: true, commit: async () => saved(), applyImport: vi.fn() });
      await action(wrapper, '导出').get('button').trigger('click');
      await flushPromises();
      expect(wrapper.text()).toContain('backup read failed');
      expect(callsFor('plugin:vcp-mobile|save_to_downloads')).toHaveLength(0);
    },
  );

  const cases = [
    { component: VcpCoreSettingsSection, label: '检查模型发现', command: 'test_vcp_connection' },
    { component: SyncSettingsSection, label: 'REFRESH', command: 'regenerate_emoticon_library' },
    { component: ConfigBackupSection, label: '导出', command: 'plugin:vcp-mobile|save_to_downloads' },
  ];

  for (const { component, label, command } of cases) {
    it(`${label}：保存失败保留错误，不执行后续任务`, async () => {
      const commit = vi.fn(async (): Promise<SettingsSaveResult> => ({ ok: false, error: '磁盘不可写' }));
      const wrapper = render(component, { ...(component === ConfigBackupSection ? {} : { settings: connectionDraft() }), active: true, disabled: false, commit, applyImport: vi.fn() });

      await action(wrapper, label).get('button').trigger('click');
      await flushPromises();

      expect(commit).toHaveBeenCalledTimes(1);
      expect(callsFor(command)).toHaveLength(0);
      expect(wrapper.text()).toContain('磁盘不可写');
    });

    it(`${label}：保存等待中隐藏页面，不再启动后续任务`, async () => {
      const pending = deferred<SettingsSaveResult>();
      const commit = vi.fn(() => pending.promise);
      const wrapper = render(component, { ...(component === ConfigBackupSection ? {} : { settings: connectionDraft() }), active: true, disabled: false, commit, applyImport: vi.fn() });

      await action(wrapper, label).get('button').trigger('click');
      expect(commit).toHaveBeenCalledTimes(1);
      expect(callsFor(command)).toHaveLength(0);
      await wrapper.setProps({ active: false });
      pending.resolve(saved());
      await flushPromises();

      expect(callsFor(command)).toHaveLength(0);
    });
  }

  it('连接诊断使用提交返回的确定快照，不使用仍在页面上的旧草稿', async () => {
    const snapshot = makeSettings({ 'server.url': 'https://saved.example', 'server.apiKey': 'saved-key', 'server.endpointMode': 'raw' });
    const wrapper = render(VcpCoreSettingsSection, {
      settings: connectionDraft(), active: true, disabled: false, commit: async () => saved(snapshot),
    });

    await action(wrapper, '检查模型发现').get('button').trigger('click');
    await flushPromises();

    expect(callsFor('test_vcp_connection')).toEqual([['test_vcp_connection', {
      vcpUrl: 'https://saved.example', vcpApiKey: 'saved-key', chatEndpointMode: 'raw',
    }]]);
  });

  it('提交完成后才读取并编码导出快照', async () => {
    const pending = deferred<SettingsSaveResult>();
    const wrapper = render(ConfigBackupSection, { active: true, disabled: false, commit: () => pending.promise, applyImport: vi.fn() });
    await action(wrapper, '导出').get('button').trigger('click');
    expect(callsFor('plugin:vcp-mobile|save_to_downloads')).toHaveLength(0);
    pending.resolve(saved(makeSettings({ 'identity.userName': '已提交用户', 'server.apiKey': 'saved-key' })));
    await flushPromises();

    const exportArgs = callsFor('plugin:vcp-mobile|save_to_downloads')[0]?.[1];
    expect(exportArgs).toBeDefined();
    const bytes = Uint8Array.from(atob(exportArgs!.contentBase64 as string), character => character.charCodeAt(0));
    const payload = JSON.parse(new TextDecoder().decode(bytes));
    expect(callsFor('export_config_domains')).toHaveLength(1);
    expect(payload.domains.identity.userName).toBe('已提交用户');
    expect(payload.domains.server.apiKey).toBe('saved-key');
  });

  it('领域导出快照读取期间隐藏页面，迟到结果不会启动原生保存', async () => {
    const pending = deferred<Record<string, unknown>>();
    mockInvoke('export_config_domains', () => pending.promise);
    const wrapper = render(ConfigBackupSection, { active: true, disabled: false, commit: async () => saved(), applyImport: vi.fn() });
    await action(wrapper, '导出').get('button').trigger('click');
    await flushPromises();
    expect(callsFor('export_config_domains')).toHaveLength(1);
    await wrapper.setProps({ active: false });
    pending.resolve({ server: { apiKey: 'committed-key' } });
    await flushPromises();
    expect(callsFor('plugin:vcp-mobile|save_to_downloads')).toHaveLength(0);
  });

  it('原生存储权限被拒绝后显示失败，不重复申请权限或再次保存', async () => {
    mockInvoke('plugin:vcp-mobile|save_to_downloads', () => { throw new Error('保存文件需要储存空间权限'); });
    const wrapper = render(ConfigBackupSection, { active: true, commit: async () => saved(), applyImport: vi.fn() });
    await action(wrapper, '导出').get('button').trigger('click');
    await flushPromises();
    expect(callsFor('plugin:vcp-mobile|save_to_downloads')).toHaveLength(1);
    expect(callsFor('plugin:vcp-mobile|request_android_permission')).toHaveLength(0);
    expect(wrapper.text()).toContain('导出失败');
    expect(wrapper.text()).toContain('保存文件需要储存空间权限');
  });

  it('打开同步面板等待连接保存，等待期间重复点击不重复打开', async () => {
    const pending = deferred<SettingsSaveResult>();
    const commit = vi.fn(() => pending.promise);
    const wrapper = render(SyncSettingsSection, { settings: connectionDraft(), active: true, disabled: false, commit });
    const button = wrapper.findAll('button').find(candidate => candidate.text() === 'OPEN PANEL');
    expect(button).toBeDefined();
    await button!.trigger('click');
    await button!.trigger('click');

    expect(commit).toHaveBeenCalledTimes(1);
    expect(harness.openSyncSession).not.toHaveBeenCalled();
    pending.resolve(saved());
    await flushPromises();
    expect(harness.openSyncSession).toHaveBeenCalledTimes(1);
  });

  it('同步面板等待保存时页面失活，迟到成功不会打开面板', async () => {
    const pending = deferred<SettingsSaveResult>();
    const wrapper = render(SyncSettingsSection, {
      settings: connectionDraft(), active: true, disabled: false, commit: () => pending.promise,
    });
    const button = wrapper.findAll('button').find(candidate => candidate.text() === 'OPEN PANEL')!;
    await button.trigger('click');
    await wrapper.setProps({ active: false });
    pending.resolve(saved());
    await flushPromises();
    expect(harness.openSyncSession).not.toHaveBeenCalled();
  });
});

describe('配置导入的原生选择与局部覆盖层生命周期', () => {
  const filePath = 'tests/fixtures/selected-backup.vcpcfg';
  const backup = { domains: { server: { apiKey: 'imported-key' } }, exportedAt: '2026-10-18T00:00:00Z' };

  it('页面隐藏时取消未交付picker，迟到文件完成解析清理但不重新打开确认', async () => {
    const picked = deferred<{ path: string }>();
    mockInvoke('plugin:vcp-mobile|pick_file', () => picked.promise);
    mockInvoke('parse_config_backup', () => backup);
    const applyImport = vi.fn(async () => ({ applied: ['服务器与同步连接'], skipped: [], failed: [] }));
    const wrapper = render(ConfigBackupSection, { active: true, disabled: false, commit: async () => saved(), applyImport });

    await action(wrapper, '导入').get('button').trigger('click');
    const operationId = callsFor('plugin:vcp-mobile|pick_file')[0]?.[1]?.operationId;
    expect(typeof operationId).toBe('string');
    await wrapper.setProps({ active: false });
    expect(callsFor('plugin:vcp-mobile|cancel_file_operation')).toContainEqual([
      'plugin:vcp-mobile|cancel_file_operation', { operationId },
    ]);
    picked.resolve({ path: filePath });
    await flushPromises();

    expect(deleteTempFile).toHaveBeenCalledWith(filePath);
    expect(wrapper.find('[role="dialog"]').exists()).toBe(false);
    expect(harness.modals.size).toBe(0);
    expect(applyImport).not.toHaveBeenCalled();
  });

  it('active=false在退场仍挂载时关闭确认并注销其modal，不应用导入', async () => {
    mockInvoke('plugin:vcp-mobile|pick_file', () => ({ path: filePath }));
    mockInvoke('parse_config_backup', () => backup);
    const applyImport = vi.fn(async () => ({ applied: ['服务器与同步连接'], skipped: [], failed: [] }));
    const wrapper = render(ConfigBackupSection, { active: true, disabled: false, commit: async () => saved(), applyImport });

    await action(wrapper, '导入').get('button').trigger('click');
    await flushPromises();
    expect(wrapper.findComponent(VcpConfirm).props('isOpen')).toBe(true);
    expect(harness.modals.size).toBe(1);
    await wrapper.setProps({ active: false });

    expect(wrapper.find('[role="dialog"]').exists()).toBe(false);
    expect(harness.modals.size).toBe(0);
    expect(applyImport).not.toHaveBeenCalled();
  });

  it('确认导入只把已验证patch交给页面，不通过普通保存顺带提交草稿', async () => {
    mockInvoke('plugin:vcp-mobile|pick_file', () => ({ path: filePath }));
    mockInvoke('parse_config_backup', () => backup);
    const commit = vi.fn(async () => saved());
    const applyImport = vi.fn(async () => ({ applied: ['服务器与同步连接'], skipped: [], failed: [] }));
    const wrapper = render(ConfigBackupSection, { active: true, disabled: false, commit, applyImport });

    await action(wrapper, '导入').get('button').trigger('click');
    await flushPromises();
    await confirmImport(wrapper);
    await flushPromises();

    expect(applyImport).toHaveBeenCalledExactlyOnceWith(backup);
    expect(commit).not.toHaveBeenCalled();
    expect(deleteTempFile).toHaveBeenCalledWith(filePath);
  });

  it('原生解析交付的坏偏好在确认前回退默认，不跳过合法兄弟字段', async () => {
    mockInvoke('plugin:vcp-mobile|pick_file', () => ({ path: filePath }));
    mockInvoke('parse_config_backup', () => ({
      domains: {}, exportedAt: '', preferences: {
        theme: { themeKey: 'removed-theme.ts', mode: 'dark' },
        chat: { smoothStreamingEnabled: 'true', messageHapticsEnabled: false },
      },
    }));
    const applyImport = vi.fn(async () => ({ applied: ['主题与深浅模式', '聊天偏好'], skipped: [], failed: [] }));
    const wrapper = render(ConfigBackupSection, { active: true, disabled: false, commit: async () => saved(), applyImport });
    await action(wrapper, '导入').get('button').trigger('click');
    await flushPromises();
    expect(wrapper.findComponent(VcpConfirm).props('message')).toContain('主题与深浅模式、聊天偏好');
    expect(wrapper.findComponent(VcpConfirm).props('message')).not.toContain('不可用');
    await confirmImport(wrapper);
    await flushPromises();
    expect(applyImport).toHaveBeenCalledExactlyOnceWith({
      domains: {}, exportedAt: '', preferences: {
        theme: { themeKey: THEME_PREFERENCE_DEFAULTS.themeKey, mode: 'dark' },
        chat: { smoothStreamingEnabled: false, messageHapticsEnabled: false },
      },
    });
    expect(deleteTempFile).toHaveBeenCalledWith(filePath);
  });
});

describe('连接设置页与真实编辑会话的集成', () => {
  function renderPage(initial = makeSettings()) {
    let persisted = snapshot(initial);
    useSettingsStore().snapshot = persisted;
    mockInvoke('read_settings', () => persisted);
    mockInvoke('update_settings', (args) => {
      persisted = applyRequest(persisted, args?.request as ConfigUpdate);
      return { status: 'saved', snapshot: persisted };
    });
    mockInvoke('import_settings_backup', (args) => {
      persisted = applyRequest(persisted, args?.request as ConfigUpdate);
      return { result: { status: 'saved', snapshot: persisted }, missingGroups: false, missingAgents: false };
    });
    return render(ConnectionSettingsPage, { active: true });
  }

  it.each([
    ['Chat 服务器 URL (HTTP/HTTPS)', 'server.url'],
    ['HTTP 服务 URL', 'sync.httpUrl'],
  ] as const)('%s 失焦即保存；未改变的再次失焦不重复写入', async (label, path) => {
    const wrapper = renderPage();
    await flushPromises();
    await field(wrapper, label).setValue('https://new.example');
    expect(callsFor('update_settings')).toHaveLength(0);
    await field(wrapper, label).trigger('blur');
    await flushPromises();

    expect(readSetting(useSettingsStore().settings!, path)).toBe('https://new.example');
    expect(callsFor('update_settings')).toHaveLength(1);
    await field(wrapper, label).trigger('blur');
    await flushPromises();
    expect(callsFor('update_settings')).toHaveLength(1);
  });

  it('失焦保存期间继续编辑，单次打开面板等待最后一批连接配置保存', async () => {
    const wrapper = renderPage();
    await flushPromises();
    const store = useSettingsStore();
    let committed = cloneConfig(store.snapshot!);
    const firstSave = deferred<ConfigSaveResult>();
    const secondSave = deferred<ConfigSaveResult>();
    let nextSave = firstSave;
    mockInvoke('update_settings', () => {
      const current = nextSave;
      nextSave = secondSave;
      return current.promise;
    });
    let openedWith: AppSettings | undefined;
    harness.openSyncSession.mockImplementation(() => { openedWith = cloneConfig(store.settings!); });

    await field(wrapper, 'HTTP 服务 URL').setValue('https://sync.example');
    await field(wrapper, 'HTTP 服务 URL').trigger('blur');
    await flushPromises();
    expect(callsFor('update_settings')).toHaveLength(1);
    expect(wrapper.attributes()).not.toHaveProperty('inert');

    await field(wrapper, 'WebSocket 服务 URL').setValue('wss://sync.example');
    await field(wrapper, 'WebSocket 服务 URL').trigger('blur');
    await field(wrapper, 'Mobile Sync Token').setValue('new-token');
    await wrapper.findAll('[role="radio"]').find(button => button.text().includes('VCP 增强 Chat'))!.trigger('click');
    await wrapper.findAll('button').find(button => button.text() === 'OPEN PANEL')!.trigger('click');
    await flushPromises();
    expect(wrapper.attributes()).toHaveProperty('inert');
    expect(harness.openSyncSession).not.toHaveBeenCalled();
    expect(callsFor('update_settings')).toHaveLength(1);

    committed = applyRequest(committed, callsFor('update_settings')[0]![1]!.request as ConfigUpdate);
    firstSave.resolve({ status: 'saved', snapshot: committed });
    await flushPromises();
    expect(callsFor('update_settings')).toHaveLength(2);
    expect(harness.openSyncSession).not.toHaveBeenCalled();
    committed = applyRequest(committed, callsFor('update_settings')[1]![1]!.request as ConfigUpdate);
    secondSave.resolve({ status: 'saved', snapshot: committed });
    await flushPromises();

    expect(harness.openSyncSession).toHaveBeenCalledTimes(1);
    expect(openedWith?.sync).toMatchObject({ httpUrl: 'https://sync.example', serverUrl: 'wss://sync.example', token: 'new-token' });
    expect(openedWith?.server.endpointMode).toBe('vcpTools');
    expect(wrapper.attributes()).not.toHaveProperty('inert');
  });

  it('失焦保存被拒绝时保留下一输入的焦点；打开同步面板也不能沿用旧配置', async () => {
    const wrapper = renderPage(makeSettings({ 'log.url': 'wss://old.example', 'log.key': 'saved-key' }));
    document.body.append(wrapper.element);
    await flushPromises();
    const store = useSettingsStore();
    const committed = cloneConfig(store.snapshot!);
    const rejected: ConfigSaveResult = {
      status: 'rejected', issues: [{
        domain: 'log', field: 'url', code: 'invalidScheme',
        message: 'WebSocket 地址必须以 ws:// 或 wss:// 开头',
      }],
    };
    const pending = deferred<ConfigSaveResult>();
    mockInvoke('update_settings', () => pending.promise);
    const url = field(wrapper, 'VCP WebSocket 服务器 URL');
    const nextInput = field(wrapper, 'Mobile Sync Token').element as HTMLInputElement;
    (url.element as HTMLInputElement).focus();
    await url.setValue('example.invalid');
    nextInput.focus();
    await flushPromises();
    expect(callsFor('update_settings')).toHaveLength(1);
    pending.resolve(rejected);
    await flushPromises();

    expect(wrapper.get('[role="alert"]').text()).toContain('WebSocket 地址必须以 ws:// 或 wss:// 开头');
    expect(document.activeElement).toBe(nextInput);
    expect(callsFor('update_settings')).toHaveLength(1);
    expect(store.snapshot).toEqual(committed);
    await wrapper.findAll('button').find(button => button.text() === 'OPEN PANEL')!.trigger('click');
    await flushPromises();
    expect(harness.openSyncSession).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(wrapper.get('[role="alert"]').element);
    expect((url.element as HTMLInputElement).value).toBe('example.invalid');
    expect(wrapper.attributes()).not.toHaveProperty('inert');
  });

  it('无连接草稿时打开同步面板查看历史，不产生配置写入', async () => {
    const wrapper = renderPage();
    await flushPromises();
    await wrapper.findAll('button').find(button => button.text() === 'OPEN PANEL')!.trigger('click');
    await flushPromises();
    expect(harness.openSyncSession).toHaveBeenCalledTimes(1);
    expect(callsFor('update_settings')).toHaveLength(0);
  });

  it('离页保存被拒绝时聚焦字段错误并保留草稿与已保存配置，修正后可保存离开', async () => {
    const initial = makeSettings({ 'log.url': 'wss://old.example', 'log.key': 'saved-key' });
    const wrapper = renderPage(initial);
    document.body.append(wrapper.element);
    await flushPromises();
    const scroll = vi.spyOn(HTMLElement.prototype, 'scrollIntoView').mockImplementation(() => {});
    const store = useSettingsStore();
    const committed = cloneConfig(store.snapshot!);
    const rejected: ConfigSaveResult = {
      status: 'rejected', issues: [{
        domain: 'log', field: 'url', code: 'invalidScheme',
        message: 'WebSocket 地址必须以 ws:// 或 wss:// 开头',
      }],
    };
    let rejectNext = true;
    mockInvoke('update_settings', (args) => {
      if (rejectNext) {
        rejectNext = false;
        return rejected;
      }
      return { status: 'saved', snapshot: applyRequest(committed, args?.request as ConfigUpdate) };
    });
    await field(wrapper, 'VCP WebSocket 服务器 URL').setValue('example.invalid');
    const footerAction = action(wrapper, '导出').get('button').element as HTMLButtonElement;
    footerAction.focus();
    expect(document.activeElement).toBe(footerAction);

    const page = wrapper.vm as unknown as { beforeLeave(): Promise<boolean> };
    expect(await page.beforeLeave()).toBe(false);
    await flushPromises();

    const summary = wrapper.get('[role="alert"]');
    expect(summary.text()).toBe('日志连接 · 日志服务器地址：WebSocket 地址必须以 ws:// 或 wss:// 开头');
    expect(summary.attributes('tabindex')).toBe('-1');
    expect(document.activeElement).toBe(summary.element);
    expect(scroll.mock.contexts).toContain(summary.element);
    expect(scroll).toHaveBeenCalledWith({ block: 'nearest' });
    expect(wrapper.attributes()).not.toHaveProperty('inert');
    expect((field(wrapper, 'VCP WebSocket 服务器 URL').element as HTMLInputElement).value).toBe('example.invalid');
    expect(store.snapshot).toEqual(committed);
    expect(useNotificationStore().activeToasts.some(notice => notice.title === '设置已保存')).toBe(false);

    await field(wrapper, 'VCP WebSocket 服务器 URL').setValue('wss://new.example:8443');
    expect(await page.beforeLeave()).toBe(true);
    await flushPromises();

    expect(store.settings?.log).toEqual({ url: 'wss://new.example:8443', key: 'saved-key' });
    expect(wrapper.find('[role="alert"]').exists()).toBe(false);
    expect(callsFor('update_settings')).toHaveLength(2);
    expect(callsFor('update_settings')[1]?.[1]).toEqual({ request: { patches: [{
      domain: 'log', expectedRevision: '1', changes: { url: 'wss://new.example:8443' },
      baseValues: { url: 'wss://old.example' },
    }] } });
  });

  it('连接页的字段拒绝阻止模型诊断，并在动作解锁后呈现可访问错误', async () => {
    const wrapper = renderPage(makeSettings({ 'log.url': 'wss://old.example', 'log.key': 'saved-key' }));
    document.body.append(wrapper.element);
    await flushPromises();
    const committed = cloneConfig(useSettingsStore().snapshot!);
    mockInvoke('update_settings', () => ({
      status: 'rejected', issues: [{
        domain: 'log', field: 'url', code: 'fragmentNotAllowed',
        message: 'WebSocket 地址不能包含 #fragment',
      }],
    }));
    await field(wrapper, 'VCP WebSocket 服务器 URL').setValue('wss://new.example#fragment');
    await action(wrapper, '检查模型发现').get('button').trigger('click');
    await flushPromises();

    expect(callsFor('test_vcp_connection')).toHaveLength(0);
    expect(callsFor('update_settings')).toHaveLength(1);
    expect(useSettingsStore().snapshot).toEqual(committed);
    expect((field(wrapper, 'VCP WebSocket 服务器 URL').element as HTMLInputElement).value).toBe('wss://new.example#fragment');
    expect(wrapper.attributes()).not.toHaveProperty('inert');
    expect(document.activeElement).toBe(wrapper.get('[role="alert"]').element);
    expect(wrapper.get('[role="alert"]').text()).toContain('日志服务器地址：WebSocket 地址不能包含 #fragment');
  });

  it('导入只提交备份patch并接受覆盖字段；其他未保存草稿保留到离开时提交', async () => {
    mockInvoke('plugin:vcp-mobile|pick_file', () => ({ path: 'tests/fixtures/connection.vcpcfg' }));
    mockInvoke('parse_config_backup', () => ({ domains: { server: { apiKey: 'imported-key' } }, exportedAt: '' }));
    const wrapper = renderPage();
    await flushPromises();
    await field(wrapper, 'API Key').setValue('unsubmitted-key');
    await field(wrapper, 'Mobile Sync Token').setValue('unsubmitted-token');
    expect(callsFor('update_settings')).toHaveLength(0);

    await action(wrapper, '导入').get('button').trigger('click');
    await flushPromises();
    await confirmImport(wrapper);
    await flushPromises();

    expect(callsFor('import_settings_backup')).toEqual([['import_settings_backup', { request: { patches: [{ domain: 'server', expectedRevision: '1', changes: { apiKey: 'imported-key' }, baseValues: { apiKey: 'draft-key' } }] }, groupPreferences: {}, agentPreferences: {} }]]);
    expect((field(wrapper, 'API Key').element as HTMLInputElement).value).toBe('imported-key');
    expect((field(wrapper, 'Mobile Sync Token').element as HTMLInputElement).value).toBe('unsubmitted-token');
    expect(useSettingsStore().settings?.sync.token).toBe('');

    expect(await (wrapper.vm as unknown as { beforeLeave(): Promise<boolean> }).beforeLeave()).toBe(true);
    const updates = callsFor('update_settings');
    expect(updates[updates.length - 1]).toEqual(['update_settings', { request: { patches: [{ domain: 'sync', expectedRevision: '1', changes: { token: 'unsubmitted-token' }, baseValues: { token: '' } }] } }]);
  });

  it('群聊设置按字段恢复，确认后原样提交部分字段并刷新群列表', async () => {
    const groupPreferences = { 'group-a': { modeSettings: { jev: { historyWindow: 25 } } } };
    mockInvoke('plugin:vcp-mobile|pick_file', () => ({ path: 'tests/fixtures/group-partial.vcpcfg' }));
    mockInvoke('parse_config_backup', () => ({ domains: {}, groupPreferences, exportedAt: '' }));
    mockInvoke('get_assistants_snapshot', () => ({ agents: [], groups: [], unreadCounts: {} }));
    const wrapper = renderPage();
    await flushPromises();
    await action(wrapper, '导入').get('button').trigger('click');
    await flushPromises();
    await confirmImport(wrapper);
    await flushPromises();
    expect(callsFor('import_settings_backup')).toEqual([
      ['import_settings_backup', { request: { patches: [] }, groupPreferences, agentPreferences: {} }],
    ]);
    expect(callsFor('get_assistants_snapshot')).toHaveLength(1);
    expect(wrapper.text()).toContain('配置导入成功');
  });

  it('缺失群暂存待同步，其他设置照常恢复且不逐群展示', async () => {
    const groupPreferences = { 'missing-a': { chatMode: 'jev' }, 'missing-b': { chatMode: 'sequential' } };
    mockInvoke('plugin:vcp-mobile|pick_file', () => ({ path: 'tests/fixtures/groups-before-sync.vcpcfg' }));
    mockInvoke('parse_config_backup', () => ({ domains: { server: { apiKey: 'restored-key' } }, groupPreferences, exportedAt: '' }));
    mockInvoke('get_assistants_snapshot', () => ({ agents: [], groups: [], unreadCounts: {} }));
    const wrapper = renderPage();
    mockInvoke('import_settings_backup', (args) => ({
      result: { status: 'saved', snapshot: applyRequest(snapshot(), args?.request as ConfigUpdate) },
      missingGroups: true,
      missingAgents: false,
    }));
    await flushPromises();
    await action(wrapper, '导入').get('button').trigger('click');
    await flushPromises();
    await confirmImport(wrapper);
    await flushPromises();
    expect(useSettingsStore().settings?.server.apiKey).toBe('restored-key');
    expect(wrapper.text().match(/请先同步，再次导入/g)).toHaveLength(1);
    expect(wrapper.text()).not.toContain('missing-a');
    expect(wrapper.text()).not.toContain('missing-b');
    expect(callsFor('import_settings_backup')).toHaveLength(1);
    expect(readStagedEntityBackup()?.groupPreferences).toEqual(groupPreferences);
  });

  it.each(['agent', 'group'] as const)('页面导入完整实体和独立数据，校准当前 %s 并暂存未同步类别', async ownerType => {
    const agentPreferences = { owner: { mobileSystemPrompt: 'restored prompt' } };
    const groupPreferences = { owner: { contextMessageWindowSize: 60 } };
    const standaloneData = { modelFavorites: ['fixture-model'], tarvenRules: [] };
    mockInvoke('plugin:vcp-mobile|pick_file', () => ({ path: 'tests/fixtures/entities.vcpcfg' }));
    mockInvoke('parse_config_backup', () => ({ domains: {}, agentPreferences, groupPreferences, standaloneData, exportedAt: '' }));
    const wrapper = renderPage();
    await flushPromises();
    const assistants = useAssistantStore();
    assistants.agents = [{ id: 'owner', name: 'Old agent', model: 'fixture-model', avatarCalculatedColor: null }];
    assistants.groups = [{ id: 'owner', name: 'Old group', members: [], mode: 'sequential', avatarCalculatedColor: null }];
    const focus = { target: { kind: 'topic', ownerType, ownerId: 'owner', topicId: 'active-topic' },
      focusRevision: 1, windowRevision: 1, anchorId: 'kept-anchor' };
    mockInvoke('connect_chat_view', () => ({ documentId: 1, focus }));
    mockInvoke('restore_chat_view', () => ({ status: 'accepted', focus }));
    const session = useChatSessionStore();
    await session.restoreFocus();
    const conversation = session.currentConversationKey;
    const window = session.acceptedWindow;
    mockInvoke('import_settings_backup', () => ({
      result: { status: 'saved', snapshot: useSettingsStore().snapshot }, missingGroups: false, missingAgents: true,
    }));
    mockInvoke('get_assistants_snapshot', () => ({
      agents: [{ ...assistants.agents[0], name: 'Refreshed agent' }],
      groups: [{ ...assistants.groups[0], name: 'Refreshed group' }], unreadCounts: {},
    }));

    await action(wrapper, '导入').get('button').trigger('click');
    await flushPromises();
    await confirmImport(wrapper);
    await flushPromises();

    expect(callsFor('import_settings_backup')).toEqual([['import_settings_backup', {
      request: { patches: [] }, agentPreferences, groupPreferences, standaloneData,
    }]]);
    expect(session.currentSelectedItem?.name).toBe(`Refreshed ${ownerType}`);
    expect(session.currentConversationKey).toBe(conversation);
    expect(session.acceptedWindow).toBe(window);
    expect(readStagedEntityBackup()?.agentPreferences).toEqual(agentPreferences);
    expect(readStagedEntityBackup()?.groupPreferences).toBeUndefined();
  });

  it('导入应用期间冻结本页动作；失败后恢复输入并保留原草稿', async () => {
    mockInvoke('plugin:vcp-mobile|pick_file', () => ({ path: 'tests/fixtures/connection.vcpcfg' }));
    mockInvoke('parse_config_backup', () => ({ domains: { server: { apiKey: 'imported-key' } }, exportedAt: '' }));
    const wrapper = renderPage();
    await flushPromises();
    await field(wrapper, 'API Key').setValue('unsubmitted-key');
    const updating = deferred<ConfigBackupImportResult>();
    mockInvoke('import_settings_backup', () => updating.promise);
    await action(wrapper, '导入').get('button').trigger('click');
    await flushPromises();
    await confirmImport(wrapper);
    await flushPromises();

    expect(wrapper.attributes()).toHaveProperty('inert');
    expect(await (wrapper.vm as unknown as { beforeLeave(): Promise<boolean> }).beforeLeave()).toBe(false);
    expect(callsFor('import_settings_backup')).toHaveLength(1);
    updating.reject(new Error('导入写入失败'));
    await flushPromises();

    expect(wrapper.attributes()).not.toHaveProperty('inert');
    expect((field(wrapper, 'API Key').element as HTMLInputElement).value).toBe('unsubmitted-key');
    expect(wrapper.text()).toContain('设置保存失败');
  });

  it('导出捕获期间冻结输入，文件写入等待期间恢复编辑且不改变已编码快照', async () => {
    const wrapper = renderPage();
    await flushPromises();
    await field(wrapper, 'API Key').setValue('export-this-key');
    const updating = deferred<ConfigSaveResult>();
    const exporting = deferred<{ displayName: string }>();
    mockInvoke('update_settings', () => updating.promise);
    mockInvoke('plugin:vcp-mobile|save_to_downloads', () => exporting.promise);

    await action(wrapper, '导出').get('button').trigger('click');
    await flushPromises();
    expect(wrapper.attributes()).toHaveProperty('inert');
    expect(callsFor('plugin:vcp-mobile|save_to_downloads')).toHaveLength(0);
    mockInvoke('export_config_domains', () => ({ server: { apiKey: 'export-this-key' } }));
    updating.resolve({ status: 'saved', snapshot: snapshot(makeSettings({ 'server.apiKey': 'export-this-key' }), '2') });
    await flushPromises();
    expect(wrapper.attributes()).not.toHaveProperty('inert');
    await field(wrapper, 'API Key').setValue('edit-after-export-started');

    const args = callsFor('plugin:vcp-mobile|save_to_downloads')[0]?.[1];
    const payload = JSON.parse(new TextDecoder().decode(Uint8Array.from(atob(args!.contentBase64 as string), character => character.charCodeAt(0))));
    expect(payload.domains.server.apiKey).toBe('export-this-key');
    expect(callsFor('update_settings')).toHaveLength(1);
    exporting.resolve({ displayName: 'backup.vcpcfg' });
    await flushPromises();
    expect((field(wrapper, 'API Key').element as HTMLInputElement).value).toBe('edit-after-export-started');
  });
});

describe('高级设置页的保存与动作边界', () => {
  function renderPage() {
    const initial = snapshot();
    useSettingsStore().snapshot = initial;
    mockInvoke('read_settings', () => initial);
    return render(AdvancedSettingsPage, { active: true });
  }

  it('外部正式快照更新总结模型时只重基展示，不产生一次用户保存', async () => {
    const wrapper = renderPage();
    await flushPromises();
    useSettingsStore().snapshot = snapshot(makeSettings({ 'conversation.topicSummaryModel': 'externally-updated-model' }), '2');
    await flushPromises();

    expect((field(wrapper, '总结专用模型').element as HTMLInputElement).value).toBe('externally-updated-model');
    expect(callsFor('update_settings')).toHaveLength(0);
  });

  it('离开时等待总结保存完成，期间禁止启动新的缓存维护动作', async () => {
    const wrapper = renderPage();
    await flushPromises();
    const updating = deferred<ConfigSaveResult>();
    mockInvoke('update_settings', () => updating.promise);
    await field(wrapper, '总结专用模型').setValue('new-summary-model');
    await flushPromises();
    const leaving = (wrapper.vm as unknown as { beforeLeave(): Promise<boolean> }).beforeLeave();
    await flushPromises();

    const rebuildButton = action(wrapper, '开始刷新').get('button');
    const clearButton = action(wrapper, '立即清理').get('button');
    expect(rebuildButton.attributes()).toHaveProperty('disabled');
    expect(clearButton.attributes()).toHaveProperty('disabled');
    await rebuildButton.trigger('click');
    await clearButton.trigger('click');
    expect(harness.openRebuildSession).not.toHaveBeenCalled();
    expect(callsFor('clear_webview_cache')).toHaveLength(0);

    updating.resolve({ status: 'saved', snapshot: snapshot(makeSettings({ 'conversation.topicSummaryModel': 'new-summary-model' }), '2') });
    expect(await leaving).toBe(true);
  });
});

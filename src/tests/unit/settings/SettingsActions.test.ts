import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { flushPromises, mount, type VueWrapper } from '@vue/test-utils';
import type { Component } from 'vue';
import { deleteTempFile } from 'tauri-plugin-vcp-mobile';
import { useSettingsStore, type AppSettings } from '../../../core/stores/settings';
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
import type { ConfigSnapshot, ConfigSaveResult, ConfigUpdate } from '../../../core/config/generated/config';
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
  return { data, domains: Object.fromEntries(Object.keys(data).map(domain => [domain, { revision, schemaVersion: 1, issues: [], health: 'ready' }])) as unknown as ConfigSnapshot['domains'] };
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
  setActivePinia(createPinia());
  mockInvoke('preview_chat_endpoint', () => ({ finalUrl: 'https://draft.example/v1/chat/completions', modelDiscoveryUrl: 'https://draft.example/v1/models' }));
  mockInvoke('test_vcp_connection', () => ({ success: true, status: 200, modelCount: 2, modelDiscoveryAvailable: true }));
  mockInvoke('regenerate_emoticon_library', () => 2);
  mockInvoke('plugin:vcp-mobile|save_to_downloads', () => ({ displayName: 'backup.vcpcfg' }));
  mockInvoke('export_config_domains', () => ({ identity: { userName: '已提交用户' }, server: { apiKey: 'saved-key' }, conversation: { topicSummaryModel: 'summary-model' } }));
});

afterEach(() => {
  for (const wrapper of wrappers.splice(0)) wrapper.unmount();
  harness.modals.clear();
});

describe('依赖配置提交的设置动作', () => {
  const cases = [
    { component: VcpCoreSettingsSection, label: '检查模型发现', command: 'test_vcp_connection' },
    { component: SyncSettingsSection, label: 'REFRESH', command: 'regenerate_emoticon_library' },
    { component: ConfigBackupSection, label: '导出', command: 'plugin:vcp-mobile|save_to_downloads' },
  ];

  for (const { component, label, command } of cases) {
    it(`${label}：保存失败保留错误，不执行后续任务`, async () => {
      const commit = vi.fn(async (): Promise<SettingsSaveResult> => ({ ok: false, error: '磁盘不可写' }));
      const wrapper = render(component, { settings: connectionDraft(), active: true, disabled: false, commit, applyImport: vi.fn() });

      await action(wrapper, label).get('button').trigger('click');
      await flushPromises();

      expect(commit).toHaveBeenCalledTimes(1);
      expect(callsFor(command)).toHaveLength(0);
      expect(wrapper.text()).toContain('磁盘不可写');
    });

    it(`${label}：保存等待中隐藏页面，不再启动后续任务`, async () => {
      const pending = deferred<SettingsSaveResult>();
      const commit = vi.fn(() => pending.promise);
      const wrapper = render(component, { settings: connectionDraft(), active: true, disabled: false, commit, applyImport: vi.fn() });

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

  it('导出编码保存成功的快照和正式偏好，并排除非备份字段', async () => {
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
    expect(payload.domains.conversation.topicSummaryModel).toBe('summary-model');
    expect(payload.preferences.theme.themeKey).toBeTruthy();
    expect(payload.preferences.chat).toHaveProperty('messageHapticsEnabled');
    expect(payload.domains).not.toHaveProperty('navigation');
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

  it('打开同步面板不提交与该操作无关的表单', async () => {
    const commit = vi.fn(async () => saved());
    const wrapper = render(SyncSettingsSection, { settings: connectionDraft(), active: true, disabled: false, commit });
    const button = wrapper.findAll('button').find(candidate => candidate.text() === 'OPEN PANEL');
    expect(button).toBeDefined();
    await button!.trigger('click');

    expect(harness.openSyncSession).toHaveBeenCalledTimes(1);
    expect(commit).not.toHaveBeenCalled();
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
    return render(ConnectionSettingsPage, { active: true });
  }

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

    expect(callsFor('update_settings')).toEqual([['update_settings', { request: { patches: [{ domain: 'server', expectedRevision: '1', changes: { apiKey: 'imported-key' }, baseValues: { apiKey: 'draft-key' }, repair: false }] } }]]);
    expect((field(wrapper, 'API Key').element as HTMLInputElement).value).toBe('imported-key');
    expect((field(wrapper, 'Mobile Sync Token').element as HTMLInputElement).value).toBe('unsubmitted-token');
    expect(useSettingsStore().settings?.sync.token).toBe('');

    expect(await (wrapper.vm as unknown as { beforeLeave(): Promise<boolean> }).beforeLeave()).toBe(true);
    const updates = callsFor('update_settings');
    expect(updates[updates.length - 1]).toEqual(['update_settings', { request: { patches: [{ domain: 'sync', expectedRevision: '1', changes: { token: 'unsubmitted-token' }, baseValues: { token: '' }, repair: false }] } }]);
  });

  it('导入应用期间冻结本页动作；失败后恢复输入并保留原草稿', async () => {
    mockInvoke('plugin:vcp-mobile|pick_file', () => ({ path: 'tests/fixtures/connection.vcpcfg' }));
    mockInvoke('parse_config_backup', () => ({ domains: { server: { apiKey: 'imported-key' } }, exportedAt: '' }));
    const wrapper = renderPage();
    await flushPromises();
    await field(wrapper, 'API Key').setValue('unsubmitted-key');
    const updating = deferred<ConfigSaveResult>();
    mockInvoke('update_settings', () => updating.promise);
    await action(wrapper, '导入').get('button').trigger('click');
    await flushPromises();
    await confirmImport(wrapper);
    await flushPromises();

    expect(field(wrapper, 'API Key').attributes()).toHaveProperty('disabled');
    expect(await (wrapper.vm as unknown as { beforeLeave(): Promise<boolean> }).beforeLeave()).toBe(false);
    expect(callsFor('update_settings')).toHaveLength(1);
    updating.reject(new Error('导入写入失败'));
    await flushPromises();

    expect(field(wrapper, 'API Key').attributes()).not.toHaveProperty('disabled');
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
    expect(field(wrapper, 'API Key').attributes()).toHaveProperty('disabled');
    expect(callsFor('plugin:vcp-mobile|save_to_downloads')).toHaveLength(0);
    mockInvoke('export_config_domains', () => ({ server: { apiKey: 'export-this-key' } }));
    updating.resolve({ status: 'saved', snapshot: snapshot(makeSettings({ 'server.apiKey': 'export-this-key' }), '2') });
    await flushPromises();
    expect(field(wrapper, 'API Key').attributes()).not.toHaveProperty('disabled');
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

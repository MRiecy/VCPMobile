import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, disposePinia, setActivePinia, type Pinia } from 'pinia';
import { flushPromises, mount, type VueWrapper } from '@vue/test-utils';
import JevSettingsPage from '../../../features/settings/pages/JevSettingsPage.vue';
import SettingsRow from '../../../components/settings/SettingsRow.vue';
import SettingsTextField from '../../../components/settings/SettingsTextField.vue';
import SettingsDisclosure from '../../../components/settings/SettingsDisclosure.vue';
import { useOverlayStore } from '../../../core/stores/overlay';
import { useSettingsStore } from '../../../core/stores/settings';
import { applySettingsFields } from '../../../core/config/fields';
import type { ConfigUpdate } from '../../../core/config/generated/config';
import { makeSettings, makeConfigSnapshot, configRequestChanges } from '../../helpers/config';
import { invokeMock, mockInvoke } from '../../mocks/tauri';

vi.mock('../../../core/stores/notification', () => ({ useNotificationStore: () => ({ addNotification: vi.fn() }) }));

let pinia: Pinia;
let wrappers: VueWrapper[] = [];
beforeEach(() => {
  pinia = createPinia();
  setActivePinia(pinia);
  let settings = makeSettings();
  mockInvoke('read_settings', () => makeConfigSnapshot(settings));
  mockInvoke('update_settings', args => {
    settings = applySettingsFields(settings, configRequestChanges(args?.request as ConfigUpdate));
    return { status: 'saved', snapshot: makeConfigSnapshot(settings, '2') };
  });
});
afterEach(() => {
  wrappers.forEach(wrapper => wrapper.unmount());
  wrappers = [];
  disposePinia(pinia);
  vi.restoreAllMocks();
});

async function renderPage() {
  const wrapper = mount(JevSettingsPage, { props: { active: true } });
  wrappers.push(wrapper);
  await flushPromises();
  return wrapper;
}
function field(wrapper: VueWrapper, label: string) {
  const component = wrapper.findAllComponents(SettingsTextField).find(item => item.props('label') === label);
  if (!component) throw new Error(`Missing JEV field: ${label}`);
  return component.get('input');
}
async function openProvider(wrapper: VueWrapper) {
  const row = wrapper.findAllComponents(SettingsRow).find(item => item.props('title') === '服务提供商');
  if (!row) throw new Error('Missing provider selector');
  await row.trigger('click');
  return useOverlayStore().contextMenuConfig!;
}
async function choose(wrapper: VueWrapper, provider: string) {
  const menu = await openProvider(wrapper);
  const action = menu.actions.find(item => item.label === provider);
  if (!action) throw new Error(`Missing JEV provider: ${provider}`);
  await action.handler?.();
  useOverlayStore().closeContextMenu();
  await flushPromises();
}
async function openAppInfo(wrapper: VueWrapper) {
  await wrapper.findAllComponents(SettingsDisclosure).find(item => item.props('title') === '应用信息')!.get('button').trigger('click');
}

describe('JEV 设置由同一领域编辑会话保存', () => {
  it('切换服务商分别保留地址、密钥和模型，返回时一次提交', async () => {
    const wrapper = await renderPage();
    expect(field(wrapper, 'API 地址').element).toHaveProperty('value', '');
    expect(field(wrapper, '决策模型').element).toHaveProperty('value', '');
    await field(wrapper, 'API 地址').setValue('https://official.example/decision');
    await field(wrapper, '决策模型').setValue('custom-official-model');
    await field(wrapper, 'API Key（支持多个）').setValue('official-one，official-two');
    await choose(wrapper, 'OpenRouter');
    expect(field(wrapper, 'API 地址').element).toHaveProperty('value', '');
    expect(field(wrapper, '决策模型').element).toHaveProperty('value', '');
    await openAppInfo(wrapper);
    expect(field(wrapper, '应用名称').element).toHaveProperty('value', '');
    await field(wrapper, 'API Key（支持多个）').setValue('router-key');
    await field(wrapper, 'API 地址').setValue('https://router.example/decisions');
    await choose(wrapper, 'TypeSafe 官方');
    expect(field(wrapper, 'API 地址').element).toHaveProperty('value', 'https://official.example/decision');
    expect(field(wrapper, '决策模型').element).toHaveProperty('value', 'custom-official-model');
    expect(field(wrapper, 'API Key（支持多个）').element).toHaveProperty('value', 'official-one，official-two');
    expect(invokeMock.mock.calls.filter(([command]) => command === 'update_settings')).toHaveLength(0);

    expect(await (wrapper.vm as unknown as { beforeLeave(): Promise<boolean> }).beforeLeave()).toBe(true);
    const calls = invokeMock.mock.calls.filter(([command]) => command === 'update_settings');
    expect(calls).toHaveLength(1);
    expect(configRequestChanges(calls[0][1]!.request as ConfigUpdate)).toEqual({
      'jev.typesafe.apiKey': 'official-one，official-two',
      'jev.typesafe.apiUrl': 'https://official.example/decision',
      'jev.typesafe.model': 'custom-official-model',
      'jev.openrouter.apiKey': 'router-key',
      'jev.openrouter.apiUrl': 'https://router.example/decisions',
    });
    expect(useSettingsStore().settings?.jev.openrouter.apiKey).toBe('router-key');
    expect(useSettingsStore().settings?.jev.typesafe.apiKey).toBe('official-one，official-two');
  });

  it('清空已有地址、模型和应用名称后保存空值，切换不会填回预设', async () => {
    let settings = makeSettings({
      'jev.openrouter.apiUrl': 'https://custom.example/api',
      'jev.openrouter.apiKey': 'keep-key',
      'jev.openrouter.model': 'custom-model',
      'jev.appTitle': 'Custom App',
    });
    mockInvoke('read_settings', () => makeConfigSnapshot(settings));
    mockInvoke('update_settings', args => {
      settings = applySettingsFields(settings, configRequestChanges(args?.request as ConfigUpdate));
      return { status: 'saved', snapshot: makeConfigSnapshot(settings, '2') };
    });
    const wrapper = await renderPage();
    await choose(wrapper, 'OpenRouter');
    await field(wrapper, 'API 地址').setValue('');
    await field(wrapper, '决策模型').setValue('');
    await openAppInfo(wrapper);
    await field(wrapper, '应用名称').setValue('');
    await choose(wrapper, 'TypeSafe 官方');
    await choose(wrapper, 'OpenRouter');
    expect(field(wrapper, 'API 地址').element).toHaveProperty('value', '');
    expect(field(wrapper, '决策模型').element).toHaveProperty('value', '');
    expect(field(wrapper, 'API Key（支持多个）').element).toHaveProperty('value', 'keep-key');
    expect(await (wrapper.vm as unknown as { beforeLeave(): Promise<boolean> }).beforeLeave()).toBe(true);
    const call = invokeMock.mock.calls.find(([command]) => command === 'update_settings')!;
    expect(configRequestChanges(call[1]!.request as ConfigUpdate)).toEqual({
      'jev.provider': 'openrouter',
      'jev.openrouter.apiUrl': '',
      'jev.openrouter.model': '',
      'jev.appTitle': '',
    });
  });

  it('页面隐藏关闭自己的菜单，迟到选择不保存或改变正式配置', async () => {
    const wrapper = await renderPage();
    const menu = await openProvider(wrapper);
    const stale = menu.actions.find(item => item.label === 'OpenRouter')!;
    await wrapper.setProps({ active: false });
    expect(useOverlayStore().contextMenuConfig).toBeNull();
    await stale.handler?.();
    await flushPromises();
    expect(useSettingsStore().settings?.jev.provider).toBe('typesafe');
    expect(invokeMock.mock.calls.filter(([command]) => command === 'update_settings')).toHaveLength(0);
  });

  it('保存被拒绝时只显示实际字段错误，保留输入并允许修改后重试', async () => {
    const wrapper = await renderPage();
    await field(wrapper, 'API 地址').setValue('broken-address');
    mockInvoke('update_settings', () => ({ status: 'rejected', issues: [
      { domain: 'jev', field: 'typesafe.apiUrl', code: 'invalidUrl', message: '请填写有效地址。' },
    ] }));
    const page = wrapper.vm as unknown as { beforeLeave(): Promise<boolean> };
    expect(await page.beforeLeave()).toBe(false);
    await flushPromises();
    expect(wrapper.findAll('[role="alert"]')).toHaveLength(1);
    expect(wrapper.get('[role="alert"]').text()).toBe('JEV · TypeSafe API 地址：请填写有效地址。');
    expect(field(wrapper, 'API 地址').element).toHaveProperty('value', 'broken-address');
    expect(field(wrapper, 'API 地址').attributes('disabled')).toBeUndefined();

    await field(wrapper, 'API 地址').setValue('https://decision.example/v1');
    mockInvoke('update_settings', args => ({
      status: 'saved',
      snapshot: makeConfigSnapshot(applySettingsFields(useSettingsStore().settings!, configRequestChanges(args?.request as ConfigUpdate)), '2'),
    }));
    expect(await page.beforeLeave()).toBe(true);
    await flushPromises();
    expect(wrapper.find('[role="alert"]').exists()).toBe(false);
    expect(useSettingsStore().settings?.jev.typesafe.apiUrl).toBe('https://decision.example/v1');
  });
});

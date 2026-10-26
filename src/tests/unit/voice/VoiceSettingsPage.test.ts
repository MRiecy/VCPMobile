import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, disposePinia, setActivePinia, type Pinia } from 'pinia';
import { reactive } from 'vue';
import { DOMWrapper, flushPromises, mount, type VueWrapper } from '@vue/test-utils';
import VoiceSettingsPage from '../../../features/chat/voice/VoiceSettingsPage.vue';
import VoiceOptionsView from '../../../features/chat/voice/VoiceOptionsView.vue';
import SettingsTextField from '../../../components/settings/SettingsTextField.vue';
import GlobalOverlayManager from '../../../components/GlobalOverlayManager.vue';
import { useOverlayStore } from '../../../core/stores/overlay';
import { makeSettings, makeConfigSnapshot, configRequestChanges } from '../../helpers/config';
import { applySettingsFields, type SettingsFieldValues } from '../../../core/config/fields';
import type { ConfigUpdate, ConfigSaveResult } from '../../../core/config/generated/config';
import { useSettingsStore, type AppSettings } from '../../../core/stores/settings';
import { SETTINGS_DEFAULTS } from '../../../core/config/generated/defaults';
import { useModalHistory } from '../../../core/composables/useModalHistory';
import { emitTauriEvent, invokeMock, listenMock, mockInvoke } from '../../mocks/tauri';

const context = vi.hoisted(() => ({
  openStyleGuidance: vi.fn(),
  voice: {} as {
    active: boolean; playbackActive: boolean;
  },
}));
vi.mock('../../../core/stores/notification', () => ({ useNotificationStore: () => ({ addNotification: vi.fn() }) }));
vi.mock('../../../features/chat/voice/voiceCallStore', () => ({ useVoiceCallStore: () => context.voice }));

function snapshot(): AppSettings {
  return makeSettings({ 'identity.userName': 'user', 'server.url': 'https://chat.example', 'server.apiKey': 'chat-key' });
}
function saved(changes: Partial<SettingsFieldValues> = {}): ConfigSaveResult {
  database = applySettingsFields(database, changes);
  revision += 1;
  return { status: 'saved', snapshot: makeConfigSnapshot(database, String(revision)) };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(accept => { resolve = accept; });
  return { promise, resolve };
}

let pinia: Pinia;
let database: AppSettings;
let revision = 1;
const wrappers: VueWrapper[] = [];
const modals = useModalHistory();

beforeEach(() => {
  pinia = createPinia();
  setActivePinia(pinia);
  database = snapshot();
  revision = 1;
  context.openStyleGuidance.mockReset();
  vi.spyOn(useOverlayStore(), 'openVoiceStyleGuidance').mockImplementation(context.openStyleGuidance);
  context.voice = reactive({
    active: true, playbackActive: false,
  });
  mockInvoke('read_settings', () => makeConfigSnapshot(database, String(revision)));
  mockInvoke('update_settings', args => {
    return saved(configRequestChanges(args?.request as ConfigUpdate));
  });
  window.history.replaceState({ vcpRoot: true, vcpMain: true }, '', '#/chat');
  vi.spyOn(window.history, 'back').mockImplementation(() => {
    window.history.replaceState({ vcpRoot: true, vcpMain: true }, '');
    window.dispatchEvent(new PopStateEvent('popstate', { state: window.history.state }));
  });
  wrappers.push(mount(GlobalOverlayManager, {
    attachTo: document.body,
    global: {
      stubs: { Transition: true, ToastManager: true, RenderedImageViewer: true },
      directives: { guide: {} },
    },
  }));
});

afterEach(() => {
  wrappers.splice(0).reverse().forEach(wrapper => wrapper.unmount());
  useOverlayStore().closeContextMenu();
  useOverlayStore().closePrompt();
  modals.unregisterModal('Page:voiceSettings:');
  modals.unregisterModal('Page:voiceStyleGuidance:');
  modals.unregisterModal('Page:voiceRuleDetails:');
  disposePinia(pinia);
  vi.restoreAllMocks();
});

async function renderPage() {
  const wrapper = mount(VoiceSettingsPage, { props: { active: true }, attachTo: document.body });
  wrappers.push(wrapper);
  await flushPromises();
  return wrapper;
}

async function renderOverlay(mode: 'settings' | 'styleGuidance' = 'settings') {
  const fallbackClose = vi.fn();
  modals.registerModal(mode === 'settings' ? 'Page:voiceSettings:' : 'Page:voiceStyleGuidance:', fallbackClose);
  const wrapper = mount(VoiceOptionsView, {
    attachTo: document.body,
    props: { mode, isOpen: true, active: true, zIndex: 40 },
    global: { stubs: { SlidePage: { template: '<div><slot /></div>' } } },
  });
  wrappers.push(wrapper);
  await flushPromises();
  return { wrapper, fallbackClose };
}

function field(wrapper: VueWrapper, label: string) {
  const component = wrapper.findAllComponents(SettingsTextField).find(candidate => candidate.props('label') === label);
  if (!component) throw new Error(`Missing field: ${label}`);
  return component.get('input');
}

async function leaveVoiceSettings(wrapper: VueWrapper) {
  await wrapper.get('button[aria-label="返回，关闭语音设置"]').trigger('click');
  await flushPromises();
}
const updates = () => invokeMock.mock.calls.filter(([command]) => command === 'update_settings');

async function openSelection(wrapper: VueWrapper, label: string) {
  await wrapper.get(`button[aria-label="${label}"]`).trigger('click');
  await flushPromises();
  const surface = document.querySelector<HTMLElement>(`[role="dialog"][aria-label="${label}"]`);
  if (!surface) throw new Error(`Missing selection menu: ${label}`);
  return new DOMWrapper(surface);
}

async function choose(wrapper: VueWrapper, label: string, option: string) {
  const menu = await openSelection(wrapper, label);
  const item = menu.findAll('[role="radio"]').find(candidate => candidate.text() === option);
  if (!item) throw new Error(`Missing option: ${option}`);
  await item.trigger('click');
  await flushPromises();
  expect(useOverlayStore().contextMenuConfig).toBeNull();
}

function disclosure(wrapper: VueWrapper, title: string) {
  const button = wrapper.findAll('.settings-disclosure-toggle').find(candidate => candidate.text().startsWith(title));
  if (!button) throw new Error(`Missing disclosure: ${title}`);
  return button;
}

async function openCustomDuration(wrapper: VueWrapper, title: string) {
  await choose(wrapper, title, '自定义…');
  const surface = document.querySelector<HTMLElement>(`[role="dialog"][aria-label="自定义${title}（ms）"]`);
  if (!surface) throw new Error(`Missing custom duration prompt: ${title}`);
  return new DOMWrapper(surface);
}

async function confirmPrompt(prompt: DOMWrapper<Element>) {
  await prompt.findAll('button').find(button => button.text() === '确认')!.trigger('click');
  await flushPromises();
}

describe('语音设置使用正式设置快照', () => {
  it('展开高级参数后修改预设时长，在返回时提交草稿', async () => {
    const { wrapper } = await renderOverlay();
    const advanced = disclosure(wrapper, '高级参数');
    expect(advanced.attributes('aria-expanded')).toBe('false');
    expect(wrapper.find('button[aria-label="人声检测灵敏度"]').exists()).toBe(false);
    await advanced.trigger('click');
    await choose(wrapper, '人声检测灵敏度', '较高');
    await choose(wrapper, '说完等待时间', '800 ms');
    await choose(wrapper, '朗读时插话确认', '128 ms');
    expect(updates()).toHaveLength(0);
    await leaveVoiceSettings(wrapper);
    expect(configRequestChanges(updates()[0]?.[1]?.request as ConfigUpdate)).toEqual({
      'voice.vad.sensitivity': 'high', 'voice.vad.endSilenceMs': 800, 'voice.vad.playbackOnsetMs': 128,
    });
  });

  it('自定义时长保留整数原值、校验范围，非法输入留在弹窗且取消不改草稿', async () => {
    const { wrapper } = await renderOverlay();
    await disclosure(wrapper, '高级参数').trigger('click');
    const silencePrompt = await openCustomDuration(wrapper, '说完等待时间');
    expect(silencePrompt.get('input').attributes('inputmode')).toBe('numeric');
    expect((silencePrompt.get('input').element as HTMLInputElement).value).toBe('512');
    for (const invalid of ['', '31', '5001', '100.5', '1e3', '-1']) {
      await silencePrompt.get('input').setValue(invalid);
      await confirmPrompt(silencePrompt);
      expect(useOverlayStore().promptConfig).not.toBeNull();
      expect(silencePrompt.get('[role="alert"]').text()).toContain('32–5000 ms');
      expect(wrapper.get('button[aria-label="说完等待时间"]').text()).toContain('512 ms');
    }
    await silencePrompt.get('input').setValue('777');
    await confirmPrompt(silencePrompt);
    expect(useOverlayStore().promptConfig).toBeNull();
    expect(wrapper.get('button[aria-label="说完等待时间"]').text()).toContain('777 ms');
    const customMenu = await openSelection(wrapper, '说完等待时间');
    expect(customMenu.get('[aria-checked="true"]').text()).toBe('自定义…');
    useOverlayStore().closeContextMenu();
    await flushPromises();
    const onsetPrompt = await openCustomDuration(wrapper, '朗读时插话确认');
    await onsetPrompt.get('input').setValue('2001');
    await confirmPrompt(onsetPrompt);
    expect(onsetPrompt.get('[role="alert"]').text()).toContain('32–2000 ms');
    await onsetPrompt.get('input').setValue('241');
    await onsetPrompt.findAll('button').find(button => button.text() === '取消')!.trigger('click');
    await flushPromises();
    expect(useOverlayStore().promptConfig).toBeNull();
    expect(wrapper.get('button[aria-label="朗读时插话确认"]').text()).toContain('192 ms');
    await leaveVoiceSettings(wrapper);
    expect(configRequestChanges(updates()[0]?.[1]?.request as ConfigUpdate)).toEqual({ 'voice.vad.endSilenceMs': 777 });
  });

  it('自定义输入随访问失活关闭，迟到确认不能改写新访问，系统返回先关闭输入', async () => {
    const { wrapper } = await renderOverlay();
    await disclosure(wrapper, '高级参数').trigger('click');
    await openCustomDuration(wrapper, '朗读时插话确认');
    const stalePrompt = useOverlayStore().promptConfig!;
    expect(modals.modalStackLength()).toBe(2);
    await wrapper.setProps({ active: false });
    expect(useOverlayStore().promptConfig).toBeNull();
    await wrapper.setProps({ active: true });
    await flushPromises();
    await disclosure(wrapper, '高级参数').trigger('click');
    await openCustomDuration(wrapper, '朗读时插话确认');
    stalePrompt.onConfirm('1000');
    expect(wrapper.get('button[aria-label="朗读时插话确认"]').text()).toContain('192 ms');
    modals.closeTopModal();
    await flushPromises();
    expect(useOverlayStore().promptConfig).toBeNull();
    expect(wrapper.emitted('close')).toBeUndefined();
    expect(modals.modalStackLength()).toBe(1);
    expect(updates()).toHaveLength(0);
  });

  it('卸载不关闭其他入口替换的自定义输入弹窗', async () => {
    const wrapper = await renderPage();
    await disclosure(wrapper, '高级参数').trigger('click');
    await openCustomDuration(wrapper, '说完等待时间');
    const overlay = useOverlayStore();
    const stalePrompt = overlay.promptConfig!;
    overlay.openPrompt({ title: '其他输入', initialValue: '', placeholder: '', onConfirm: vi.fn() });
    const replacement = overlay.promptConfig;
    stalePrompt.onConfirm('900');
    expect(wrapper.get('button[aria-label="说完等待时间"]').text()).toContain('512 ms');
    wrappers.splice(wrappers.indexOf(wrapper), 1);
    wrapper.unmount();
    await flushPromises();
    expect(overlay.promptConfig).toBe(replacement);
  });

  it('默认选择 WAV，编码随本页草稿保存并可切回，不改变朗读配置', async () => {
    const { wrapper } = await renderOverlay();
    expect(wrapper.get('button[aria-label="语音输入编码"]').text()).toContain('WAV 无损');
    const menu = await openSelection(wrapper, '语音输入编码');
    expect(menu.get('[aria-checked="true"]').text()).toBe('WAV 无损');
    await menu.get('[aria-checked="false"]').trigger('click');
    await flushPromises();
    expect(useOverlayStore().contextMenuConfig).toBeNull();
    expect(updates()).toHaveLength(0);
    expect(useSettingsStore().settings?.voice.asr.inputEncoding).toBe('wav');
    await leaveVoiceSettings(wrapper);
    await flushPromises();
    expect(configRequestChanges(updates()[0]?.[1]?.request as ConfigUpdate)).toEqual({ 'voice.asr.inputEncoding': 'mp3_64' });
    expect(database.voice.asr.inputEncoding).toBe('mp3_64');
    expect(database.voice.tts.stream).toBe(true);
    expect(database.voice.tts.voice).toBe('mimo_default');
    expect(wrapper.emitted('close')).toHaveLength(1);
    await wrapper.setProps({ active: false });
    await wrapper.setProps({ active: true });
    await flushPromises();
    expect(wrapper.get('button[aria-label="语音输入编码"]').text()).toContain('MP3 64 kbps');
    await choose(wrapper, '语音输入编码', 'WAV 无损');
    await leaveVoiceSettings(wrapper);
    await flushPromises();
    expect(configRequestChanges(updates()[1]?.[1]?.request as ConfigUpdate)).toEqual({ 'voice.asr.inputEncoding': 'wav' });
    expect(database.voice.asr.inputEncoding).toBe('wav');
  });

  it('只提交本页dirty字段，两个流式开关互不改变，中文音色ID原样保存', async () => {
    const { wrapper } = await renderOverlay();
    await field(wrapper, '语音 API Key').setValue('voice-secret');
    await choose(wrapper, '识别语言', '中文');
    await choose(wrapper, '朗读音色', '白桦 · 中文男声');
    await wrapper.get('[role="switch"][aria-label="流式识别结果"]').trigger('click');
    expect(updates()).toHaveLength(0);
    expect(useSettingsStore().settings?.voice.connection.apiKey).toBe('');
    await leaveVoiceSettings(wrapper);
    await flushPromises();
    expect(configRequestChanges(updates()[0]?.[1]?.request as ConfigUpdate)).toEqual({
      'voice.connection.apiKey': 'voice-secret', 'voice.asr.language': 'zh', 'voice.tts.voice': '白桦', 'voice.asr.stream': false,
    });
    expect(database.voice.tts.stream).toBe(true);
    expect(database.server.apiKey).toBe('chat-key');
    await wrapper.setProps({ active: false });
    await wrapper.setProps({ active: true });
    await flushPromises();
    await wrapper.get('[role="switch"][aria-label="流式朗读"]').trigger('click');
    await leaveVoiceSettings(wrapper);
    await flushPromises();
    expect(configRequestChanges(updates()[1]?.[1]?.request as ConfigUpdate)).toEqual({ 'voice.tts.stream': false });
    expect(database.voice.asr.stream).toBe(false);
    expect(wrapper.text()).toContain('整段上传');
  });

  it('失活只丢弃未提交草稿，不暗中保存API Key与输入编码', async () => {
    const { wrapper } = await renderOverlay();
    await field(wrapper, '语音 API Key').setValue('unsubmitted-secret');
    await choose(wrapper, '语音输入编码', 'MP3 64 kbps');
    await openSelection(wrapper, '识别语言');
    const staleAction = useOverlayStore().contextMenuConfig!.actions[1];
    await wrapper.setProps({ active: false });
    await flushPromises();
    expect(useOverlayStore().contextMenuConfig).toBeNull();
    expect(updates()).toHaveLength(0);
    expect(database.voice.connection.apiKey).toBe('');
    expect(database.voice.asr.inputEncoding).toBe('wav');
    await wrapper.setProps({ active: true });
    await flushPromises();
    staleAction.handler();
    await flushPromises();
    expect(wrapper.get('button[aria-label="语音输入编码"]').text()).toContain('WAV 无损');
    expect(wrapper.get('button[aria-label="识别语言"]').text()).toContain('自动识别');
  });

  it('保存失败不回显或记录密钥，保留草稿供重试', async () => {
    const log = vi.spyOn(console, 'error').mockImplementation(() => {});
    mockInvoke('update_settings', () => Promise.reject(new Error('request rejected: voice-private-token')));
    const { wrapper } = await renderOverlay();
    await field(wrapper, '语音 API Key').setValue('voice-private-token');
    await leaveVoiceSettings(wrapper);
    await flushPromises();
    expect(wrapper.get('[role="alert"]').text()).toContain('设置保存失败');
    expect(wrapper.get('[role="alert"]').text()).not.toContain('voice-private-token');
    expect(JSON.stringify(log.mock.calls)).not.toContain('voice-private-token');
    expect((field(wrapper, '语音 API Key').element as HTMLInputElement).value).toBe('voice-private-token');
    expect(database.voice.connection.apiKey).toBe('');
    expect(wrapper.emitted('close')).toBeUndefined();
    expect(modals.modalStackLength()).toBe(1);
    mockInvoke('update_settings', args => {
      return saved(configRequestChanges(args?.request as ConfigUpdate));
    });
    await leaveVoiceSettings(wrapper);
    expect(database.voice.connection.apiKey).toBe('voice-private-token');
    expect(wrapper.emitted('close')).toHaveLength(1);
  });

  it('进入风格指导前等待当前草稿确定保存', async () => {
    const pending = deferred<ConfigSaveResult>();
    mockInvoke('update_settings', () => pending.promise);
    const wrapper = await renderPage();
    await field(wrapper, '语音 API Key').setValue('configured-key');
    await wrapper.get('.voice-style-guidance-entry').trigger('click');
    await flushPromises();
    expect(context.openStyleGuidance).not.toHaveBeenCalled();
    pending.resolve(saved({ 'voice.connection.apiKey': 'configured-key' }));
    await flushPromises();
    expect(context.openStyleGuidance).toHaveBeenCalledOnce();
  });

  it('系统返回复用原页面history entry并等待保存，不直接弹出', async () => {
    const pending = deferred<ConfigSaveResult>();
    mockInvoke('update_settings', () => pending.promise);
    const { wrapper, fallbackClose } = await renderOverlay();
    expect(modals.modalStackLength()).toBe(1);
    await field(wrapper, '语音 API Key').setValue('back-key');
    await openSelection(wrapper, '朗读音色');
    expect(modals.modalStackLength()).toBe(2);
    modals.closeTopModal();
    await flushPromises();
    expect(useOverlayStore().contextMenuConfig).toBeNull();
    expect(updates()).toHaveLength(0);
    expect(wrapper.emitted('close')).toBeUndefined();
    expect(modals.modalStackLength()).toBe(1);
    modals.closeTopModal();
    await flushPromises();
    expect(fallbackClose).not.toHaveBeenCalled();
    expect(wrapper.emitted('close')).toBeUndefined();
    expect(modals.modalStackLength()).toBe(1);
    pending.resolve(saved({ 'voice.connection.apiKey': 'back-key' }));
    await flushPromises();
    expect(wrapper.emitted('close')).toHaveLength(1);
  });

  it('被覆盖后返回重新建立设置会话，旧草稿不会复活', async () => {
    const { wrapper } = await renderOverlay();
    await field(wrapper, '语音 API Key').setValue('old-draft');
    await wrapper.setProps({ active: false });
    await wrapper.setProps({ active: true });
    await flushPromises();
    expect((field(wrapper, '语音 API Key').element as HTMLInputElement).value).toBe('');
    await field(wrapper, '语音 API Key').setValue('new-draft');
    await leaveVoiceSettings(wrapper);
    await flushPromises();
    expect(database.voice.connection.apiKey).toBe('new-draft');
    expect(modals.modalStackLength()).toBe(1);
  });

  it('页面卸载只关闭自己仍持有的菜单，不关闭后续其他入口的菜单', async () => {
    const wrapper = await renderPage();
    await openSelection(wrapper, '朗读音色');
    const overlay = useOverlayStore();
    overlay.openContextMenu([{ label: '其他操作', handler: vi.fn() }], '其他菜单');
    const replacement = overlay.contextMenuConfig;
    wrappers.splice(wrappers.indexOf(wrapper), 1);
    wrapper.unmount();
    await flushPromises();
    expect(overlay.contextMenuConfig).toBe(replacement);
    expect(modals.modalStackLength()).toBe(1);
  });

  it('仅提供预置和复刻，切换保留音色草稿', async () => {
    const { wrapper } = await renderOverlay();
    await choose(wrapper, '朗读音色', '冰糖 · 中文女声');
    await choose(wrapper, '音色模式', '音色复刻');
    expect(wrapper.get('[aria-label="参考音频"]').isVisible()).toBe(true);
    expect(wrapper.find('[aria-label="朗读音色"]').exists()).toBe(false);
    expect(wrapper.find('[role="switch"][aria-label="流式朗读"]').exists()).toBe(false);
    await choose(wrapper, '音色模式', '预置音色');
    expect(wrapper.get('[aria-label="参考音频"]').isVisible()).toBe(false);
    expect(wrapper.find('[aria-label="朗读音色"]').exists()).toBe(true);
    expect(wrapper.get('[aria-label="朗读音色"]').text()).toContain('冰糖');
    await leaveVoiceSettings(wrapper);
    expect(configRequestChanges(updates()[0]?.[1]?.request as ConfigUpdate)).toEqual({ 'voice.tts.voice': '冰糖' });
    expect(database.voice.tts.stream).toBe(true);
  });

  it('风格指导只编辑一项规则，保留原文且系统返回等待保存', async () => {
    const pending = deferred<ConfigSaveResult>();
    mockInvoke('update_settings', () => pending.promise);
    const { wrapper, fallbackClose } = await renderOverlay('styleGuidance');
    const areas = wrapper.findAll('textarea');
    expect(areas).toHaveLength(1);
    expect(wrapper.find('[role="switch"]').exists()).toBe(false);
    const rules = '  依据当前角色生成声音描述。\n仅在需要时使用 [轻笑]。  ';
    await areas[0].setValue(rules);
    modals.closeTopModal();
    await flushPromises();
    expect(fallbackClose).not.toHaveBeenCalled();
    expect(wrapper.emitted('close')).toBeUndefined();
    expect(configRequestChanges(updates()[0]?.[1]?.request as ConfigUpdate)).toEqual({ 'voice.guidance.rules': rules });
    pending.resolve(saved({ 'voice.guidance.rules': rules }));
    await flushPromises();
    expect(wrapper.emitted('close')).toHaveLength(1);
    expect(modals.modalStackLength()).toBe(1);
  });

  it.each(['', ' \n '])('风格指导允许空规则并按原文保存（%j）', async rules => {
    const { wrapper } = await renderOverlay('styleGuidance');
    await wrapper.get('textarea').setValue(rules);
    await wrapper.get('button[aria-label="返回，关闭风格指导"]').trigger('click');
    await flushPromises();
    expect(configRequestChanges(updates()[0]?.[1]?.request as ConfigUpdate)).toEqual({ 'voice.guidance.rules': rules });
    expect(database.voice.guidance.rules).toBe(rules);
    expect(wrapper.emitted('close')).toHaveLength(1);
  });

  it('风格指导可恢复默认规则后返回', async () => {
    const { wrapper } = await renderOverlay('styleGuidance');
    await wrapper.get('textarea').setValue('自定义规则');
    await wrapper.findAll('button').find(button => button.text() === '恢复默认规则')!.trigger('click');
    expect((wrapper.get('textarea').element as HTMLTextAreaElement).value).toBe(SETTINGS_DEFAULTS.voice.guidance.rules);
    await wrapper.get('button[aria-label="返回，关闭风格指导"]').trigger('click');
    await flushPromises();
    expect(updates()).toHaveLength(0);
    expect(wrapper.emitted('close')).toHaveLength(1);
  });

  it.each(['页面返回', '系统返回'])('规则细节等待草稿保存，%s后继续编辑已保存规则', async destination => {
    const pending = deferred<ConfigSaveResult>();
    mockInvoke('update_settings', () => pending.promise);
    const overlay = useOverlayStore();
    overlay.pushPage('voiceStyleGuidance');
    const { wrapper } = await renderOverlay('styleGuidance');
    const rules = '  声音轻柔。\n保留我的自定义规则。  ';
    await wrapper.get('textarea').setValue(rules);
    await wrapper.get('button[aria-label="规则细节"]').trigger('click');
    await flushPromises();
    expect(overlay.pageStackTop?.type).toBe('voiceStyleGuidance');
    expect(wrapper.get('textarea').attributes('disabled')).toBeDefined();
    modals.closeTopModal();
    await flushPromises();
    expect(wrapper.emitted('close')).toBeUndefined();

    pending.resolve(saved({ 'voice.guidance.rules': rules }));
    await flushPromises();
    expect(overlay.pageStack.map(page => page.type)).toEqual(['voiceStyleGuidance', 'voiceRuleDetails']);
    expect(overlay.getPageZIndex('voiceRuleDetails')).toBeGreaterThan(overlay.getPageZIndex('voiceStyleGuidance'));
    await wrapper.setProps({ active: false });
    const details = mount(VoiceOptionsView, {
      props: { mode: 'ruleDetails', isOpen: true, active: true,
        zIndex: overlay.getPageZIndex('voiceRuleDetails'), onClose: overlay.closeVoiceRuleDetails },
      global: { stubs: { SlidePage: { template: '<div><slot /></div>' } } },
    });
    wrappers.push(details);
    expect(details.get('article h1').text()).toBe('MiMo 语音风格控制');
    expect(details.findAll('article table')).toHaveLength(2);
    expect(details.find('textarea').exists()).toBe(false);
    expect(details.get('article').text()).toContain('<voice_guidance>');
    if (destination === '页面返回') await details.get('button[aria-label="返回，关闭规则细节"]').trigger('click');
    else modals.closeTopModal();
    await flushPromises();
    expect(overlay.pageStackTop?.type).toBe('voiceStyleGuidance');
    expect(modals.modalStackLength()).toBe(1);
    await details.setProps({ isOpen: false, active: false });
    await wrapper.setProps({ active: true });
    await flushPromises();
    expect((wrapper.get('textarea').element as HTMLTextAreaElement).value).toBe(rules);
    expect(updates()).toHaveLength(1);
    overlay.popToRoot();
  });

  it('打开规则细节时保存失败，留在编辑页保留草稿并允许重试', async () => {
    mockInvoke('update_settings', () => { throw new Error('保存失败'); });
    const { wrapper } = await renderOverlay('styleGuidance');
    await wrapper.get('textarea').setValue('尚未保存的规则');
    await wrapper.get('button[aria-label="规则细节"]').trigger('click');
    await flushPromises();
    expect(useOverlayStore().isVoiceRuleDetailsOpen).toBe(false);
    expect(wrapper.get('[role="alert"]').text()).toContain('保存失败');
    expect((wrapper.get('textarea').element as HTMLTextAreaElement).value).toBe('尚未保存的规则');
    expect(wrapper.get('button[aria-label="规则细节"]').attributes('disabled')).toBeUndefined();
  });

  it('试听只提交当前语音草稿，事件按身份处理，返回只停止本页试听', async () => {
    context.voice.active = false;
    const { wrapper } = await renderOverlay();
    await field(wrapper, '语音 API Key').setValue('draft-preview-key');
    await choose(wrapper, '朗读音色', '冰糖 · 中文女声');
    await disclosure(wrapper, '试听声音').trigger('click');
    await wrapper.get('.voice-preview-button').trigger('click');
    await flushPromises();
    const previewCall = invokeMock.mock.calls.find(([command]) => command === 'preview_tts')!;
    expect(previewCall[1]?.settings).toMatchObject({ connection: { apiBaseUrl: '', apiKey: 'draft-preview-key' }, tts: { voice: '冰糖' } });
    expect(updates()).toHaveLength(0);
    expect(wrapper.get('.voice-preview-button').text()).toBe('停止试听');
    emitTauriEvent('vcp-tts', { ttsId: 'another-narration', kind: 'stopped', stage: 'finished' });
    await flushPromises();
    expect(wrapper.get('.voice-preview-button').text()).toBe('停止试听');
    await leaveVoiceSettings(wrapper);
    expect(invokeMock.mock.calls.filter(([command]) => command === 'stop_tts').map(([, args]) => args?.ttsId)).toEqual([previewCall[1]?.ttsId]);
    expect(database.voice.connection.apiKey).toBe('draft-preview-key');
  });

  it('通话中不能试听，试听提交后失活会处理迟到接纳且不保存草稿', async () => {
    const pending = deferred<unknown>();
    mockInvoke('preview_tts', () => pending.promise);
    const { wrapper } = await renderOverlay();
    await disclosure(wrapper, '试听声音').trigger('click');
    expect(wrapper.get('.voice-preview-button').attributes('disabled')).toBeDefined();
    context.voice.active = false;
    await flushPromises();
    await wrapper.get('.voice-preview-button').trigger('click');
    await flushPromises();
    const id = invokeMock.mock.calls.find(([command]) => command === 'preview_tts')?.[1]?.ttsId;
    await wrapper.setProps({ active: false });
    pending.resolve({ configured: true });
    await flushPromises();
    expect(invokeMock.mock.calls.filter(([command]) => command === 'stop_tts').every(([, args]) => args?.ttsId === id)).toBe(true);
    expect(invokeMock.mock.calls.filter(([command]) => command === 'stop_tts')).toHaveLength(1);
    expect(updates()).toHaveLength(0);
  });

  it('取消尚在注册监听的试听，等待清理后才允许再次开始', async () => {
    const previousListen = deferred<() => void>();
    const currentListen = deferred<() => void>();
    const unlistenPrevious = vi.fn();
    const unlistenCurrent = vi.fn();
    context.voice.active = false;
    const { wrapper } = await renderOverlay();
    await disclosure(wrapper, '试听声音').trigger('click');
    listenMock.mockImplementationOnce(() => previousListen.promise).mockImplementationOnce(() => currentListen.promise);

    await wrapper.get('.voice-preview-button').trigger('click');
    await wrapper.get('.voice-preview-button').trigger('click');
    expect(wrapper.get('.voice-preview-button').text()).toBe('停止试听');
    previousListen.resolve(unlistenPrevious);
    await flushPromises();
    expect(wrapper.get('.voice-preview-button').text()).toBe('试听当前设置');
    expect(invokeMock.mock.calls.filter(([command]) => command === 'preview_tts')).toHaveLength(0);
    await wrapper.get('.voice-preview-button').trigger('click');
    currentListen.resolve(unlistenCurrent);
    await flushPromises();

    expect(unlistenPrevious).toHaveBeenCalledOnce();
    expect(unlistenCurrent).not.toHaveBeenCalled();
    expect(wrapper.get('.voice-preview-button').text()).toBe('停止试听');
    expect(invokeMock.mock.calls.filter(([command]) => command === 'preview_tts')).toHaveLength(1);
    expect(invokeMock.mock.calls.filter(([command]) => command === 'stop_tts')).toHaveLength(0);
    const currentId = invokeMock.mock.calls.find(([command]) => command === 'preview_tts')?.[1]?.ttsId;
    await wrapper.get('.voice-preview-button').trigger('click');
    await flushPromises();
    expect(invokeMock.mock.calls.filter(([command]) => command === 'stop_tts').map(([, args]) => args?.ttsId)).toEqual([currentId]);
  });

  it.each(['返回', '风格指导'])('离开前等待迟到试听启动与原生停止确认（%s）', async destination => {
    context.voice.active = false;
    const starting = deferred<unknown>();
    const stopping = deferred<unknown>();
    mockInvoke('preview_tts', () => starting.promise);
    mockInvoke('stop_tts', () => stopping.promise);
    const { wrapper } = await renderOverlay();
    await disclosure(wrapper, '试听声音').trigger('click');
    await wrapper.get('.voice-preview-button').trigger('click');
    await flushPromises();
    if (destination === '返回') await wrapper.get('button[aria-label="返回，关闭语音设置"]').trigger('click');
    else await wrapper.get('.voice-style-guidance-entry').trigger('click');
    await flushPromises();
    expect(wrapper.emitted('close')).toBeUndefined();
    expect(context.openStyleGuidance).not.toHaveBeenCalled();
    expect(wrapper.get('.voice-preview-button').attributes('disabled')).toBeDefined();
    expect(invokeMock.mock.calls.filter(([command]) => command === 'stop_tts')).toHaveLength(0);
    starting.resolve({ configured: true });
    await flushPromises();
    expect(invokeMock.mock.calls.filter(([command]) => command === 'stop_tts')).toHaveLength(1);
    expect(wrapper.emitted('close')).toBeUndefined();
    expect(context.openStyleGuidance).not.toHaveBeenCalled();
    stopping.resolve(undefined);
    await flushPromises();
    if (destination === '返回') expect(wrapper.emitted('close')).toHaveLength(1);
    else expect(context.openStyleGuidance).toHaveBeenCalledOnce();
  });

  it('复刻导入只携路径，切模式保留未保存样本，保存后的清理交给Rust判断', async () => {
    const asset = { id: 'new-sample', fileName: 'voice.wav', format: 'wav', bytes: 96000 };
    mockInvoke('plugin:vcp-mobile|pick_file', () => ({ path: '/native/staging/voice.wav', name: 'voice.wav', size: 96000, mime: 'audio/wav', hash: 'file-hash' }));
    mockInvoke('import_voice_clone_sample', () => asset);
    mockInvoke('get_voice_clone_sample', () => asset);
    const { wrapper } = await renderOverlay();
    await choose(wrapper, '音色模式', '音色复刻');
    await wrapper.get('button[aria-label="参考音频"]').trigger('click');
    await flushPromises();
    expect(invokeMock.mock.calls.find(([command]) => command === 'plugin:vcp-mobile|pick_file')?.[1]).toMatchObject({ mode: 'file', maxBytes: 7_000_000 });
    expect(invokeMock.mock.calls.find(([command]) => command === 'import_voice_clone_sample')?.[1]).toEqual({ path: '/native/staging/voice.wav', name: 'voice.wav' });
    expect(wrapper.get('button[aria-label="参考音频"]').text()).toContain('voice.wav');
    await choose(wrapper, '音色模式', '预置音色');
    expect(invokeMock.mock.calls.filter(([command]) => command === 'discard_voice_clone_sample')).toHaveLength(0);
    await choose(wrapper, '音色模式', '音色复刻');
    await leaveVoiceSettings(wrapper);
    expect(database.voice.tts.cloneSampleId).toBe('new-sample');
    expect(configRequestChanges(updates()[0]?.[1]?.request as ConfigUpdate)).toEqual({ 'voice.tts.mode': 'clone', 'voice.tts.cloneSampleId': 'new-sample' });
  });

  it('失活后的迟到复刻导入不更新设置，释放新资产', async () => {
    const pending = deferred<unknown>();
    mockInvoke('plugin:vcp-mobile|pick_file', () => ({ path: '/native/staging/late.mp3', name: 'late.mp3', size: 1000, mime: 'audio/mpeg', hash: 'late-hash' }));
    mockInvoke('import_voice_clone_sample', () => pending.promise);
    const { wrapper } = await renderOverlay();
    await choose(wrapper, '音色模式', '音色复刻');
    await wrapper.get('button[aria-label="参考音频"]').trigger('click');
    await flushPromises();
    await wrapper.setProps({ active: false });
    pending.resolve({ id: 'late-sample', fileName: 'late.mp3', format: 'mp3', bytes: 1000 });
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledWith('discard_voice_clone_sample', { sampleId: 'late-sample' });
    expect(updates()).toHaveLength(0);
    expect(database.voice.tts.cloneSampleId).toBe('');
  });

  it('样本草稿已提交保存时，失活清理等待真实保存完成', async () => {
    const pending = deferred<ConfigSaveResult>();
    const asset = { id: 'saving-sample', fileName: 'saving.wav', format: 'wav', bytes: 96000 };
    mockInvoke('plugin:vcp-mobile|pick_file', () => ({ path: '/native/staging/saving.wav', name: 'saving.wav', size: 96000, mime: 'audio/wav', hash: 'saving-hash' }));
    mockInvoke('import_voice_clone_sample', () => asset);
    mockInvoke('get_voice_clone_sample', () => asset);
    mockInvoke('update_settings', () => pending.promise);
    const { wrapper } = await renderOverlay();
    await choose(wrapper, '音色模式', '音色复刻');
    await wrapper.get('button[aria-label="参考音频"]').trigger('click');
    await flushPromises();
    await leaveVoiceSettings(wrapper);
    expect(updates()).toHaveLength(1);
    await wrapper.setProps({ active: false });
    await flushPromises();
    expect(invokeMock.mock.calls.filter(([command]) => command === 'discard_voice_clone_sample')).toHaveLength(0);
    pending.resolve(saved({ 'voice.tts.mode': 'clone', 'voice.tts.cloneSampleId': asset.id }));
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledWith('discard_voice_clone_sample', { sampleId: asset.id });
    expect(useSettingsStore().settings?.voice.tts.cloneSampleId).toBe(asset.id);
  });
});

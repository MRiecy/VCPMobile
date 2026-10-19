import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, disposePinia, setActivePinia, type Pinia } from 'pinia';
import { reactive } from 'vue';
import { DOMWrapper, flushPromises, mount, type VueWrapper } from '@vue/test-utils';
import VoiceSettingsPage from '../../../features/chat/voice/VoiceSettingsPage.vue';
import VoiceOptionsView from '../../../features/chat/voice/VoiceOptionsView.vue';
import SettingsTextField from '../../../components/settings/SettingsTextField.vue';
import GlobalOverlayManager from '../../../components/GlobalOverlayManager.vue';
import { useOverlayStore } from '../../../core/stores/overlay';
import { DEFAULT_VOICE_SETTINGS } from '../../../features/chat/voice/settings';
import { useSettingsStore, type AppSettings } from '../../../core/stores/settings';
import { useModalHistory } from '../../../core/composables/useModalHistory';
import { invokeMock, mockInvoke } from '../../mocks/tauri';

const context = vi.hoisted(() => ({
  openPresets: vi.fn(),
  voice: {} as {
    active: boolean; microphoneState: string; playbackActive: boolean; probeActive: boolean; probePending: boolean;
    speechActive: boolean; aecState: string | null; route: string; probeError: string;
    setProbe: ReturnType<typeof vi.fn>;
  },
}));
vi.mock('../../../core/stores/notification', () => ({ useNotificationStore: () => ({ addNotification: vi.fn() }) }));
vi.mock('../../../features/chat/voice/voiceCallStore', () => ({ useVoiceCallStore: () => context.voice }));
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: vi.fn().mockResolvedValue(undefined) }));

function snapshot(): AppSettings {
  return {
    ...DEFAULT_VOICE_SETTINGS,
    userName: 'user', vcpServerUrl: 'https://chat.example', chatEndpointMode: 'standard', vcpApiKey: 'chat-key',
    vcpLogUrl: '', vcpLogKey: '', syncServerUrl: '', syncHttpUrl: '', syncToken: '',
    adminUsername: '', adminPassword: '', fileKey: '', topicSummaryModel: '', syncLogLevel: 'INFO',
    agentOrder: [], groupOrder: [], currentThemeMode: null, syncPrerenderEnabled: false,
    distributedEnabled: false, distributedWsUrl: '', distributedVcpKey: '', distributedDeviceName: '',
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(accept => { resolve = accept; });
  return { promise, resolve };
}

let pinia: Pinia;
let database: AppSettings;
const wrappers: VueWrapper[] = [];
const modals = useModalHistory();

beforeEach(() => {
  pinia = createPinia();
  setActivePinia(pinia);
  database = snapshot();
  context.openPresets.mockReset();
  vi.spyOn(useOverlayStore(), 'openVoicePresets').mockImplementation(context.openPresets);
  context.voice = reactive({
    active: true, microphoneState: 'on', playbackActive: false, probeActive: false, probePending: false,
    aecState: 'enabled', speechActive: false, route: 'speaker', probeError: '',
    setProbe: vi.fn().mockResolvedValue(true),
  });
  mockInvoke('read_settings', () => ({ ...database }));
  mockInvoke('get_settings_recovery_status', () => ({ recoveredCorrupt: false }));
  mockInvoke('update_settings', args => {
    database = { ...database, ...(args?.updates as Partial<AppSettings>) };
    return { ...database };
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
  modals.unregisterModal('Page:voiceSettings:');
  disposePinia(pinia);
  vi.restoreAllMocks();
});

async function renderPage() {
  const wrapper = mount(VoiceSettingsPage, { props: { active: true }, attachTo: document.body });
  wrappers.push(wrapper);
  await flushPromises();
  return wrapper;
}

async function renderOverlay() {
  const fallbackClose = vi.fn();
  modals.registerModal('Page:voiceSettings:', fallbackClose);
  const wrapper = mount(VoiceOptionsView, {
    attachTo: document.body,
    props: { mode: 'settings', isOpen: true, active: true, zIndex: 40 },
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

describe('local voice diagnostics', () => {
  it('requires active capture, exposes sample attribution and does not infer AEC effectiveness', async () => {
    context.voice.microphoneState = 'off';
    const wrapper = await renderPage();
    expect(wrapper.get('.settings-disclosure-toggle').attributes('aria-expanded')).toBe('false');
    expect(wrapper.find('.voice-probe-button').exists()).toBe(false);
    await wrapper.get('.settings-disclosure-toggle').trigger('click');
    const button = wrapper.get('.voice-probe-button');
    expect(button.attributes('disabled')).toBeDefined();
    await button.trigger('click');
    expect(context.voice.setProbe).not.toHaveBeenCalled();
    expect(wrapper.text()).toContain('请先进入语音模式并开启麦克风');
    expect(wrapper.text()).toContain('效果待验证');
    expect(wrapper.get('a[href="https://github.com/Jakobovski/free-spoken-digit-dataset"]').text()).toBe('FSDD');
    expect(wrapper.text()).toContain('CC BY-SA 4.0');
  });

  it('allows stopping playback setup and stops the sample when its settings page becomes inactive', async () => {
    const wrapper = await renderPage();
    await wrapper.get('.settings-disclosure-toggle').trigger('click');
    const button = wrapper.get('.voice-probe-button');
    await button.trigger('click');
    expect(context.voice.setProbe).toHaveBeenLastCalledWith(true);
    context.voice.probePending = true;
    await wrapper.vm.$nextTick();
    expect(button.text()).toBe('停止样本');
    await button.trigger('click');
    expect(context.voice.setProbe).toHaveBeenLastCalledWith(false);
    context.voice.setProbe.mockClear();
    await wrapper.setProps({ active: false });
    expect(context.voice.setProbe).toHaveBeenCalledExactlyOnceWith(false);
    expect(button.attributes('disabled')).toBeDefined();
    wrappers.splice(wrappers.indexOf(wrapper), 1);
    wrapper.unmount();
    expect(context.voice.setProbe).toHaveBeenCalledOnce();
  });

  it('stops a sample when the active page is unmounted', async () => {
    const wrapper = await renderPage();
    wrappers.splice(wrappers.indexOf(wrapper), 1);
    wrapper.unmount();
    expect(context.voice.setProbe).toHaveBeenCalledExactlyOnceWith(false);
  });

  it('distinguishes TTS playback from the local diagnostic sample', async () => {
    context.voice.playbackActive = true;
    const wrapper = await renderPage();
    await wrapper.get('.settings-disclosure-toggle').trigger('click');
    const button = wrapper.get('.voice-probe-button');
    expect(button.text()).toBe('播放本地样本');
    await button.trigger('click');
    expect(context.voice.setProbe).toHaveBeenLastCalledWith(true);
    context.voice.probeActive = true;
    await wrapper.vm.$nextTick();
    expect(button.text()).toBe('停止样本');
    await wrapper.get('.settings-disclosure-toggle').trigger('click');
    expect(wrapper.find('.voice-probe-button').exists()).toBe(false);
    expect(wrapper.get('.settings-disclosure-toggle').text()).toContain('本地样本播放中');
    expect(context.voice.setProbe).toHaveBeenCalledExactlyOnceWith(true);
  });
});

describe('语音设置使用正式设置快照', () => {
  it('默认选择 WAV，编码随本页草稿保存并可切回，不改变朗读配置', async () => {
    const { wrapper } = await renderOverlay();
    expect(wrapper.find('select').exists()).toBe(false);
    expect(wrapper.get('button[aria-label="语音输入编码"]').text()).toContain('WAV 无损');
    const menu = await openSelection(wrapper, '语音输入编码');
    expect(menu.findAll('[role="radio"]').map(option => option.text())).toEqual(['WAV 无损', 'MP3 64 kbps']);
    expect(menu.get('[aria-checked="true"]').text()).toBe('WAV 无损');
    await menu.get('[aria-checked="false"]').trigger('click');
    await flushPromises();
    expect(useOverlayStore().contextMenuConfig).toBeNull();
    expect(updates()).toHaveLength(0);
    expect(useSettingsStore().settings?.voiceInputEncoding).toBe('wav');
    expect(wrapper.find('footer').exists()).toBe(false);
    expect(wrapper.find('.voice-settings-save').exists()).toBe(false);
    await leaveVoiceSettings(wrapper);
    await flushPromises();
    expect(updates()[0]?.[1]?.updates).toEqual({ voiceInputEncoding: 'mp3_64' });
    expect(database.voiceInputEncoding).toBe('mp3_64');
    expect(database.voiceTtsStream).toBe(true);
    expect(database.voiceTtsVoice).toBe('mimo_default');
    expect(wrapper.emitted('close')).toHaveLength(1);
    await wrapper.setProps({ active: false });
    await wrapper.setProps({ active: true });
    await flushPromises();
    expect(wrapper.get('button[aria-label="语音输入编码"]').text()).toContain('MP3 64 kbps');
    await choose(wrapper, '语音输入编码', 'WAV 无损');
    await leaveVoiceSettings(wrapper);
    await flushPromises();
    expect(updates()[1]?.[1]?.updates).toEqual({ voiceInputEncoding: 'wav' });
    expect(database.voiceInputEncoding).toBe('wav');
  });

  it('只提交本页dirty字段，两个流式开关互不改变，中文音色ID原样保存', async () => {
    const { wrapper } = await renderOverlay();
    await field(wrapper, '语音 API Key').setValue('voice-secret');
    await choose(wrapper, '识别语言', '中文');
    await choose(wrapper, '朗读音色', '白桦 · 中文男声');
    await wrapper.get('[role="switch"][aria-label="流式识别结果"]').trigger('click');
    expect(updates()).toHaveLength(0);
    expect(useSettingsStore().settings?.voiceApiKey).toBe('');
    await leaveVoiceSettings(wrapper);
    await flushPromises();
    expect(updates()[0]?.[1]?.updates).toEqual({
      voiceApiKey: 'voice-secret', voiceLanguage: 'zh', voiceTtsVoice: '白桦', voiceAsrStream: false,
    });
    expect(database.voiceTtsStream).toBe(true);
    expect(database.vcpApiKey).toBe('chat-key');
    await wrapper.setProps({ active: false });
    await wrapper.setProps({ active: true });
    await flushPromises();
    await wrapper.get('[role="switch"][aria-label="流式朗读"]').trigger('click');
    await leaveVoiceSettings(wrapper);
    await flushPromises();
    expect(updates()[1]?.[1]?.updates).toEqual({ voiceTtsStream: false });
    expect(database.voiceAsrStream).toBe(false);
    expect(wrapper.text()).toContain('整段上传');
    expect(wrapper.text()).toContain('重新开启麦克风');
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
    expect(database.voiceApiKey).toBe('');
    expect(database.voiceInputEncoding).toBe('wav');
    expect(context.voice.setProbe).toHaveBeenCalledWith(false);
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
    expect(wrapper.get('[role="alert"]').text()).toContain('语音设置保存失败');
    expect(wrapper.get('[role="alert"]').text()).not.toContain('voice-private-token');
    expect(JSON.stringify(log.mock.calls)).not.toContain('voice-private-token');
    expect((field(wrapper, '语音 API Key').element as HTMLInputElement).value).toBe('voice-private-token');
    expect(database.voiceApiKey).toBe('');
    expect(wrapper.emitted('close')).toBeUndefined();
    expect(modals.modalStackLength()).toBe(1);
    mockInvoke('update_settings', args => {
      database = { ...database, ...(args?.updates as Partial<AppSettings>) };
      return { ...database };
    });
    await leaveVoiceSettings(wrapper);
    expect(database.voiceApiKey).toBe('voice-private-token');
    expect(wrapper.emitted('close')).toHaveLength(1);
  });

  it('进入预设前等待当前草稿确定保存', async () => {
    const pending = deferred<AppSettings>();
    mockInvoke('update_settings', () => pending.promise);
    const wrapper = await renderPage();
    await field(wrapper, '语音 API Key').setValue('configured-key');
    await wrapper.get('.voice-preset-entry').trigger('click');
    await flushPromises();
    expect(context.openPresets).not.toHaveBeenCalled();
    pending.resolve({ ...database, voiceApiKey: 'configured-key' });
    await flushPromises();
    expect(context.openPresets).toHaveBeenCalledOnce();
  });

  it('系统返回复用原页面history entry并等待保存，不直接弹出', async () => {
    const pending = deferred<AppSettings>();
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
    pending.resolve({ ...database, voiceApiKey: 'back-key' });
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
    expect(database.voiceApiKey).toBe('new-draft');
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
});

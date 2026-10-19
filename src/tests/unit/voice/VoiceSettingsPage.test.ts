import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, disposePinia, setActivePinia, type Pinia } from 'pinia';
import { reactive } from 'vue';
import { flushPromises, mount, type VueWrapper } from '@vue/test-utils';
import VoiceSettingsPage from '../../../features/chat/voice/VoiceSettingsPage.vue';
import VoiceOptionsView from '../../../features/chat/voice/VoiceOptionsView.vue';
import SettingsTextField from '../../../components/settings/SettingsTextField.vue';
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
vi.mock('../../../core/stores/overlay', () => ({ useOverlayStore: () => ({ openVoicePresets: context.openPresets }) }));
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
});

afterEach(() => {
  wrappers.splice(0).forEach(wrapper => wrapper.unmount());
  modals.unregisterModal('Page:voiceSettings:');
  disposePinia(pinia);
  vi.restoreAllMocks();
});

async function renderPage() {
  const wrapper = mount(VoiceSettingsPage, { props: { active: true } });
  wrappers.push(wrapper);
  await flushPromises();
  return wrapper;
}

async function renderOverlay() {
  const fallbackClose = vi.fn();
  modals.registerModal('Page:voiceSettings:', fallbackClose);
  const wrapper = mount(VoiceOptionsView, {
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

function saveButton(wrapper: VueWrapper) {
  const button = wrapper.findAll('button').find(candidate => candidate.text() === '保存语音设置');
  if (!button) throw new Error('Missing save button');
  return button;
}
const updates = () => invokeMock.mock.calls.filter(([command]) => command === 'update_settings');

describe('local voice diagnostics', () => {
  it('requires active capture, exposes sample attribution and does not infer AEC effectiveness', async () => {
    context.voice.microphoneState = 'off';
    const wrapper = await renderPage();
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
    const button = wrapper.get('.voice-probe-button');
    expect(button.text()).toBe('播放本地样本');
    await button.trigger('click');
    expect(context.voice.setProbe).toHaveBeenLastCalledWith(true);
    context.voice.probeActive = true;
    await wrapper.vm.$nextTick();
    expect(button.text()).toBe('停止样本');
  });
});

describe('语音设置使用正式设置快照', () => {
  it('默认选择 WAV，编码随本页草稿保存并可切回，不改变朗读配置', async () => {
    const { wrapper } = await renderOverlay();
    const encoding = wrapper.get('select[aria-label="语音输入编码"]');
    expect((encoding.element as HTMLSelectElement).value).toBe('wav');
    expect(encoding.findAll('option').map(option => option.text())).toEqual(['WAV 无损', 'MP3 64 kbps']);
    await encoding.setValue('mp3_64');
    expect(updates()).toHaveLength(0);
    expect(useSettingsStore().settings?.voiceInputEncoding).toBe('wav');
    await saveButton(wrapper).trigger('click');
    await flushPromises();
    expect(updates()[0]?.[1]?.updates).toEqual({ voiceInputEncoding: 'mp3_64' });
    expect(database.voiceInputEncoding).toBe('mp3_64');
    expect(database.voiceTtsStream).toBe(true);
    expect(database.voiceTtsVoice).toBe('mimo_default');
    await wrapper.setProps({ active: false });
    await wrapper.setProps({ active: true });
    await flushPromises();
    expect((wrapper.get('select[aria-label="语音输入编码"]').element as HTMLSelectElement).value).toBe('mp3_64');
    await wrapper.get('select[aria-label="语音输入编码"]').setValue('wav');
    await saveButton(wrapper).trigger('click');
    await flushPromises();
    expect(updates()[1]?.[1]?.updates).toEqual({ voiceInputEncoding: 'wav' });
    expect(database.voiceInputEncoding).toBe('wav');
  });

  it('只提交本页dirty字段，两个流式开关互不改变，中文音色ID原样保存', async () => {
    const wrapper = await renderPage();
    await field(wrapper, '语音 API Key').setValue('voice-secret');
    await wrapper.get('select[aria-label="识别语言"]').setValue('zh');
    await wrapper.get('select[aria-label="朗读音色"]').setValue('白桦');
    await wrapper.get('[role="switch"][aria-label="流式识别结果"]').trigger('click');
    expect(updates()).toHaveLength(0);
    expect(useSettingsStore().settings?.voiceApiKey).toBe('');
    await saveButton(wrapper).trigger('click');
    await flushPromises();
    expect(updates()[0]?.[1]?.updates).toEqual({
      voiceApiKey: 'voice-secret', voiceLanguage: 'zh', voiceTtsVoice: '白桦', voiceAsrStream: false,
    });
    expect(database.voiceTtsStream).toBe(true);
    expect(database.vcpApiKey).toBe('chat-key');
    await wrapper.get('[role="switch"][aria-label="流式朗读"]').trigger('click');
    await saveButton(wrapper).trigger('click');
    await flushPromises();
    expect(updates()[1]?.[1]?.updates).toEqual({ voiceTtsStream: false });
    expect(database.voiceAsrStream).toBe(false);
    expect(wrapper.text()).toContain('整段上传');
    expect(wrapper.text()).toContain('重新开启麦克风');
  });

  it('失活只丢弃未提交草稿，不暗中保存API Key与输入编码', async () => {
    const { wrapper } = await renderOverlay();
    await field(wrapper, '语音 API Key').setValue('unsubmitted-secret');
    await wrapper.get('select[aria-label="语音输入编码"]').setValue('mp3_64');
    await wrapper.setProps({ active: false });
    await flushPromises();
    expect(updates()).toHaveLength(0);
    expect(database.voiceApiKey).toBe('');
    expect(database.voiceInputEncoding).toBe('wav');
    expect(context.voice.setProbe).toHaveBeenCalledWith(false);
    await wrapper.setProps({ active: true });
    await flushPromises();
    expect((wrapper.get('select[aria-label="语音输入编码"]').element as HTMLSelectElement).value).toBe('wav');
  });

  it('保存失败不回显或记录密钥，保留草稿供重试', async () => {
    const log = vi.spyOn(console, 'error').mockImplementation(() => {});
    mockInvoke('update_settings', () => Promise.reject(new Error('request rejected: voice-private-token')));
    const wrapper = await renderPage();
    await field(wrapper, '语音 API Key').setValue('voice-private-token');
    await saveButton(wrapper).trigger('click');
    await flushPromises();
    expect(wrapper.get('[role="alert"]').text()).toContain('语音设置保存失败');
    expect(wrapper.get('[role="alert"]').text()).not.toContain('voice-private-token');
    expect(JSON.stringify(log.mock.calls)).not.toContain('voice-private-token');
    expect((field(wrapper, '语音 API Key').element as HTMLInputElement).value).toBe('voice-private-token');
    expect(database.voiceApiKey).toBe('');
    expect((saveButton(wrapper).element as HTMLButtonElement).disabled).toBe(false);
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
    await saveButton(wrapper).trigger('click');
    await flushPromises();
    expect(database.voiceApiKey).toBe('new-draft');
    expect(modals.modalStackLength()).toBe(1);
  });
});

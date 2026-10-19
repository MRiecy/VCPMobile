import { beforeEach, describe, expect, it, vi } from 'vitest';
import { mount } from '@vue/test-utils';
import { reactive } from 'vue';
import VoiceSettingsPage from '../../../features/chat/voice/VoiceSettingsPage.vue';

const context = vi.hoisted(() => ({
  store: {} as {
    active: boolean; microphoneState: string; playbackActive: boolean; probePending: boolean;
    speechActive: boolean; aecState: string | null; route: string; probeError: string;
    setProbe: ReturnType<typeof vi.fn>;
  },
  openPresets: vi.fn(),
  openUrl: vi.fn(),
}));

vi.mock('../../../features/chat/voice/voiceCallStore', () => ({ useVoiceCallStore: () => context.store }));
vi.mock('../../../core/stores/overlay', () => ({ useOverlayStore: () => ({ openVoicePresets: context.openPresets }) }));
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: context.openUrl }));

beforeEach(() => {
  context.store = reactive({
    active: true, microphoneState: 'on', playbackActive: false, probePending: false,
    speechActive: false, aecState: 'enabled', route: 'speaker', probeError: '',
    setProbe: vi.fn().mockResolvedValue(undefined),
  });
  context.openPresets.mockReset();
  context.openUrl.mockReset().mockResolvedValue(undefined);
});

describe('local voice diagnostics', () => {
  it('requires active capture, exposes sample attribution and does not infer AEC effectiveness', async () => {
    context.store.microphoneState = 'off';
    const wrapper = mount(VoiceSettingsPage, { props: { active: true } });
    const button = wrapper.get('.voice-probe-button');
    expect(button.attributes('disabled')).toBeDefined();
    await button.trigger('click');
    expect(context.store.setProbe).not.toHaveBeenCalled();
    expect(wrapper.text()).toContain('请先进入语音模式并开启麦克风');
    expect(wrapper.text()).toContain('效果待验证');
    expect(wrapper.get('a[href="https://github.com/Jakobovski/free-spoken-digit-dataset"]').text()).toBe('FSDD');
    expect(wrapper.text()).toContain('CC BY-SA 4.0');
    wrapper.unmount();
  });

  it('allows stopping playback setup and stops the sample when its settings page becomes inactive', async () => {
    const wrapper = mount(VoiceSettingsPage, { props: { active: true } });
    const button = wrapper.get('.voice-probe-button');
    await button.trigger('click');
    expect(context.store.setProbe).toHaveBeenLastCalledWith(true);

    context.store.probePending = true;
    await wrapper.vm.$nextTick();
    expect(button.text()).toBe('停止样本');
    await button.trigger('click');
    expect(context.store.setProbe).toHaveBeenLastCalledWith(false);

    context.store.setProbe.mockClear();
    await wrapper.setProps({ active: false });
    expect(context.store.setProbe).toHaveBeenCalledExactlyOnceWith(false);
    expect(button.attributes('disabled')).toBeDefined();
    wrapper.unmount();
    expect(context.store.setProbe).toHaveBeenCalledOnce();
  });

  it('stops a sample when the active page is unmounted', () => {
    const wrapper = mount(VoiceSettingsPage, { props: { active: true } });
    wrapper.unmount();
    expect(context.store.setProbe).toHaveBeenCalledExactlyOnceWith(false);
  });
});

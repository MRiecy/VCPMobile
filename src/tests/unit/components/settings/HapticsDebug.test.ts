import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest';
import { mount, flushPromises, type VueWrapper } from '@vue/test-utils';
import { triggerNativeHaptic } from '@/core/utils/nativeHaptics';
import { invokeMock, mockInvoke } from '@/tests/mocks/tauri';
import HapticsDebugPanel from '@/features/settings/developer/HapticsDebugPanel.vue';

const platform = vi.hoisted(() => ({ android: true, ios: false }));
vi.mock('@/core/utils/platform', () => ({
  get isAndroid() { return platform.android; },
  get isIOS() { return platform.ios; },
}));

describe('Haptics Debug & Native Haptics Suite', () => {
  let wrapper: VueWrapper | undefined;
  beforeEach(() => {
    vi.clearAllMocks();
    platform.android = true;
    platform.ios = false;
  });
  afterEach(() => { wrapper?.unmount(); wrapper = undefined; });

  it('calls plugin:vcp-mobile|trigger_haptic_feedback with expected payload', async () => {
    mockInvoke('plugin:vcp-mobile|trigger_haptic_feedback', () => ({
      submitted: true,
      type: 'view',
      effect: 'keyboard_tap',
      constant: 3,
    }));

    const result = await triggerNativeHaptic({
      type: 'view',
      effect: 'keyboard_tap',
    });

    expect(result.submitted).toBe(true);
    expect(result.constant).toBe(3);
    expect(invokeMock).toHaveBeenCalledWith('plugin:vcp-mobile|trigger_haptic_feedback', {
      payload: {
        type: 'view',
        effect: 'keyboard_tap',
      },
    });
  });

  it('reports native failure without changing it into a different web vibration', async () => {
    mockInvoke('plugin:vcp-mobile|trigger_haptic_feedback', () => {
      throw new Error('plugin not supported on this platform');
    });

    const result = await triggerNativeHaptic({
      type: 'one_shot',
      durationMs: 15,
      amplitude: 60,
    });

    expect(result.submitted).toBe(false);
    expect(result.reason).toContain('plugin not supported');
    expect(vi.mocked(navigator.vibrate)).not.toHaveBeenCalled();
  });

  it('plays Android test effects and reports submission without claiming hardware playback', async () => {
    mockInvoke('plugin:vcp-mobile|trigger_haptic_feedback', () => ({
      submitted: true,
      type: 'view',
      effect: 'keyboard_tap',
      constant: 3,
    }));

    wrapper = mount(HapticsDebugPanel, {
      props: { active: true },
    });

    expect(wrapper.text()).toContain('消息触感与效果测试');
    expect(wrapper.text()).toContain('键盘轻点 (TAP)');

    const tapButton = wrapper.findAll('button').find((b) => b.text().includes('键盘轻点'));
    expect(tapButton).toBeDefined();

    await tapButton!.trigger('click');
    await flushPromises();

    expect(invokeMock).toHaveBeenCalledWith('plugin:vcp-mobile|trigger_haptic_feedback', {
      payload: {
        type: 'view',
        effect: 'keyboard_tap',
      },
    });

    // Check last log updated
    expect(wrapper.text()).toContain('键盘轻点 (KEYBOARD_TAP)');
    expect(wrapper.text()).toContain('已提交');
  });

  it('uses the real message feedback path for production previews', async () => {
    mockInvoke('plugin:vcp-mobile|trigger_haptic_feedback', () => ({ submitted: true }));
    wrapper = mount(HapticsDebugPanel, { props: { active: true } });
    await wrapper.findAll('button').find((b) => b.text().includes('回复完整生成'))!.trigger('click');
    expect(invokeMock).toHaveBeenCalledWith('plugin:vcp-mobile|trigger_haptic_feedback', {
      payload: { type: 'message', effect: 'complete' },
    });
  });

  it('shows iOS effects and sends the selected intensity', async () => {
    platform.android = false;
    platform.ios = true;
    mockInvoke('plugin:vcp-mobile|trigger_haptic_feedback', () => ({ submitted: true }));
    wrapper = mount(HapticsDebugPanel, { props: { active: true } });
    expect(wrapper.text()).not.toContain('EFFECT_TICK');
    await wrapper.get('input[type="range"]').setValue('0.3');
    await wrapper.findAll('button').find((b) => b.text().startsWith('soft'))!.trigger('click');
    expect(invokeMock).toHaveBeenCalledWith('plugin:vcp-mobile|trigger_haptic_feedback', {
      payload: { type: 'impact', effect: 'soft', intensity: 0.3 },
    });
  });

  it('ignores late feedback after leaving and blocks hidden interactions', async () => {
    let resolve!: (result: { submitted: boolean }) => void;
    mockInvoke('plugin:vcp-mobile|trigger_haptic_feedback', () => new Promise((done) => { resolve = done; }));
    wrapper = mount(HapticsDebugPanel, { props: { active: true } });
    const send = wrapper.findAll('button').find((b) => b.text().startsWith('发送消息'))!;
    await send.trigger('click');
    await wrapper.setProps({ active: false });
    resolve({ submitted: true });
    await flushPromises();
    expect(wrapper.text()).toContain('等待测试');
    await send.trigger('click');
    expect(invokeMock).toHaveBeenCalledTimes(1);
  });
});

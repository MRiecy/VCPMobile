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

  it('keeps controls enabled during feedback while ignoring overlapping requests', async () => {
    let resolve!: (result: { submitted: boolean }) => void;
    mockInvoke('plugin:vcp-mobile|trigger_haptic_feedback', () => new Promise((done) => { resolve = done; }));
    wrapper = mount(HapticsDebugPanel, { props: { active: true } });
    const send = wrapper.findAll('button').find((b) => b.text().startsWith('发送消息'))!;
    const complete = wrapper.findAll('button').find((b) => b.text().startsWith('回复完整生成'))!;
    const web = wrapper.findAll('button').find((b) => b.text().startsWith('10ms 脉冲'))!;
    await send.trigger('click');
    expect(wrapper.get('fieldset').element.disabled).toBe(false);
    expect(wrapper.get('fieldset').attributes('aria-busy')).toBe('true');
    await send.trigger('click');
    await complete.trigger('click');
    await web.trigger('click');
    expect(invokeMock).toHaveBeenCalledTimes(1);
    expect(vi.mocked(navigator.vibrate)).not.toHaveBeenCalled();

    resolve({ submitted: true });
    await flushPromises();
    expect(wrapper.get('fieldset').attributes('aria-busy')).toBe('false');
    await complete.trigger('click');
    expect(invokeMock).toHaveBeenCalledTimes(2);
    resolve({ submitted: true });
    await flushPromises();
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

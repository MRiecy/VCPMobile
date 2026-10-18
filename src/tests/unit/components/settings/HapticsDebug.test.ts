import { describe, expect, it, vi, beforeEach } from 'vitest';
import { mount } from '@vue/test-utils';
import { triggerNativeHaptic } from '@/core/utils/nativeHaptics';
import { invokeMock, mockInvoke } from '@/tests/mocks/tauri';
import HapticsDebugCard from '@/features/chat/settings/HapticsDebugCard.vue';

describe('Haptics Debug & Native Haptics Suite', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('calls plugin:vcp-mobile|trigger_haptic_feedback with expected payload', async () => {
    mockInvoke('plugin:vcp-mobile|trigger_haptic_feedback', () => ({
      performed: true,
      type: 'view',
      effect: 'keyboard_tap',
      constant: 3,
    }));

    const result = await triggerNativeHaptic({
      type: 'view',
      effect: 'keyboard_tap',
    });

    expect(result.performed).toBe(true);
    expect(result.constant).toBe(3);
    expect(invokeMock).toHaveBeenCalledWith('plugin:vcp-mobile|trigger_haptic_feedback', {
      payload: {
        type: 'view',
        effect: 'keyboard_tap',
      },
    });
  });

  it('falls back to navigator.vibrate when native invoke fails', async () => {
    mockInvoke('plugin:vcp-mobile|trigger_haptic_feedback', () => {
      throw new Error('plugin not supported on this platform');
    });

    const result = await triggerNativeHaptic({
      type: 'one_shot',
      durationMs: 15,
      amplitude: 60,
    });

    expect(result.fallback).toBe(true);
    expect(result.performed).toBe(true);
    expect(vi.mocked(navigator.vibrate)).toHaveBeenCalledWith(15);
  });

  it('renders HapticsDebugCard and triggers native haptic on button click', async () => {
    mockInvoke('plugin:vcp-mobile|trigger_haptic_feedback', () => ({
      performed: true,
      type: 'view',
      effect: 'keyboard_tap',
      constant: 3,
    }));

    const wrapper = mount(HapticsDebugCard, {
      props: { active: true },
    });

    expect(wrapper.text()).toContain('系统级触感调试实验室 (Debug)');
    expect(wrapper.text()).toContain('键盘轻点 (TAP)');

    const tapButton = wrapper.findAll('button').find((b) => b.text().includes('键盘轻点'));
    expect(tapButton).toBeDefined();

    await tapButton!.trigger('click');

    expect(invokeMock).toHaveBeenCalledWith('plugin:vcp-mobile|trigger_haptic_feedback', {
      payload: {
        type: 'view',
        effect: 'keyboard_tap',
      },
    });

    // Check last log updated
    expect(wrapper.text()).toContain('键盘轻点 (KEYBOARD_TAP)');
  });
});

import { invoke } from '@tauri-apps/api/core';

export interface NativeHapticFeedbackPayload {
  type: 'view' | 'predefined' | 'one_shot';
  effect?: string;
  durationMs?: number;
  amplitude?: number;
}

export interface NativeHapticFeedbackResult {
  performed?: boolean;
  type?: string;
  effect?: string;
  constant?: number;
  effectId?: number;
  hasAmplitudeControl?: boolean;
  durationMs?: number;
  amplitude?: number;
  fallback?: boolean;
  reason?: string;
}

/**
 * 触发系统级/原生触觉反馈。
 * 优先调用移动端插件命令；若在非 Android 环境或调用失败，自动回退到 Web Vibration API。
 */
export async function triggerNativeHaptic(
  payload: NativeHapticFeedbackPayload,
): Promise<NativeHapticFeedbackResult> {
  try {
    const result = await invoke<NativeHapticFeedbackResult>('plugin:vcp-mobile|trigger_haptic_feedback', {
      payload,
    });
    return result;
  } catch (error) {
    // 非 Android 环境或模拟器 fallback
    if (typeof navigator !== 'undefined' && navigator.vibrate) {
      const ms = payload.durationMs || (payload.type === 'view' ? 15 : 20);
      try {
        navigator.vibrate(ms);
        return {
          performed: true,
          type: payload.type,
          effect: payload.effect,
          fallback: true,
          durationMs: ms,
          reason: `fallback_navigator_vibrate: ${String(error)}`,
        };
      } catch (vibErr) {
        return { performed: false, reason: String(vibErr) };
      }
    }
    return { performed: false, reason: String(error) };
  }
}

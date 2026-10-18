import { invoke } from '@tauri-apps/api/core';

export interface NativeHapticFeedbackPayload {
  type: 'message' | 'view' | 'predefined' | 'one_shot' | 'impact' | 'selection' | 'notification';
  effect?: string;
  durationMs?: number;
  amplitude?: number;
  intensity?: number;
}

export interface NativeHapticFeedbackResult {
  /** The native API accepted the request; hardware playback remains system-controlled. */
  submitted: boolean;
  type?: string;
  effect?: string;
  constant?: number;
  effectId?: number;
  hasAmplitudeControl?: boolean;
  durationMs?: number;
  amplitude?: number;
  intensity?: number;
  resolvedEffect?: string;
  fallback?: boolean;
  reason?: string;
}

/**
 * Submit one native haptic request. Unsupported or suppressed feedback stays silent:
 * falling back to a timed Web vibration would change the selected effect and system policy.
 */
export async function triggerNativeHaptic(
  payload: NativeHapticFeedbackPayload,
): Promise<NativeHapticFeedbackResult> {
  try {
    const result = await invoke<NativeHapticFeedbackResult>('plugin:vcp-mobile|trigger_haptic_feedback', {
      payload,
    });
    return typeof result?.submitted === 'boolean'
      ? result
      : { submitted: false, reason: 'invalid_native_haptic_result' };
  } catch (error) {
    return { submitted: false, reason: String(error) };
  }
}

/** Fire-and-forget enhancement; the chat owners decide eligibility and read the live preference. */
export function triggerMessageHaptic(effect: 'send' | 'complete'): void {
  void triggerNativeHaptic({ type: 'message', effect });
}

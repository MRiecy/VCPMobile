import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

export interface VoiceAudioFrame {
  kind: 'frame';
  sessionId: string;
  audioEpoch: number;
  sampleIndex: number;
  probability: number;
  level: number;
  bands: readonly number[];
  aecState: 'enabled' | 'unavailable' | 'failed';
  route: string;
  playbackActive: boolean;
  playbackId?: string | null;
  probeRevision: number;
  speechActive: boolean;
  speechTransition: 'start' | 'end' | null;
  discontinuity: boolean;
}

export interface VoiceAudioEnd {
  kind: 'stopped' | 'error';
  sessionId: string;
  audioEpoch: number;
  reason: string;
  sampleIndex?: number;
}

export interface CaptureStopReceipt {
  audioEpoch: number;
  sampleIndex: number;
}

interface CaptureRequest {
  sessionId: string;
  epoch: number;
  sampleIndex: number;
  startSubmitted: boolean;
  running: boolean;
  unlisten?: UnlistenFn;
  heartbeat?: ReturnType<typeof setTimeout>;
  cancel: () => void;
  cancelled: Promise<false>;
}

const SILENT_BANDS: readonly number[] = Object.freeze(Array<number>(32).fill(0));

/** Native capture owns PCM, AEC and VAD. Vue receives only bounded display data. */
export function createMicrophoneMonitor(
  onLevel: (level: number, bands: readonly number[]) => void,
  onEnded: (event: VoiceAudioEnd) => void,
  onFrame: (frame: VoiceAudioFrame) => void,
  lifecycle: { beforeStart: (sessionId: string) => Promise<string>; onStop: (sessionId: string, receipt?: CaptureStopReceipt) => void },
) {
  let current: CaptureRequest | null = null;
  let pendingStop: Promise<CaptureStopReceipt> | null = null;

  const stopNative = (request: CaptureRequest, beforeNextStart = true) => {
    // Never await start or its permission dialog before submitting cancellation.
    const stopping = invoke<CaptureStopReceipt>('plugin:vcp-mobile|stop_voice_audio', { sessionId: request.sessionId });
    if (beforeNextStart) pendingStop = stopping;
    void stopping.finally(() => {
      if (pendingStop === stopping) pendingStop = null;
    }).catch(error => console.warn('[VoiceAudio] Failed to stop native capture:', String(error).slice(0, 512)));
    return stopping;
  };

  const release = (request: CaptureRequest, cancel = true, inputStopped = true, receipt?: CaptureStopReceipt) => {
    if (current === request) current = null;
    if (cancel) request.cancel();
    clearTimeout(request.heartbeat);
    request.heartbeat = undefined;
    request.unlisten?.();
    request.unlisten = undefined;
    if (inputStopped) lifecycle.onStop(request.sessionId, receipt);
  };

  const haltInput = (request: CaptureRequest, cancel = true) => {
    release(request, cancel, false);
    if (request.startSubmitted) {
      // Seal native PCM before asking ASR to drain the already-recorded input.
      // UI cancellation remains immediate; accepted recognition is independent.
      void stopNative(request).then(receipt => lifecycle.onStop(request.sessionId, receipt),
        () => lifecycle.onStop(request.sessionId)).catch(() => {});
    } else lifecycle.onStop(request.sessionId);
  };

  const stop = () => {
    const request = current;
    if (request) haltInput(request);
    onLevel(0, SILENT_BANDS);
  };

  const armHeartbeat = (request: CaptureRequest) => {
    clearTimeout(request.heartbeat);
    request.heartbeat = setTimeout(() => {
      if (current !== request) return;
      haltInput(request);
      onLevel(0, SILENT_BANDS);
      onEnded({ kind: 'error', sessionId: request.sessionId, audioEpoch: Math.max(0, request.epoch), reason: 'voice_events_timeout' });
    }, 3000);
  };

  const start = (): Promise<boolean> => {
    stop();
    const previousStop = pendingStop;
    let cancel!: () => void;
    const cancelled = new Promise<false>(resolve => { cancel = () => resolve(false); });
    const request: CaptureRequest = {
      sessionId: crypto.randomUUID(), epoch: -1, sampleIndex: -1,
      startSubmitted: false, running: false, cancel, cancelled,
    };
    current = request;
    const work = (async () => {
      try {
        const unlisten = await listen<VoiceAudioFrame | VoiceAudioEnd>('vcp-voice-audio', ({ payload }) => {
          if (current !== request || !request.startSubmitted || payload.sessionId !== request.sessionId) return;
          if (!Number.isSafeInteger(payload.audioEpoch) || payload.audioEpoch < 0 || payload.audioEpoch < request.epoch) return;
          if (payload.kind === 'frame') {
            if (!Number.isSafeInteger(payload.probeRevision) || payload.probeRevision < 0) return;
            if (!Number.isSafeInteger(payload.sampleIndex) || payload.sampleIndex < 0
              || (payload.audioEpoch === request.epoch && payload.sampleIndex <= request.sampleIndex)) return;
            if (!Number.isFinite(payload.level) || !Number.isFinite(payload.probability)
              || !Array.isArray(payload.bands) || payload.bands.length !== 32
              || !payload.bands.every(Number.isFinite)) return;
            request.epoch = payload.audioEpoch;
            request.sampleIndex = payload.sampleIndex;
            if (request.running) armHeartbeat(request);
            // Bound values at the UI boundary; neither raw PCM nor full-session
            // history is copied across the native event channel.
            const frame = {
              ...payload, level: Math.min(1, Math.max(0, payload.level)),
              probability: Math.min(1, Math.max(0, payload.probability)),
              bands: payload.bands.map(value => Math.min(1, Math.max(0, value))),
            };
            onLevel(frame.level, frame.bands);
            onFrame(frame);
          } else if (payload.kind === 'stopped' || payload.kind === 'error') {
            release(request, true, true, typeof payload.sampleIndex === 'number'
              ? { audioEpoch: payload.audioEpoch, sampleIndex: payload.sampleIndex } : undefined);
            onLevel(0, SILENT_BANDS);
            onEnded(payload);
          }
        });
        if (current !== request) {
          unlisten();
          return false;
        }
        request.unlisten = unlisten;
        // The next start waits only for the preceding stop acknowledgement,
        // never for an old permission dialog/start result. Rust then admits the
        // new UUID after the old owner has actually been revoked.
        if (previousStop) await previousStop;
        if (current !== request) return false;
        const callId = await lifecycle.beforeStart(request.sessionId);
        if (current !== request) return false;
        request.startSubmitted = true;
        await invoke('plugin:vcp-mobile|start_voice_audio', { sessionId: request.sessionId, callId });
        if (current !== request) {
          // Native also guards late start completion; target this old UUID only.
          stopNative(request, false);
          return false;
        }
        request.running = true;
        // Silence still produces frames. Only a running capture with no valid
        // frames times out; permission/startup waits have no heartbeat timer.
        armHeartbeat(request);
        return true;
      } catch (error) {
        if (current !== request) return false;
        // Preserve a failed start as a rejection instead of resolving its
        // cancellation race before the real permission/runtime error arrives.
        haltInput(request, false);
        onLevel(0, SILENT_BANDS);
        throw error;
      }
    })();
    return Promise.race([work, cancelled]);
  };

  const setProbe = async (enabled: boolean): Promise<boolean> => {
    const request = current;
    if (!request?.running) return false;
    const work = (async () => {
      await invoke('plugin:vcp-mobile|set_voice_probe', { sessionId: request.sessionId, enabled });
      // Rust/native probe revisions own ordering. A late acknowledgement must
      // never become a new playback intent that overrides a later user action.
      return current === request;
    })();
    return Promise.race([work, request.cancelled]);
  };

  return { start, stop, setProbe };
}

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

interface AudioObservation {
  sessionId: string;
  epoch: number;
  sampleIndex: number;
  unlisten?: UnlistenFn;
}

interface CaptureRequest extends AudioObservation {
  callId?: string;
  expectedLifecycleEpoch?: number | null;
  startSubmitted: boolean;
  cancel: () => void;
  cancelled: Promise<false>;
}

const SILENT_BANDS: readonly number[] = Object.freeze(Array<number>(32).fill(0));

/** Native capture owns PCM, AEC and VAD. Vue receives only bounded display data. */
export function createMicrophoneMonitor(
  onLevel: (level: number, bands: readonly number[]) => void,
  onEnded: (event: VoiceAudioEnd) => void,
  onFrame: (frame: VoiceAudioFrame) => void,
  lifecycle: {
    beforeStart: (sessionId: string) => Promise<string>;
    closeCurrent: () => Promise<void>;
    currentLifecycleEpoch?: () => number | null;
    onConfigured?: (configured: boolean) => void;
  },
) {
  let current: CaptureRequest | null = null;
  let observed: AudioObservation | null = null;
  let pendingStop: Promise<void> | null = null;

  const deliver = (cursor: AudioObservation, payload: VoiceAudioFrame | VoiceAudioEnd, finish: () => void) => {
    if (payload.sessionId !== cursor.sessionId || !Number.isSafeInteger(payload.audioEpoch)
      || payload.audioEpoch < 0 || payload.audioEpoch < cursor.epoch) return;
    if (payload.kind === 'frame') {
      if (!Number.isSafeInteger(payload.sampleIndex) || payload.sampleIndex < 0
        || (payload.audioEpoch === cursor.epoch && payload.sampleIndex <= cursor.sampleIndex)) return;
      if (!Number.isFinite(payload.level) || !Number.isFinite(payload.probability)
        || !Array.isArray(payload.bands) || payload.bands.length !== 32
        || !payload.bands.every(Number.isFinite)) return;
      cursor.epoch = payload.audioEpoch;
      cursor.sampleIndex = payload.sampleIndex;
      // Both fresh and recovered views receive only bounded display data.
      const frame = {
        ...payload, level: Math.min(1, Math.max(0, payload.level)),
        probability: Math.min(1, Math.max(0, payload.probability)),
        bands: payload.bands.map(value => Math.min(1, Math.max(0, value))),
      };
      onLevel(frame.level, frame.bands);
      onFrame(frame);
    } else if (payload.kind === 'stopped' || payload.kind === 'error') {
      // Native terminals already seal input in Rust; this is display cleanup.
      finish();
      onLevel(0, SILENT_BANDS);
      onEnded(payload);
    }
  };

  const releaseObserved = () => {
    observed?.unlisten?.();
    observed = null;
  };

  const observe = (sessionId: string | null) => {
    if (observed?.sessionId === sessionId) return;
    releaseObserved();
    if (!sessionId || current?.sessionId === sessionId) return;
    const observation: AudioObservation = { sessionId, epoch: -1, sampleIndex: -1 };
    observed = observation;
    void listen<VoiceAudioFrame | VoiceAudioEnd>('vcp-voice-audio', ({ payload }) => {
      if (observed === observation) deliver(observation, payload, releaseObserved);
    }).then(unlisten => {
      if (observed === observation) observation.unlisten = unlisten;
      else unlisten();
    }).catch(error => {
      if (observed === observation) observed = null;
      console.warn('[VoiceAudio] Display listener unavailable:', String(error).slice(0, 512));
    });
  };

  const trackStop = (stopping: Promise<void>, beforeNextStart = true) => {
    if (beforeNextStart) pendingStop = stopping;
    void stopping.finally(() => {
      if (pendingStop === stopping) pendingStop = null;
    }).catch(error => console.warn('[VoiceAudio] Failed to stop native capture:', String(error).slice(0, 512)));
    return stopping;
  };

  const stopNative = (request: CaptureRequest, beforeNextStart = true) => {
    // Never await start or its permission dialog before submitting cancellation.
    return trackStop(invoke<void>('close_voice_microphone', {
      callId: request.callId, sessionId: request.sessionId,
    }), beforeNextStart);
  };

  const release = (request: CaptureRequest, cancel = true) => {
    if (current === request) current = null;
    if (cancel) request.cancel();
    request.unlisten?.();
    request.unlisten = undefined;
  };

  const haltInput = (request: CaptureRequest, cancel = true) => {
    release(request, cancel);
    // Rust closes capture and seals ASR in one intent even if this WebView
    // suspends before the acknowledgement. No second JS completion is needed.
    return request.startSubmitted ? stopNative(request) : Promise.resolve();
  };

  const stop = () => {
    const request = current;
    const previousObservation = observed;
    // A recovered view has no CaptureRequest. Its close intent still belongs
    // to the Rust call owner; absence of a JS monitor is not evidence of silence.
    const stopping = request ? haltInput(request) : pendingStop ?? trackStop(lifecycle.closeCurrent());
    return stopping.then(() => {
      if (observed === previousObservation) releaseObserved();
      if (!current && !observed) onLevel(0, SILENT_BANDS);
    });
  };

  const detach = () => {
    if (current) release(current);
    releaseObserved();
    onLevel(0, SILENT_BANDS);
  };

  const start = (): Promise<boolean> => {
    if (current) void haltInput(current).catch(() => {});
    releaseObserved();
    const previousStop = pendingStop;
    let cancel!: () => void;
    const cancelled = new Promise<false>(resolve => { cancel = () => resolve(false); });
    const request: CaptureRequest = {
      sessionId: crypto.randomUUID(), epoch: -1, sampleIndex: -1,
      expectedLifecycleEpoch: lifecycle.currentLifecycleEpoch?.(),
      startSubmitted: false, cancel, cancelled,
    };
    current = request;
    const work = (async () => {
      try {
        const unlisten = await listen<VoiceAudioFrame | VoiceAudioEnd>('vcp-voice-audio', ({ payload }) => {
          if (current === request && request.startSubmitted) deliver(request, payload, () => release(request));
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
        request.callId = callId;
        request.startSubmitted = true;
        const result = await invoke<{ configured: boolean }>('start_voice_microphone', {
          sessionId: request.sessionId, callId,
          ...(request.expectedLifecycleEpoch === undefined ? {} : { expectedLifecycleEpoch: request.expectedLifecycleEpoch }),
        });
        if (current !== request) {
          // Native also guards late start completion; target this old UUID only.
          stopNative(request, false);
          return false;
        }
        lifecycle.onConfigured?.(result.configured);
        // A WebView can suspend event delivery while Rust continues the call.
        // Native capture owns health and terminal detection; JS has no timeout.
        return true;
      } catch (error) {
        if (current !== request) return false;
        // Preserve a failed start as a rejection instead of resolving its
        // cancellation race before the real permission/runtime error arrives.
        void haltInput(request, false).catch(() => {});
        onLevel(0, SILENT_BANDS);
        throw error;
      }
    })();
    return Promise.race([work, cancelled]);
  };

  return { start, stop, observe, detach };
}

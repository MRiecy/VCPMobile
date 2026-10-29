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

const SILENT_BANDS: readonly number[] = Object.freeze(Array<number>(32).fill(0));

/** Native capture owns PCM, AEC and VAD. Vue receives only bounded display data. */
export function createMicrophoneMonitor(
  onLevel: (level: number, bands: readonly number[]) => void,
  onEnded: (event: VoiceAudioEnd) => void,
  onFrame: (frame: VoiceAudioFrame) => void,
) {
  let observed: AudioObservation | null = null;

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
    if (!sessionId) return;
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

  const detach = () => {
    releaseObserved();
    onLevel(0, SILENT_BANDS);
  };
  return { observe, detach };
}

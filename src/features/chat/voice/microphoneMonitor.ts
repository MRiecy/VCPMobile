import { invoke } from '@tauri-apps/api/core';
import { isAndroid } from '../../../core/utils/platform';

const SILENT_BANDS: readonly number[] = Object.freeze(Array<number>(32).fill(0));

/** Live input metering only: no recorder, retained audio, network or playback. */
export function createMicrophoneMonitor(onLevel: (level: number, bands: readonly number[]) => void, onEnded: () => void) {
  let generation = 0;
  let pending: Promise<boolean> | null = null;
  let stream: MediaStream | null = null;
  let context: AudioContext | null = null;
  let source: MediaStreamAudioSourceNode | null = null;
  let analyser: AnalyserNode | null = null;
  let frame = 0;
  let cancelResume: (() => void) | null = null;

  const release = () => {
    cancelResume?.();
    cancelResume = null;
    cancelAnimationFrame(frame);
    frame = 0;
    source?.disconnect();
    analyser?.disconnect();
    source = null;
    analyser = null;
    stream?.getTracks().forEach(track => {
      track.onended = null;
      track.onmute = null;
      track.stop();
    });
    stream = null;
    const oldContext = context;
    context = null;
    if (oldContext) oldContext.onstatechange = null;
    if (oldContext && oldContext.state !== 'closed') void oldContext.close().catch(() => {});
    onLevel(0, SILENT_BANDS);
  };

  const stop = () => {
    generation += 1;
    release();
  };

  const start = (): Promise<boolean> => {
    const request = ++generation;
    release();
    const previous = pending;
    const work = (async () => {
      if (!navigator.mediaDevices?.getUserMedia) {
        throw new Error('当前环境无法启动麦克风。');
      }
      const audioContext = new AudioContext();
      context = audioContext;
      const cancelled = new Promise<null>(resolve => { cancelResume = () => resolve(null); });
      // Resume in the original user activation, before waiting for OS permission.
      const resumed = audioContext.resume().then(() => null, error => error);
      try {
        // A system dialog cannot be cancelled. Only permission/acquisition is
        // serialized; this request's AudioContext has already used its gesture.
        if (previous) await previous.catch(() => false);
        if (request !== generation) return false;
        if (isAndroid) {
          await invoke('plugin:vcp-mobile|request_android_permission', { pType: 'microphone' });
          if (request !== generation) return false;
          const permission = await invoke<{ microphone: boolean }>('plugin:vcp-mobile|check_all_permissions');
          if (request !== generation) return false;
          if (!permission.microphone) throw new DOMException('麦克风权限未开启', 'NotAllowedError');
        }
        // iOS requests OS microphone access through WebKit; its purpose string
        // lives in Info.ios.plist. Browser permission is still required on Android.
        const acquired = await navigator.mediaDevices.getUserMedia({
          audio: { echoCancellation: true, noiseSuppression: true },
          video: false,
        });
        if (request !== generation) {
          acquired.getTracks().forEach(track => track.stop());
          return false;
        }
        stream = acquired;
        const resumeError = await Promise.race([resumed, cancelled]);
        if (request !== generation) return false;
        if (resumeError) throw resumeError;
        if (audioContext.state !== 'running') throw new Error('音频输入已暂停，请点击麦克风重试。');
        if (!acquired.getAudioTracks().some(track => track.readyState === 'live' && !track.muted)) {
          throw new Error('麦克风未提供有效音轨，请重试。');
        }
        source = audioContext.createMediaStreamSource(acquired);
        const meter = audioContext.createAnalyser();
        analyser = meter;
        meter.fftSize = 2048;
        meter.minDecibels = -80;
        meter.maxDecibels = -20;
        meter.smoothingTimeConstant = 0.15;
        source.connect(meter);
        const samples = new Float32Array(meter.fftSize);
        const spectrum = new Uint8Array(meter.frequencyBinCount);
        const bands = new Float32Array(SILENT_BANDS.length);
        const binHz = audioContext.sampleRate / meter.fftSize;
        const upperHz = Math.min(8000, audioContext.sampleRate / 2);
        const ranges = Array.from(bands, (_, index) => {
          const low = Math.floor(80 * (upperHz / 80) ** (index / bands.length) / binHz);
          const high = Math.ceil(80 * (upperHz / 80) ** ((index + 1) / bands.length) / binHz);
          return [Math.min(low, spectrum.length - 1), Math.min(Math.max(low + 1, high), spectrum.length)] as const;
        });
        let lastSample = 0;
        let smoothed = 0;
        const measure = (now: number) => {
          if (request !== generation) return;
          if (now - lastSample >= 32) {
            lastSample = now;
            meter.getFloatTimeDomainData(samples);
            meter.getByteFrequencyData(spectrum);
            let energy = 0;
            for (const sample of samples) energy += sample * sample;
            const rms = Math.sqrt(energy / samples.length);
            // Perceptual gain makes ordinary speech visible without letting
            // tiny background energy animate a silent microphone. This is a
            // display curve, not a speech/VAD decision.
            const value = Math.min(1, (Math.max(0, rms - 0.002) / 0.06) ** 0.65);
            smoothed += (value - smoothed) * (value > smoothed ? 0.9 : 0.5);
            ranges.forEach(([low, high], index) => {
              let peak = 0;
              for (let bin = low; bin < high; bin++) peak = Math.max(peak, spectrum[bin]);
              const target = Math.min(1, (peak / 255) ** 0.85 * Math.sqrt(value) * 1.25);
              bands[index] += (target - bands[index]) * (target > bands[index] ? 0.9 : 0.45);
            });
            // A bounded 32-value snapshot gives Vue an observable new frame;
            // PCM/FFT buffers are reused and never retained in UI state.
            onLevel(smoothed, Array.from(bands));
          }
          frame = requestAnimationFrame(measure);
        };
        const interrupted = () => {
          if (request !== generation) return;
          stop();
          onEnded();
        };
        acquired.getAudioTracks().forEach(track => {
          track.onended = interrupted;
          track.onmute = interrupted;
        });
        audioContext.onstatechange = () => {
          if (audioContext.state !== 'running') interrupted();
        };
        frame = requestAnimationFrame(measure);
        return true;
      } catch (error) {
        if (request !== generation) return false;
        release();
        throw error;
      }
    })();
    pending = work;
    void work.finally(() => { if (pending === work) pending = null; }).catch(() => {});
    return work;
  };

  return { start, stop };
}

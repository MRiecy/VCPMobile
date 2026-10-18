import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushPromises } from '@vue/test-utils';
import { createMicrophoneMonitor } from '../../../features/chat/voice/microphoneMonitor';
import { invokeMock, mockInvoke } from '../../mocks/tauri';

const platform = vi.hoisted(() => ({ android: false }));
vi.mock('../../../core/utils/platform', () => ({
  get isAndroid() { return platform.android; },
}));

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((finish, fail) => { resolve = finish; reject = fail; });
  return { promise, resolve, reject };
}

function makeStream() {
  const track = {
    readyState: 'live',
    muted: false,
    onended: null as (() => void) | null,
    onmute: null as (() => void) | null,
    stop: vi.fn(),
  };
  return {
    track,
    stream: { getTracks: () => [track], getAudioTracks: () => [track] } as unknown as MediaStream,
  };
}

const contexts: MockAudioContext[] = [];
const resumePlans: Array<(context: MockAudioContext) => Promise<void>> = [];
class MockAudioContext {
  state = 'suspended';
  sampleRate = 48000;
  onstatechange: (() => void) | null = null;
  destination = {};
  meter = {
    fftSize: 0,
    get frequencyBinCount() { return this.fftSize / 2; },
    disconnect: vi.fn(),
    getFloatTimeDomainData: vi.fn((samples: Float32Array) => samples.fill(0.2)),
    getByteFrequencyData: vi.fn((spectrum: Uint8Array) => spectrum.fill(128)),
  };
  source = { connect: vi.fn(), disconnect: vi.fn() };
  resume = vi.fn(async () => {
    const plan = resumePlans.shift();
    if (plan) await plan(this);
    else this.setState('running');
  });
  close = vi.fn(async () => { this.setState('closed'); });
  createMediaStreamSource = vi.fn(() => this.source);
  createAnalyser = vi.fn(() => this.meter);
  constructor() { contexts.push(this); }
  setState(state: string) {
    this.state = state;
    this.onstatechange?.();
  }
}

const frames = new Map<number, FrameRequestCallback>();
let nextFrame = 0;
const getUserMedia = vi.fn<() => Promise<MediaStream>>();
const network = vi.fn();
const blob = vi.fn();
const recorder = vi.fn();
const socket = vi.fn();
let oldMediaDevices: PropertyDescriptor | undefined;
let monitor: ReturnType<typeof createMicrophoneMonitor> | undefined;

const runFrame = (time: number) => {
  const pending = [...frames.values()];
  frames.clear();
  pending.forEach(callback => callback(time));
};

beforeEach(() => {
  platform.android = false;
  contexts.length = 0;
  resumePlans.length = 0;
  frames.clear();
  nextFrame = 0;
  getUserMedia.mockReset();
  oldMediaDevices = Object.getOwnPropertyDescriptor(navigator, 'mediaDevices');
  Object.defineProperty(navigator, 'mediaDevices', {
    configurable: true, value: { getUserMedia },
  });
  vi.stubGlobal('AudioContext', MockAudioContext);
  vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => {
    const id = ++nextFrame;
    frames.set(id, callback);
    return id;
  });
  vi.stubGlobal('cancelAnimationFrame', (id: number) => frames.delete(id));
  vi.stubGlobal('fetch', network);
  vi.stubGlobal('WebSocket', socket);
  vi.stubGlobal('Blob', blob);
  vi.stubGlobal('MediaRecorder', recorder);
  mockInvoke('plugin:vcp-mobile|request_android_permission', () => undefined);
  mockInvoke('plugin:vcp-mobile|check_all_permissions', () => ({ microphone: true }));
});

afterEach(() => {
  monitor?.stop();
  monitor = undefined;
  vi.unstubAllGlobals();
  if (oldMediaDevices) Object.defineProperty(navigator, 'mediaDevices', oldMediaDevices);
  else Reflect.deleteProperty(navigator, 'mediaDevices');
});

describe('microphone input metering', () => {
  it('releases the audio context when browser microphone permission is denied', async () => {
    const error = new DOMException('Denied', 'NotAllowedError');
    getUserMedia.mockRejectedValue(error);
    const onEnded = vi.fn();
    monitor = createMicrophoneMonitor(vi.fn(), onEnded);

    await expect(monitor.start()).rejects.toBe(error);

    expect(contexts[0].close).toHaveBeenCalledOnce();
    expect(contexts[0].createMediaStreamSource).not.toHaveBeenCalled();
    expect(frames.size).toBe(0);
    expect(onEnded).not.toHaveBeenCalled();
    expect(network).not.toHaveBeenCalled();
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it('does not acquire a browser stream after Android denies microphone permission', async () => {
    platform.android = true;
    mockInvoke('plugin:vcp-mobile|check_all_permissions', () => ({ microphone: false }));
    monitor = createMicrophoneMonitor(vi.fn(), vi.fn());

    await expect(monitor.start()).rejects.toMatchObject({ name: 'NotAllowedError' });

    expect(invokeMock).toHaveBeenCalledWith('plugin:vcp-mobile|request_android_permission', { pType: 'microphone' });
    expect(getUserMedia).not.toHaveBeenCalled();
    expect(contexts[0].close).toHaveBeenCalledOnce();
  });

  it('stops a stream delivered after cancellation without installing a meter', async () => {
    const request = deferred<MediaStream>();
    const acquired = makeStream();
    getUserMedia.mockReturnValue(request.promise);
    monitor = createMicrophoneMonitor(vi.fn(), vi.fn());
    const started = monitor.start();

    monitor.stop();
    request.resolve(acquired.stream);

    await expect(started).resolves.toBe(false);
    expect(acquired.track.stop).toHaveBeenCalledOnce();
    expect(contexts[0].close).toHaveBeenCalledOnce();
    expect(contexts[0].createMediaStreamSource).not.toHaveBeenCalled();
    expect(frames.size).toBe(0);
  });

  it('keeps a new start separate from an older unresolved permission request', async () => {
    const firstRequest = deferred<MediaStream>();
    const oldStream = makeStream();
    const newStream = makeStream();
    getUserMedia.mockReturnValueOnce(firstRequest.promise).mockResolvedValueOnce(newStream.stream);
    monitor = createMicrophoneMonitor(vi.fn(), vi.fn());
    const first = monitor.start();
    monitor.stop();
    const second = monitor.start();
    expect(getUserMedia).toHaveBeenCalledTimes(1);
    // The new user gesture must unlock its AudioContext immediately, even
    // while browser acquisition waits for the previous permission result.
    expect(contexts).toHaveLength(2);
    expect(contexts[1].resume).toHaveBeenCalledOnce();

    firstRequest.resolve(oldStream.stream);
    await expect(first).resolves.toBe(false);
    await expect(second).resolves.toBe(true);

    expect(oldStream.track.stop).toHaveBeenCalledOnce();
    expect(newStream.track.stop).not.toHaveBeenCalled();
    expect(contexts[0].createMediaStreamSource).not.toHaveBeenCalled();
    expect(contexts[1].createMediaStreamSource).toHaveBeenCalledWith(newStream.stream);
    monitor.stop();
    expect(newStream.track.stop).toHaveBeenCalledOnce();
  });

  it('lets a new start proceed after stop cancels an unresolved AudioContext resume', async () => {
    const resume = deferred<void>();
    resumePlans.push(() => resume.promise);
    const oldStream = makeStream();
    const newStream = makeStream();
    getUserMedia.mockResolvedValueOnce(oldStream.stream).mockResolvedValueOnce(newStream.stream);
    monitor = createMicrophoneMonitor(vi.fn(), vi.fn());
    const first = monitor.start();
    await flushPromises();
    expect(contexts[0].state).toBe('suspended');
    expect(oldStream.track.stop).not.toHaveBeenCalled();

    monitor.stop();
    const second = monitor.start();

    await expect(first).resolves.toBe(false);
    await expect(second).resolves.toBe(true);
    expect(oldStream.track.stop).toHaveBeenCalledOnce();
    expect(contexts[0].close).toHaveBeenCalledOnce();
    expect(newStream.track.stop).not.toHaveBeenCalled();
    expect(contexts[1].state).toBe('running');

    resume.reject(new Error('late resume failure'));
    await flushPromises();
    expect(newStream.track.stop).not.toHaveBeenCalled();
    expect(contexts[1].close).not.toHaveBeenCalled();
  });

  it.each(['suspended-context', 'muted-track'] as const)('does not announce a running monitor with %s', async condition => {
    const acquired = makeStream();
    if (condition === 'suspended-context') resumePlans.push(async () => {});
    else acquired.track.muted = true;
    getUserMedia.mockResolvedValue(acquired.stream);
    const onEnded = vi.fn();
    monitor = createMicrophoneMonitor(vi.fn(), onEnded);

    await expect(monitor.start()).rejects.toThrow();

    expect(acquired.track.stop).toHaveBeenCalledOnce();
    expect(contexts[0].close).toHaveBeenCalledOnce();
    expect(contexts[0].createMediaStreamSource).not.toHaveBeenCalled();
    expect(frames.size).toBe(0);
    expect(onEnded).not.toHaveBeenCalled();
  });

  it('does not acquire audio if cancelled while an Android permission dialog is pending', async () => {
    platform.android = true;
    const permission = deferred<void>();
    mockInvoke('plugin:vcp-mobile|request_android_permission', () => permission.promise);
    monitor = createMicrophoneMonitor(vi.fn(), vi.fn());
    const started = monitor.start();
    monitor.stop();
    permission.resolve();

    await expect(started).resolves.toBe(false);
    expect(getUserMedia).not.toHaveBeenCalled();
    expect(contexts[0].close).toHaveBeenCalledOnce();
  });

  it('samples at a bounded rate into one reusable buffer without recording or sending audio', async () => {
    const acquired = makeStream();
    getUserMedia.mockResolvedValue(acquired.stream);
    const onLevel = vi.fn();
    monitor = createMicrophoneMonitor(onLevel, vi.fn());
    await expect(monitor.start()).resolves.toBe(true);
    onLevel.mockClear();

    for (let time = 10; time <= 1000; time += 10) runFrame(time);

    const samples = contexts[0].meter.getFloatTimeDomainData.mock.calls.map(([buffer]) => buffer);
    expect(samples.length).toBeGreaterThan(20);
    expect(samples.length).toBeLessThanOrEqual(32);
    expect(new Set(samples).size).toBe(1);
    expect(samples[0].byteLength).toBeLessThanOrEqual(8192);
    const spectra = contexts[0].meter.getByteFrequencyData.mock.calls.map(([buffer]) => buffer);
    expect(new Set(spectra).size).toBe(1);
    expect(spectra[0].byteLength).toBeLessThanOrEqual(2048);
    expect(onLevel.mock.calls.every(([value]) => value >= 0 && value <= 1)).toBe(true);
    expect(onLevel.mock.calls.some(([value]) => value > 0)).toBe(true);
    expect(contexts[0].source.connect).toHaveBeenCalledExactlyOnceWith(contexts[0].meter);
    expect(network).not.toHaveBeenCalled();
    expect(socket).not.toHaveBeenCalled();
    expect(blob).not.toHaveBeenCalled();
    expect(recorder).not.toHaveBeenCalled();
    expect(invokeMock).not.toHaveBeenCalled();

    monitor.stop();
    const callsAfterStop = onLevel.mock.calls.length;
    runFrame(1100);
    expect(onLevel).toHaveBeenCalledTimes(callsAfterStop);
    expect(onLevel).toHaveBeenLastCalledWith(0, Array(32).fill(0));
    expect(frames.size).toBe(0);
    expect(acquired.track.stop).toHaveBeenCalledOnce();
  });

  it('moves the dominant bars with the measured frequencies instead of pulsing one envelope', async () => {
    getUserMedia.mockResolvedValue(makeStream().stream);
    const onLevel = vi.fn();
    monitor = createMicrophoneMonitor(onLevel, vi.fn());
    await monitor.start();
    const meter = contexts[0].meter;
    meter.getFloatTimeDomainData.mockImplementation(samples => samples.fill(0.025));
    let peakBin = 20;
    meter.getByteFrequencyData.mockImplementation(spectrum => {
      spectrum.fill(0);
      spectrum[peakBin] = 200;
      return spectrum;
    });
    runFrame(40);
    const lowBands = onLevel.mock.calls[onLevel.mock.calls.length - 1][1] as number[];
    const lowPeak = lowBands.indexOf(Math.max(...lowBands));
    expect(Math.max(...lowBands)).toBeGreaterThan(0);
    expect(lowBands.some(value => value === 0)).toBe(true);

    peakBin = 120;
    runFrame(80);
    runFrame(120);
    const highBands = onLevel.mock.calls[onLevel.mock.calls.length - 1][1] as number[];
    const highPeak = highBands.indexOf(Math.max(...highBands));
    expect(highPeak).toBeGreaterThan(lowPeak);
    expect(highBands[lowPeak]).toBeLessThan(lowBands[lowPeak]);
    expect(highBands).not.toBe(lowBands);
    expect(highBands.every(value => Number.isFinite(value) && value >= 0 && value <= 1)).toBe(true);

    meter.getFloatTimeDomainData.mockImplementation(samples => samples.fill(0));
    for (let time = 160; time <= 1160; time += 40) runFrame(time);
    const quietBands = onLevel.mock.calls[onLevel.mock.calls.length - 1][1] as number[];
    expect(quietBands.every(value => value < 0.001)).toBe(true);
  });

  it('releases live resources and reports an unexpected track end once', async () => {
    const acquired = makeStream();
    getUserMedia.mockResolvedValue(acquired.stream);
    const onEnded = vi.fn();
    monitor = createMicrophoneMonitor(vi.fn(), onEnded);
    await monitor.start();
    const end = acquired.track.onended!;

    end();
    end();

    expect(onEnded).toHaveBeenCalledOnce();
    expect(acquired.track.stop).toHaveBeenCalledOnce();
    expect(acquired.track.onended).toBeNull();
    expect(contexts[0].source.disconnect).toHaveBeenCalledOnce();
    expect(contexts[0].meter.disconnect).toHaveBeenCalledOnce();
    expect(contexts[0].close).toHaveBeenCalledOnce();
    expect(frames.size).toBe(0);
  });

  it.each(['track-mute', 'context-suspended', 'context-closed', 'context-interrupted'] as const)(
    'stops metering and reports a runtime %s interruption once', async interruption => {
      const acquired = makeStream();
      getUserMedia.mockResolvedValue(acquired.stream);
      const onEnded = vi.fn();
      monitor = createMicrophoneMonitor(vi.fn(), onEnded);
      await monitor.start();
      const mute = acquired.track.onmute!;
      const stateChanged = contexts[0].onstatechange!;

      if (interruption === 'track-mute') {
        acquired.track.muted = true;
        mute();
        mute();
      } else {
        contexts[0].setState(interruption.replace('context-', ''));
        stateChanged();
      }

      expect(onEnded).toHaveBeenCalledOnce();
      expect(acquired.track.stop).toHaveBeenCalledOnce();
      expect(acquired.track.onended).toBeNull();
      expect(acquired.track.onmute).toBeNull();
      expect(contexts[0].onstatechange).toBeNull();
      expect(contexts[0].source.disconnect).toHaveBeenCalledOnce();
      expect(contexts[0].meter.disconnect).toHaveBeenCalledOnce();
      expect(frames.size).toBe(0);
    },
  );

  it('clears hardware listeners before intentional close so hangup is not reported as interruption', async () => {
    const acquired = makeStream();
    getUserMedia.mockResolvedValue(acquired.stream);
    const onEnded = vi.fn();
    monitor = createMicrophoneMonitor(vi.fn(), onEnded);
    await monitor.start();
    expect(contexts[0].onstatechange).toBeTypeOf('function');
    expect(acquired.track.onmute).toBeTypeOf('function');

    monitor.stop();

    expect(contexts[0].close).toHaveBeenCalledOnce();
    expect(contexts[0].state).toBe('closed');
    expect(contexts[0].onstatechange).toBeNull();
    expect(acquired.track.onended).toBeNull();
    expect(acquired.track.onmute).toBeNull();
    expect(onEnded).not.toHaveBeenCalled();
  });
});

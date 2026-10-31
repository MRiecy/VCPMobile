import { afterEach, describe, expect, it, vi } from 'vitest';
import { flushPromises } from '@vue/test-utils';
import { createMicrophoneMonitor, type VoiceAudioFrame } from '../../../features/chat/voice/microphoneMonitor';
import { emitTauriEvent, invokeMock, listenMock } from '../../mocks/tauri';

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((finish, fail) => { resolve = finish; reject = fail; });
  return { promise, resolve, reject };
}

let monitor: ReturnType<typeof createMicrophoneMonitor> | undefined;
const audioEvent = 'vcp-voice-audio';

function emitFrame(sessionId: string, overrides: Partial<VoiceAudioFrame> = {}) {
  emitTauriEvent(audioEvent, {
    kind: 'frame', sessionId, audioEpoch: 1, sampleIndex: 512, probability: 0.8,
    level: 0.7, bands: Array(32).fill(0.5), aecState: 'enabled', route: 'speaker',
    playbackActive: false, speechActive: true, speechTransition: 'start', discontinuity: false,
    ...overrides,
  } satisfies VoiceAudioFrame);
}

afterEach(() => {
  monitor?.detach();
  monitor = undefined;
  vi.useRealTimers();
});

describe('native microphone display observer', () => {
  it('only observes the capture selected by the snapshot and never starts or stops audio', async () => {
    const installed = deferred<() => void>();
    const unlisten = vi.fn();
    listenMock.mockImplementationOnce(() => installed.promise);
    monitor = createMicrophoneMonitor(vi.fn(), vi.fn(), vi.fn());
    monitor.observe('capture');
    expect(listenMock).toHaveBeenCalledWith(audioEvent, expect.any(Function));
    expect(invokeMock).not.toHaveBeenCalled();

    installed.resolve(unlisten);
    await flushPromises();
    monitor.observe(null);
    expect(unlisten).toHaveBeenCalledOnce();
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it('cancels immediately during listener installation and releases the late listener without starting audio', async () => {
    const installed = deferred<() => void>();
    const unlisten = vi.fn();
    listenMock.mockImplementationOnce(() => installed.promise);
    monitor = createMicrophoneMonitor(vi.fn(), vi.fn(), vi.fn());
    monitor.observe('capture');
    monitor.detach();
    expect(invokeMock).not.toHaveBeenCalled();

    installed.resolve(unlisten);
    await flushPromises();
    expect(unlisten).toHaveBeenCalledOnce();
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it('ignores frames from a capture superseded by the snapshot', async () => {
    const onFrame = vi.fn();
    monitor = createMicrophoneMonitor(vi.fn(), vi.fn(), onFrame);
    monitor.observe('old');
    await flushPromises();
    monitor.observe('next');
    await flushPromises();
    emitFrame('old');
    expect(onFrame).not.toHaveBeenCalled();
    emitFrame('next');
    expect(onFrame).toHaveBeenCalledOnce();
    emitFrame('next', { sampleIndex: 1024 });
    expect(onFrame).toHaveBeenCalledTimes(2);
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it('accepts only current, ordered audio frames and a new epoch can restart its sample counter', async () => {
    const onFrame = vi.fn();
    monitor = createMicrophoneMonitor(vi.fn(), vi.fn(), onFrame);
    const id = 'capture';
    monitor.observe(id);
    await flushPromises();
    emitFrame(id);
    emitFrame(id);
    emitFrame(id, { sampleIndex: 256 });
    emitFrame(id, { audioEpoch: 0, sampleIndex: 2048 });
    emitFrame('another-session');
    expect(onFrame).toHaveBeenCalledOnce();

    emitFrame(id, { audioEpoch: 2, sampleIndex: 0, discontinuity: true });
    emitFrame(id, { audioEpoch: 1, sampleIndex: 4096 });
    expect(onFrame).toHaveBeenCalledTimes(2);
    expect(onFrame).toHaveBeenLastCalledWith(expect.objectContaining({ audioEpoch: 2, sampleIndex: 0, discontinuity: true }));
  });

  it('bounds display values and discards malformed frequency frames', async () => {
    const onLevel = vi.fn();
    const onFrame = vi.fn();
    monitor = createMicrophoneMonitor(onLevel, vi.fn(), onFrame);
    monitor.observe('capture');
    await flushPromises();
    emitFrame('capture', { level: 2, probability: -1, bands: Array(32).fill(3) });
    expect(onLevel).toHaveBeenLastCalledWith(1, Array(32).fill(1));
    expect(onFrame).toHaveBeenLastCalledWith(expect.objectContaining({ probability: 0 }));

    emitFrame('capture', { sampleIndex: 1024, bands: [0.5] });
    emitFrame('capture', { sampleIndex: 1536, level: Number.NaN });
    emitFrame('capture', { audioEpoch: -1, sampleIndex: 2048 });
    expect(onFrame).toHaveBeenCalledOnce();
  });

  it.each(['stopped', 'error'] as const)('clears displayed meters once after native %s', async kind => {
    const onEnded = vi.fn();
    const onLevel = vi.fn();
    monitor = createMicrophoneMonitor(onLevel, onEnded, vi.fn());
    monitor.observe('capture');
    await flushPromises();
    const ended = { kind, sessionId: 'capture', audioEpoch: 1, reason: 'background' };
    emitTauriEvent(audioEvent, ended);
    emitTauriEvent(audioEvent, ended);
    expect(onEnded).toHaveBeenCalledExactlyOnceWith(ended);
    expect(onLevel).toHaveBeenLastCalledWith(0, Array(32).fill(0));

    expect(invokeMock).not.toHaveBeenCalled();
  });

  it('ignores terminal events from an old epoch and removes its listener after an intentional stop', async () => {
    const onEnded = vi.fn();
    const onFrame = vi.fn();
    monitor = createMicrophoneMonitor(vi.fn(), onEnded, onFrame);
    const id = 'capture';
    monitor.observe(id);
    await flushPromises();
    emitFrame(id, { audioEpoch: 2 });
    emitTauriEvent(audioEvent, { kind: 'error', sessionId: id, audioEpoch: 1, reason: 'old route' });
    expect(onEnded).not.toHaveBeenCalled();

    monitor.detach();
    monitor.detach();
    emitFrame(id, { audioEpoch: 2, sampleIndex: 1024 });
    emitTauriEvent(audioEvent, { kind: 'stopped', sessionId: id, audioEpoch: 2, reason: 'requested' });
    expect(onEnded).not.toHaveBeenCalled();
    expect(onFrame).toHaveBeenCalledOnce();
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it('does not interpret suspended WebView frames as a native capture failure', async () => {
    vi.useFakeTimers();
    const onEnded = vi.fn();
    const onLevel = vi.fn();
    monitor = createMicrophoneMonitor(onLevel, onEnded, vi.fn());
    monitor.observe('capture');
    await vi.advanceTimersByTimeAsync(10_000);
    expect(onEnded).not.toHaveBeenCalled();

    const id = 'capture';
    await vi.advanceTimersByTimeAsync(2999);
    emitFrame(id, { level: 0, probability: 0, bands: Array(32).fill(0) });
    await vi.advanceTimersByTimeAsync(2999);
    expect(onEnded).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);
    expect(onEnded).not.toHaveBeenCalled();
    expect(invokeMock).not.toHaveBeenCalled();
    monitor.detach();
    await vi.advanceTimersByTimeAsync(6000);
    expect(onEnded).not.toHaveBeenCalled();
  });

});

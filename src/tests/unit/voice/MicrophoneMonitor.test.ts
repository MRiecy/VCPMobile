import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushPromises } from '@vue/test-utils';
import { createMicrophoneMonitor as createNativeMicrophoneMonitor, type VoiceAudioFrame } from '../../../features/chat/voice/microphoneMonitor';
import { emitTauriEvent, invokeMock, listenMock, mockInvoke } from '../../mocks/tauri';

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((finish, fail) => { resolve = finish; reject = fail; });
  return { promise, resolve, reject };
}

const createMicrophoneMonitor = (...callbacks: [
  Parameters<typeof createNativeMicrophoneMonitor>[0],
  Parameters<typeof createNativeMicrophoneMonitor>[1],
  Parameters<typeof createNativeMicrophoneMonitor>[2],
]) => createNativeMicrophoneMonitor(...callbacks, {
  beforeStart: async () => 'call-1', closeCurrent: async () => {},
});

let monitor: ReturnType<typeof createMicrophoneMonitor> | undefined;
const startCommand = 'start_voice_microphone';
const stopCommand = 'close_voice_microphone';
const audioEvent = 'vcp-voice-audio';

beforeEach(() => mockInvoke(startCommand, () => ({ configured: true })));

function currentSession() {
  const calls = invokeMock.mock.calls.filter(([command]) => command === startCommand);
  return calls[calls.length - 1][1]!.sessionId as string;
}

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

describe('native microphone client', () => {
  it('releases a recovered display subscription that finishes installing after disposal', async () => {
    const installed = deferred<() => void>();
    const unlisten = vi.fn();
    listenMock.mockImplementationOnce(() => installed.promise);
    monitor = createMicrophoneMonitor(vi.fn(), vi.fn(), vi.fn());
    monitor.observe('surviving-capture');
    monitor.detach();
    installed.resolve(unlisten);
    await flushPromises();
    expect(unlisten).toHaveBeenCalledOnce();
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it('installs the event listener before starting native audio and uses one session UUID', async () => {
    const installed = deferred<() => void>();
    const unlisten = vi.fn();
    listenMock.mockImplementationOnce(() => installed.promise);
    monitor = createMicrophoneMonitor(vi.fn(), vi.fn(), vi.fn());
    const started = monitor.start();
    expect(listenMock).toHaveBeenCalledWith(audioEvent, expect.any(Function));
    expect(invokeMock).not.toHaveBeenCalled();

    installed.resolve(unlisten);
    await expect(started).resolves.toBe(true);
    expect(invokeMock).toHaveBeenCalledExactlyOnceWith(startCommand, { sessionId: currentSession(), callId: 'call-1' });

    await monitor.stop();
    expect(unlisten).toHaveBeenCalledOnce();
    expect(invokeMock).toHaveBeenLastCalledWith(stopCommand, { callId: 'call-1', sessionId: currentSession() });
  });

  it('reports the native permission failure and releases its installed listener', async () => {
    mockInvoke(startCommand, () => Promise.reject('麦克风权限未开启'));
    const onEnded = vi.fn();
    const onFrame = vi.fn();
    monitor = createMicrophoneMonitor(vi.fn(), onEnded, onFrame);
    await expect(monitor.start()).rejects.toBe('麦克风权限未开启');
    emitFrame(currentSession());
    expect(onFrame).not.toHaveBeenCalled();
    expect(onEnded).not.toHaveBeenCalled();
    expect(invokeMock).toHaveBeenCalledWith(stopCommand, { callId: 'call-1', sessionId: currentSession() });
  });

  it('cancels immediately during listener installation and releases the late listener without starting audio', async () => {
    const installed = deferred<() => void>();
    const unlisten = vi.fn();
    listenMock.mockImplementationOnce(() => installed.promise);
    monitor = createMicrophoneMonitor(vi.fn(), vi.fn(), vi.fn());
    const started = monitor.start();
    monitor.stop();
    await expect(started).resolves.toBe(false);
    expect(invokeMock).not.toHaveBeenCalled();

    installed.resolve(unlisten);
    await flushPromises();
    expect(unlisten).toHaveBeenCalledOnce();
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it.each(['resolve', 'reject'] as const)('cancels pending native permission without blocking a new start after late %s', async outcome => {
    const pending = deferred<void>();
    mockInvoke(startCommand, () => pending.promise);
    const onFrame = vi.fn();
    monitor = createMicrophoneMonitor(vi.fn(), vi.fn(), onFrame);
    const first = monitor.start();
    await flushPromises();
    const oldId = currentSession();
    monitor.stop();
    await expect(first).resolves.toBe(false);
    expect(invokeMock).toHaveBeenCalledWith(stopCommand, { callId: 'call-1', sessionId: oldId });

    mockInvoke(startCommand, () => ({ configured: true }));
    await expect(monitor.start()).resolves.toBe(true);
    const nextId = currentSession();
    expect(nextId).not.toBe(oldId);
    emitFrame(oldId);
    expect(onFrame).not.toHaveBeenCalled();
    emitFrame(nextId);
    expect(onFrame).toHaveBeenCalledOnce();

    if (outcome === 'resolve') pending.resolve();
    else pending.reject('old permission denied');
    await flushPromises();
    emitFrame(nextId, { sampleIndex: 1024 });
    expect(onFrame).toHaveBeenCalledTimes(2);
    expect(invokeMock.mock.calls.filter(([command, args]) => command === stopCommand && args?.sessionId === nextId)).toHaveLength(0);
  });

  it('accepts only current, ordered audio frames and a new epoch can restart its sample counter', async () => {
    const onFrame = vi.fn();
    monitor = createMicrophoneMonitor(vi.fn(), vi.fn(), onFrame);
    await monitor.start();
    const id = currentSession();
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

  it.each([false, true])('waits for stop acknowledgement without waiting for old permission, cancelled next start=%s', async cancelNext => {
    const oldStart = deferred<void>();
    const stopped = deferred<void>();
    mockInvoke(startCommand, () => oldStart.promise);
    mockInvoke(stopCommand, () => stopped.promise);
    monitor = createMicrophoneMonitor(vi.fn(), vi.fn(), vi.fn());
    const first = monitor.start();
    await flushPromises();
    const firstId = currentSession();

    monitor.stop();
    await expect(first).resolves.toBe(false);
    expect(invokeMock).toHaveBeenCalledWith(stopCommand, { callId: 'call-1', sessionId: firstId });
    mockInvoke(startCommand, () => ({ configured: true }));
    const next = monitor.start();
    await flushPromises();
    expect(invokeMock.mock.calls.filter(([command]) => command === startCommand)).toHaveLength(1);

    if (cancelNext) {
      monitor.stop();
      await expect(next).resolves.toBe(false);
    }
    stopped.resolve();
    await expect(next).resolves.toBe(!cancelNext);
    await flushPromises();
    expect(invokeMock.mock.calls.filter(([command]) => command === startCommand)).toHaveLength(cancelNext ? 1 : 2);
    if (!cancelNext) expect(currentSession()).not.toBe(firstId);

    oldStart.resolve();
    await flushPromises();
    expect(invokeMock.mock.calls.filter(([command]) => command === startCommand)).toHaveLength(cancelNext ? 1 : 2);
  });

  it('bounds display values and discards malformed frequency frames', async () => {
    const onLevel = vi.fn();
    const onFrame = vi.fn();
    monitor = createMicrophoneMonitor(onLevel, vi.fn(), onFrame);
    await monitor.start();
    emitFrame(currentSession(), { level: 2, probability: -1, bands: Array(32).fill(3) });
    expect(onLevel).toHaveBeenLastCalledWith(1, Array(32).fill(1));
    expect(onFrame).toHaveBeenLastCalledWith(expect.objectContaining({ probability: 0 }));

    emitFrame(currentSession(), { sampleIndex: 1024, bands: [0.5] });
    emitFrame(currentSession(), { sampleIndex: 1536, level: Number.NaN });
    emitFrame(currentSession(), { audioEpoch: -1, sampleIndex: 2048 });
    expect(onFrame).toHaveBeenCalledOnce();
  });

  it.each(['stopped', 'error'] as const)('handles a native %s before start acknowledgement without restoring the microphone', async kind => {
    const pending = deferred<void>();
    mockInvoke(startCommand, () => pending.promise);
    const onEnded = vi.fn();
    const onLevel = vi.fn();
    monitor = createMicrophoneMonitor(onLevel, onEnded, vi.fn());
    const started = monitor.start();
    await flushPromises();
    const ended = { kind, sessionId: currentSession(), audioEpoch: 1, reason: 'background' };
    emitTauriEvent(audioEvent, ended);
    emitTauriEvent(audioEvent, ended);
    await expect(started).resolves.toBe(false);
    expect(onEnded).toHaveBeenCalledExactlyOnceWith(ended);
    expect(onLevel).toHaveBeenLastCalledWith(0, Array(32).fill(0));

    pending.resolve();
    await flushPromises();
    expect(onEnded).toHaveBeenCalledOnce();
  });

  it('ignores terminal events from an old epoch and removes its listener after an intentional stop', async () => {
    const onEnded = vi.fn();
    const onFrame = vi.fn();
    monitor = createMicrophoneMonitor(vi.fn(), onEnded, onFrame);
    await monitor.start();
    const id = currentSession();
    emitFrame(id, { audioEpoch: 2 });
    emitTauriEvent(audioEvent, { kind: 'error', sessionId: id, audioEpoch: 1, reason: 'old route' });
    expect(onEnded).not.toHaveBeenCalled();

    monitor.stop();
    monitor.stop();
    emitFrame(id, { audioEpoch: 2, sampleIndex: 1024 });
    emitTauriEvent(audioEvent, { kind: 'stopped', sessionId: id, audioEpoch: 2, reason: 'requested' });
    expect(onEnded).not.toHaveBeenCalled();
    expect(onFrame).toHaveBeenCalledOnce();
    expect(invokeMock.mock.calls.filter(([command]) => command === stopCommand)).toHaveLength(1);
  });

  it('does not interpret suspended WebView frames as a native capture failure', async () => {
    vi.useFakeTimers();
    const pending = deferred<{ configured: boolean }>();
    mockInvoke(startCommand, () => pending.promise);
    const onEnded = vi.fn();
    const onLevel = vi.fn();
    monitor = createMicrophoneMonitor(onLevel, onEnded, vi.fn());
    const started = monitor.start();
    await vi.advanceTimersByTimeAsync(10_000);
    expect(onEnded).not.toHaveBeenCalled();

    pending.resolve({ configured: true });
    await expect(started).resolves.toBe(true);
    const id = currentSession();
    await vi.advanceTimersByTimeAsync(2999);
    emitFrame(id, { level: 0, probability: 0, bands: Array(32).fill(0) });
    await vi.advanceTimersByTimeAsync(2999);
    expect(onEnded).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);
    expect(onEnded).not.toHaveBeenCalled();
    expect(invokeMock.mock.calls.some(([name]) => name === stopCommand)).toBe(false);

    mockInvoke(startCommand, () => ({ configured: true }));
    await monitor.start();
    monitor.stop();
    await vi.advanceTimersByTimeAsync(6000);
    expect(onEnded).not.toHaveBeenCalled();
  });

  it('submits one Rust close intent without a JS acknowledgement-dependent seal', async () => {
    const stopped = deferred<{ audioEpoch: number; sampleIndex: number }>();
    mockInvoke(stopCommand, () => stopped.promise);
    monitor = createNativeMicrophoneMonitor(vi.fn(), vi.fn(), vi.fn(), {
      beforeStart: async () => 'call-1', closeCurrent: async () => {},
    });
    await monitor.start();
    const id = currentSession();
    monitor.stop();
    expect(invokeMock).toHaveBeenLastCalledWith(stopCommand, { callId: 'call-1', sessionId: id });
    stopped.resolve({ audioEpoch: 3, sampleIndex: 731 });
    await flushPromises();
    expect(invokeMock.mock.calls.some(([name]) => name === 'close_voice_input')).toBe(false);
  });

  it('projects native background termination without issuing a second seal or stop', async () => {
    const onEnded = vi.fn();
    monitor = createNativeMicrophoneMonitor(vi.fn(), onEnded, vi.fn(), {
      beforeStart: async () => 'call-1', closeCurrent: async () => {},
    });
    await monitor.start();
    const id = currentSession();
    emitTauriEvent(audioEvent, { kind: 'stopped', sessionId: id, audioEpoch: 2, sampleIndex: 999, reason: 'background' });
    expect(onEnded).toHaveBeenCalledWith(expect.objectContaining({ reason: 'background' }));
    expect(invokeMock.mock.calls.some(([name]) => name === 'close_voice_input' || name === stopCommand)).toBe(false);
  });

  it('freezes the native lifecycle epoch at the tap before awaiting call readiness', async () => {
    let epoch = 3;
    const ready = deferred<string>();
    monitor = createNativeMicrophoneMonitor(vi.fn(), vi.fn(), vi.fn(), {
      beforeStart: () => ready.promise, closeCurrent: async () => {}, currentLifecycleEpoch: () => epoch,
    });
    const starting = monitor.start();
    await flushPromises();
    // The native lifecycle advanced while JS was suspended. Rust must receive
    // the old intent epoch and reject it, rather than treating resume as a tap.
    epoch = 5;
    mockInvoke(startCommand, args => args?.expectedLifecycleEpoch === epoch
      ? { configured: true } : Promise.reject('voice_requires_foreground'));
    ready.resolve('call-1');
    await expect(starting).rejects.toBe('voice_requires_foreground');
    expect(invokeMock).toHaveBeenCalledWith(startCommand, expect.objectContaining({ expectedLifecycleEpoch: 3 }));
  });

  it('cancels an input intent awaiting call readiness without starting or closing native capture', async () => {
    const ready = deferred<string>();
    monitor = createNativeMicrophoneMonitor(vi.fn(), vi.fn(), vi.fn(), {
      beforeStart: () => ready.promise, closeCurrent: async () => {},
    });
    const starting = monitor.start();
    await flushPromises();
    monitor.stop();
    ready.resolve('call-1');
    await expect(starting).resolves.toBe(false);
    await flushPromises();
    expect(invokeMock).not.toHaveBeenCalled();
  });
});

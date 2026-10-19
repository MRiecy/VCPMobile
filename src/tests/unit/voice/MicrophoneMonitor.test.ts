import { afterEach, describe, expect, it, vi } from 'vitest';
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
]) => createNativeMicrophoneMonitor(...callbacks, { beforeStart: async () => 'call-1', onStop: vi.fn() });

let monitor: ReturnType<typeof createMicrophoneMonitor> | undefined;
const startCommand = 'plugin:vcp-mobile|start_voice_audio';
const stopCommand = 'plugin:vcp-mobile|stop_voice_audio';
const probeCommand = 'plugin:vcp-mobile|set_voice_probe';
const audioEvent = 'vcp-voice-audio';

function currentSession() {
  const calls = invokeMock.mock.calls.filter(([command]) => command === startCommand);
  return calls[calls.length - 1][1]!.sessionId as string;
}

function emitFrame(sessionId: string, overrides: Partial<VoiceAudioFrame> = {}) {
  emitTauriEvent(audioEvent, {
    kind: 'frame', sessionId, audioEpoch: 1, sampleIndex: 512, probability: 0.8,
    level: 0.7, bands: Array(32).fill(0.5), aecState: 'enabled', route: 'speaker',
    playbackActive: false, probeRevision: 0, speechActive: true, speechTransition: 'start', discontinuity: false,
    ...overrides,
  } satisfies VoiceAudioFrame);
}

afterEach(() => {
  monitor?.stop();
  monitor = undefined;
  vi.useRealTimers();
});

describe('native microphone client', () => {
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
    expect(currentSession()).toMatch(/^[0-9a-f-]{36}$/);
    expect(invokeMock).toHaveBeenCalledExactlyOnceWith(startCommand, { sessionId: currentSession(), callId: 'call-1' });

    monitor.stop();
    expect(unlisten).toHaveBeenCalledOnce();
    expect(invokeMock).toHaveBeenLastCalledWith(stopCommand, { sessionId: currentSession() });
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
    expect(invokeMock).toHaveBeenCalledWith(stopCommand, { sessionId: currentSession() });
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
    expect(invokeMock).toHaveBeenCalledWith(stopCommand, { sessionId: oldId });

    mockInvoke(startCommand, () => undefined);
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
    expect(invokeMock).toHaveBeenCalledWith(stopCommand, { sessionId: firstId });
    mockInvoke(startCommand, () => undefined);
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
    emitFrame(currentSession(), { probeRevision: -1, sampleIndex: 2560 });
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

  it('runs the local probe only for running capture and does not create a new intent after a late start acknowledgement', async () => {
    monitor = createMicrophoneMonitor(vi.fn(), vi.fn(), vi.fn());
    await expect(monitor.setProbe(true)).resolves.toBe(false);
    expect(invokeMock).not.toHaveBeenCalled();
    await monitor.start();
    const pending = deferred<void>();
    mockInvoke(probeCommand, args => args?.enabled ? pending.promise : undefined);
    const enabled = monitor.setProbe(true);
    await monitor.setProbe(false);
    expect(invokeMock).toHaveBeenLastCalledWith(probeCommand, { sessionId: currentSession(), enabled: false });

    pending.resolve();
    await expect(enabled).resolves.toBe(true);
    expect(invokeMock.mock.calls.filter(([command]) => command === probeCommand)).toEqual([
      [probeCommand, { sessionId: currentSession(), enabled: true }],
      [probeCommand, { sessionId: currentSession(), enabled: false }],
    ]);
  });

  it('settles a pending probe when microphone capture stops and cannot affect a newer session', async () => {
    monitor = createMicrophoneMonitor(vi.fn(), vi.fn(), vi.fn());
    await monitor.start();
    const oldId = currentSession();
    const pending = deferred<void>();
    mockInvoke(probeCommand, () => pending.promise);
    const enabled = monitor.setProbe(true);
    monitor.stop();
    await expect(enabled).resolves.toBe(false);
    await monitor.start();
    pending.resolve();
    await flushPromises();
    expect(invokeMock.mock.calls.filter(([command, args]) => command === probeCommand && args?.sessionId !== oldId)).toHaveLength(0);
  });

  it('times out missing frames only after start acknowledgement, accepts silence and clears timers on stop', async () => {
    vi.useFakeTimers();
    const pending = deferred<void>();
    mockInvoke(startCommand, () => pending.promise);
    const onEnded = vi.fn();
    const onLevel = vi.fn();
    monitor = createMicrophoneMonitor(onLevel, onEnded, vi.fn());
    const started = monitor.start();
    await vi.advanceTimersByTimeAsync(10_000);
    expect(onEnded).not.toHaveBeenCalled();
    expect(vi.getTimerCount()).toBe(0);

    pending.resolve();
    await expect(started).resolves.toBe(true);
    const id = currentSession();
    await vi.advanceTimersByTimeAsync(2999);
    emitFrame(id, { level: 0, probability: 0, bands: Array(32).fill(0) });
    await vi.advanceTimersByTimeAsync(2999);
    expect(onEnded).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);
    expect(onEnded).toHaveBeenCalledExactlyOnceWith({
      kind: 'error', sessionId: id, audioEpoch: 1, reason: 'voice_events_timeout',
    });
    expect(invokeMock).toHaveBeenCalledWith(stopCommand, { sessionId: id });
    expect(onLevel).toHaveBeenLastCalledWith(0, Array(32).fill(0));
    expect(vi.getTimerCount()).toBe(0);

    mockInvoke(startCommand, () => undefined);
    await monitor.start();
    expect(vi.getTimerCount()).toBe(1);
    monitor.stop();
    expect(vi.getTimerCount()).toBe(0);
    await vi.advanceTimersByTimeAsync(6000);
    expect(onEnded).toHaveBeenCalledOnce();
  });

  it('forwards the final native PCM position only after stop acknowledges sealing the input', async () => {
    const stopped = deferred<{ audioEpoch: number; sampleIndex: number }>();
    mockInvoke(stopCommand, () => stopped.promise);
    const onStop = vi.fn();
    monitor = createNativeMicrophoneMonitor(vi.fn(), vi.fn(), vi.fn(), {
      beforeStart: async () => 'call-1', onStop,
    });
    await monitor.start();
    const id = currentSession();
    monitor.stop();
    expect(onStop).not.toHaveBeenCalled();
    stopped.resolve({ audioEpoch: 3, sampleIndex: 731 });
    await flushPromises();
    expect(onStop).toHaveBeenCalledExactlyOnceWith(id, { audioEpoch: 3, sampleIndex: 731 });
  });

  it('passes a native background terminal position through to input draining', async () => {
    const onStop = vi.fn();
    monitor = createNativeMicrophoneMonitor(vi.fn(), vi.fn(), vi.fn(), {
      beforeStart: async () => 'call-1', onStop,
    });
    await monitor.start();
    const id = currentSession();
    emitTauriEvent(audioEvent, { kind: 'stopped', sessionId: id, audioEpoch: 2, sampleIndex: 999, reason: 'background' });
    expect(onStop).toHaveBeenCalledExactlyOnceWith(id, { audioEpoch: 2, sampleIndex: 999 });
  });
});

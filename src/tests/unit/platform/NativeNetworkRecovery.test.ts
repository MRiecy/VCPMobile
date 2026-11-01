import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { defineComponent, h, reactive } from 'vue';
import { flushPromises, mount, type VueWrapper } from '@vue/test-utils';
import { useAppLifecycle } from '../../../core/composables/useAppLifecycle';
import type { LifecycleSnapshotDto } from '../../../core/types/lifecycle';
import { emitTauriEvent, listenMock } from '../../mocks/tauri';

const recover = vi.fn<() => Promise<void>>();
const voiceRecover = vi.fn<() => Promise<void>>();
const voiceDispose = vi.fn();
const refreshInfo = vi.fn(async () => {});
let lifecycle: {
  state: string;
  lifecycleSnapshot: LifecycleSnapshotDto | null;
  hydrateSystemStatus: () => Promise<void>;
  applyLifecycleSnapshot: (snapshot: LifecycleSnapshotDto) => void;
};

vi.mock('../../../core/stores/appLifecycle', () => ({
  useAppLifecycleStore: () => lifecycle,
}));
vi.mock('../../../core/stores/chatStreamStore', () => ({
  useChatStreamStore: () => ({ checkAndRecoverInterruptedStreams: recover }),
}));
vi.mock('../../../core/stores/ragObserver', () => ({
  useRagObserverStore: () => ({ refresh: refreshInfo }),
}));
vi.mock('../../../features/chat/voice/voiceHistorySync', () => ({
  createVoiceHistorySync: () => ({ reconcile: voiceRecover, dispose: voiceDispose }),
}));

describe('native network recovery', () => {
  let wrapper: VueWrapper | undefined;
  const consumer = defineComponent({
    setup() {
      useAppLifecycle();
      return () => h('div');
    },
  });

  beforeEach(() => {
    recover.mockReset().mockResolvedValue(undefined);
    voiceRecover.mockReset().mockResolvedValue(undefined);
    voiceDispose.mockReset();
    refreshInfo.mockClear();
    vi.spyOn(document, 'hidden', 'get').mockReturnValue(false);
    lifecycle = reactive({
      state: 'READY',
      lifecycleSnapshot: {
        phase: 'active', nativeSequence: 1, snapshot: true, transitionEpoch: 1,
      } as LifecycleSnapshotDto,
      hydrateSystemStatus: vi.fn(async () => {}),
      applyLifecycleSnapshot: (snapshot: LifecycleSnapshotDto) => {
        lifecycle.lifecycleSnapshot = snapshot;
      },
    });
  });

  afterEach(() => {
    wrapper?.unmount();
    wrapper = undefined;
  });

  it('recovers from native network changes and ignores browser online events', async () => {
    let finish: () => void = () => {};
    recover.mockImplementationOnce(() => new Promise<void>((resolve) => { finish = resolve; }));
    wrapper = mount(consumer);
    await flushPromises();

    window.dispatchEvent(new Event('online'));
    await flushPromises();
    emitTauriEvent('vcp-network-changed', { connected: false });
    expect(recover).not.toHaveBeenCalled();
    emitTauriEvent('vcp-network-changed', { connected: true });
    window.dispatchEvent(new Event('online'));
    emitTauriEvent('vcp-network-changed', { connected: true });
    expect(recover).toHaveBeenCalledTimes(1);
    expect(voiceRecover).toHaveBeenCalledTimes(1);

    finish();
    await flushPromises();
    window.dispatchEvent(new Event('online'));
    await flushPromises();
    expect(recover).toHaveBeenCalledTimes(1);
    emitTauriEvent('vcp-network-changed', { connected: true });
    expect(recover).toHaveBeenCalledTimes(2);
  });

  it('reads socket status once when an entire native background interval crosses a frozen WebView', async () => {
    wrapper = mount(consumer);
    await flushPromises();
    vi.mocked(lifecycle.hydrateSystemStatus).mockClear();
    const transition = (epoch: number, phase: 'background' | 'active') => ({
      transitionEpoch: epoch, nativeSequence: epoch, phase, snapshot: false,
    });
    emitTauriEvent('vcp-lifecycle-changed', transition(2, 'background'));
    emitTauriEvent('vcp-lifecycle-changed', transition(3, 'active'));
    await flushPromises();
    emitTauriEvent('vcp-lifecycle-changed', transition(3, 'active'));
    await flushPromises();
    expect(lifecycle.hydrateSystemStatus).toHaveBeenCalledTimes(1);
    expect(refreshInfo).toHaveBeenCalledTimes(1);
  });

  it('refreshes Log and Info when the document becomes visible before a native active event is delivered', async () => {
    wrapper = mount(consumer);
    await flushPromises();
    vi.mocked(lifecycle.hydrateSystemStatus).mockClear();
    emitTauriEvent('vcp-lifecycle-changed', { phase: 'background', transitionEpoch: 2, nativeSequence: 2, snapshot: false });
    vi.spyOn(document, 'hidden', 'get').mockReturnValue(true);
    document.dispatchEvent(new Event('visibilitychange'));
    await flushPromises();
    expect(lifecycle.hydrateSystemStatus).not.toHaveBeenCalled();
    vi.spyOn(document, 'hidden', 'get').mockReturnValue(false);
    document.dispatchEvent(new Event('visibilitychange'));
    await flushPromises();
    expect(lifecycle.hydrateSystemStatus).toHaveBeenCalledTimes(1);
    expect(refreshInfo).toHaveBeenCalledTimes(1);
  });

  it('coalesces the visible notification and native resume even if JS missed the hidden notification', async () => {
    wrapper = mount(consumer);
    await flushPromises();
    vi.mocked(lifecycle.hydrateSystemStatus).mockClear();
    document.dispatchEvent(new Event('visibilitychange'));
    emitTauriEvent('vcp-lifecycle-changed', { phase: 'active', transitionEpoch: 3, nativeSequence: 3, snapshot: false });
    await flushPromises();
    expect(lifecycle.hydrateSystemStatus).toHaveBeenCalledTimes(1);
    expect(refreshInfo).toHaveBeenCalledTimes(1);
  });

  it('waits for READY and a native active snapshot while the document is visible', async () => {
    wrapper = mount(consumer);
    await flushPromises();
    lifecycle.state = 'LOADING';
    emitTauriEvent('vcp-network-changed', { connected: true });
    expect(recover).not.toHaveBeenCalled();
    const snapshot = lifecycle.lifecycleSnapshot!;
    lifecycle.lifecycleSnapshot = null;
    lifecycle.state = 'READY';
    await flushPromises();
    emitTauriEvent('vcp-network-changed', { connected: true });
    expect(recover).not.toHaveBeenCalled();
    lifecycle.lifecycleSnapshot = { ...snapshot, phase: 'inactive' };
    await flushPromises();
    emitTauriEvent('vcp-network-changed', { connected: true });
    lifecycle.lifecycleSnapshot!.phase = 'background';
    await flushPromises();
    emitTauriEvent('vcp-network-changed', { connected: true });
    expect(recover).not.toHaveBeenCalled();

    vi.spyOn(document, 'hidden', 'get').mockReturnValue(true);
    document.dispatchEvent(new Event('visibilitychange'));
    lifecycle.lifecycleSnapshot!.phase = 'active';
    await flushPromises();
    emitTauriEvent('vcp-network-changed', { connected: true });
    expect(recover).not.toHaveBeenCalled();
  });

  it('releases a late listener and ignores its callbacks after unmount', async () => {
    const stop = vi.fn();
    let finishRegistration: (stopListening: () => void) => void = () => {};
    let callback: ((event: { payload: unknown }) => void) | undefined;
    listenMock.mockImplementationOnce(async (_name, handler) => {
      callback = handler;
      return new Promise<() => void>((resolve) => { finishRegistration = resolve; });
    });
    wrapper = mount(consumer);
    await flushPromises();
    wrapper.unmount();
    wrapper = undefined;
    callback?.({ payload: { connected: true } });
    finishRegistration(stop);
    await flushPromises();

    expect(stop).toHaveBeenCalledOnce();
    expect(recover).not.toHaveBeenCalled();
  });
});

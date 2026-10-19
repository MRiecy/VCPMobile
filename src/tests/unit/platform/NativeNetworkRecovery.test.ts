import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { defineComponent, h, reactive } from 'vue';
import { flushPromises, mount, type VueWrapper } from '@vue/test-utils';
import { useAppLifecycle } from '../../../core/composables/useAppLifecycle';
import type { LifecycleSnapshotDto } from '../../../core/types/lifecycle';
import { emitTauriEvent, listenMock } from '../../mocks/tauri';

const recover = vi.fn<() => Promise<void>>();
const voiceRecover = vi.fn<() => Promise<void>>();
const voiceDispose = vi.fn();
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
vi.mock('../../../features/chat/voice/voiceHistorySync', () => ({
  createVoiceHistorySync: () => ({ reconcile: voiceRecover, dispose: voiceDispose }),
}));
vi.mock('../../../core/utils/platform', () => ({
  nativePlatform: 'ios',
  isAndroid: false,
  isIOS: true,
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

  it('coalesces native and browser online in the existing recovery owner', async () => {
    let finish: () => void = () => {};
    recover.mockImplementationOnce(() => new Promise<void>((resolve) => { finish = resolve; }));
    wrapper = mount(consumer);
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
    emitTauriEvent('vcp-network-changed', { connected: true });
    expect(recover).toHaveBeenCalledTimes(2);
  });

  it('waits for READY and a real active iOS snapshot while the document is visible', async () => {
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

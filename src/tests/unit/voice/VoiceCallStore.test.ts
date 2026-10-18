import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, disposePinia, setActivePinia, type Pinia } from 'pinia';
import { reactive } from 'vue';
import { flushPromises } from '@vue/test-utils';
import { useVoiceCallStore } from '../../../features/chat/voice/voiceCallStore';
import type { ConversationKey } from '../../../core/stores/chatSessionStore';
import { invokeMock } from '../../mocks/tauri';

const context = vi.hoisted(() => ({
  session: {} as {
    currentConversationKey: ConversationKey | null;
    isConversationCurrent: (key: ConversationKey | null) => boolean;
  },
  lifecycle: {} as { isBackground: boolean },
  start: vi.fn<() => Promise<boolean>>(),
  stop: vi.fn(),
  onLevel: undefined as ((level: number, bands: readonly number[]) => void) | undefined,
  onEnded: undefined as (() => void) | undefined,
}));

vi.mock('../../../core/stores/chatSessionStore', () => ({ useChatSessionStore: () => context.session }));
vi.mock('../../../core/stores/appLifecycle', () => ({ useAppLifecycleStore: () => context.lifecycle }));
vi.mock('../../../features/chat/voice/microphoneMonitor', () => ({
  createMicrophoneMonitor: (onLevel: (level: number, bands: readonly number[]) => void, onEnded: () => void) => {
    context.onLevel = onLevel;
    context.onEnded = onEnded;
    return { start: context.start, stop: context.stop };
  },
}));

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((finish, fail) => { resolve = finish; reject = fail; });
  return { promise, resolve, reject };
}

let pinia: Pinia;
beforeEach(() => {
  pinia = createPinia();
  setActivePinia(pinia);
  context.session = reactive({
    currentConversationKey: { ownerId: 'agent', ownerType: 'agent', topicId: 'topic', epoch: 1 },
    isConversationCurrent: (key: ConversationKey | null) => {
      const current = context.session.currentConversationKey;
      return !!key && !!current && key.ownerId === current.ownerId && key.ownerType === current.ownerType
        && key.topicId === current.topicId && key.epoch === current.epoch;
    },
  });
  context.lifecycle = reactive({ isBackground: false });
  context.start.mockReset().mockResolvedValue(true);
  context.stop.mockReset().mockImplementation(() => context.onLevel?.(0, []));
  context.onLevel = undefined;
  context.onEnded = undefined;
});

afterEach(() => disposePinia(pinia));

describe('voice call lifecycle', () => {
  it('requires a current conversation and does not start twice for one call', async () => {
    context.session.currentConversationKey = null;
    const store = useVoiceCallStore();
    store.start();
    expect(store.active).toBe(false);
    expect(context.start).not.toHaveBeenCalled();

    context.session.currentConversationKey = { ownerId: 'agent', ownerType: 'agent', topicId: 'topic', epoch: 1 };
    store.start();
    store.start();
    await flushPromises();
    expect(store.active).toBe(true);
    expect(store.microphoneState).toBe('on');
    expect(context.start).toHaveBeenCalledOnce();
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it('keeps the call scene with a recoverable error after permission rejection', async () => {
    context.start.mockRejectedValueOnce(new DOMException('Denied', 'NotAllowedError'));
    const store = useVoiceCallStore();
    store.start();
    await flushPromises();

    expect(store.active).toBe(true);
    expect(store.microphoneState).toBe('error');
    expect(store.errorMessage).toContain('麦克风权限未开启');
    expect(invokeMock).not.toHaveBeenCalled();

    store.toggleMicrophone();
    await flushPromises();
    expect(store.microphoneState).toBe('on');
    expect(store.errorMessage).toBe('');
  });

  it.each(['mute', 'hangup'] as const)('ignores a microphone request that resolves after %s', async action => {
    const request = deferred<boolean>();
    context.start.mockReturnValueOnce(request.promise);
    const store = useVoiceCallStore();
    store.start();
    expect(store.microphoneState).toBe('requesting');

    if (action === 'mute') store.toggleMicrophone();
    else store.end();
    request.resolve(true);
    await flushPromises();

    expect(context.stop).toHaveBeenCalledOnce();
    expect(store.active).toBe(action === 'mute');
    expect(store.microphoneState).toBe('off');
    expect(store.level).toBe(0);
  });

  it.each(['resolve', 'reject'] as const)('does not let an older call %s overwrite a new call', async outcome => {
    const oldRequest = deferred<boolean>();
    const newRequest = deferred<boolean>();
    context.start.mockReturnValueOnce(oldRequest.promise).mockReturnValueOnce(newRequest.promise);
    const store = useVoiceCallStore();
    store.start();
    store.end();
    store.start();

    if (outcome === 'resolve') oldRequest.resolve(true);
    else oldRequest.reject(new DOMException('Old denial', 'NotAllowedError'));
    await flushPromises();
    expect(store.active).toBe(true);
    expect(store.microphoneState).toBe('requesting');
    expect(store.errorMessage).toBe('');

    newRequest.resolve(true);
    await flushPromises();
    expect(store.microphoneState).toBe('on');
  });

  it('releases the microphone on background and requires an explicit tap to resume', async () => {
    const store = useVoiceCallStore();
    store.start();
    await flushPromises();
    context.onLevel!(0.8, [0.2, 0.8, 0.4]);
    expect(store.level).toBe(0.8);
    expect(store.bands).toEqual([0.2, 0.8, 0.4]);

    context.lifecycle.isBackground = true;
    expect(context.stop).toHaveBeenCalledOnce();
    expect(store.active).toBe(true);
    expect(store.microphoneState).toBe('off');
    expect(store.level).toBe(0);
    expect(store.bands).toEqual([]);
    store.toggleMicrophone();
    expect(context.start).toHaveBeenCalledOnce();

    context.lifecycle.isBackground = false;
    await flushPromises();
    expect(context.start).toHaveBeenCalledOnce();
    store.toggleMicrophone();
    await flushPromises();
    expect(context.start).toHaveBeenCalledTimes(2);
    expect(store.microphoneState).toBe('on');
  });

  it('ends the call and releases audio when the bound conversation is replaced', async () => {
    const store = useVoiceCallStore();
    store.start();
    await flushPromises();

    context.session.currentConversationKey = { ...context.session.currentConversationKey!, epoch: 2 };

    expect(context.stop).toHaveBeenCalledOnce();
    expect(store.active).toBe(false);
    expect(store.microphoneState).toBe('off');
  });

  it('reports a hardware interruption without leaving the call and cleans up on disposal', async () => {
    const store = useVoiceCallStore();
    store.start();
    await flushPromises();
    context.onEnded!();
    expect(store.active).toBe(true);
    expect(store.microphoneState).toBe('error');
    expect(store.errorMessage).toContain('系统中断');

    store.$dispose();
    expect(context.stop).toHaveBeenCalledOnce();
    expect(store.active).toBe(false);
  });
});

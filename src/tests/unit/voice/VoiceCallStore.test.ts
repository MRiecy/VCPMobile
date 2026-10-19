import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, disposePinia, setActivePinia, type Pinia } from 'pinia';
import { reactive } from 'vue';
import { flushPromises } from '@vue/test-utils';
import { useVoiceCallStore } from '../../../features/chat/voice/voiceCallStore';
import type { ConversationKey } from '../../../core/stores/chatSessionStore';
import type { VoiceAudioEnd, VoiceAudioFrame } from '../../../features/chat/voice/microphoneMonitor';
import type { VoiceCallSnapshot } from '../../../features/chat/voice/voiceConversation';
import { invokeMock } from '../../mocks/tauri';

const context = vi.hoisted(() => ({
  session: {} as {
    currentConversationKey: ConversationKey | null;
    isConversationCurrent: (key: ConversationKey | null) => boolean;
  },
  lifecycle: {} as { isBackground: boolean },
  start: vi.fn<() => Promise<boolean>>(),
  stop: vi.fn(),
  detach: vi.fn(),
  endCall: vi.fn(),
  disposeConversation: vi.fn(),
  recover: vi.fn(),
  onSnapshot: undefined as ((snapshot: VoiceCallSnapshot | null) => void) | undefined,
  setProbe: vi.fn<(enabled: boolean) => Promise<boolean>>(),
  onLevel: undefined as ((level: number, bands: readonly number[]) => void) | undefined,
  onEnded: undefined as ((event: VoiceAudioEnd) => void) | undefined,
  onFrame: undefined as ((frame: VoiceAudioFrame) => void) | undefined,
}));

vi.mock('../../../core/stores/chatSessionStore', () => ({ useChatSessionStore: () => context.session }));
vi.mock('../../../core/stores/appLifecycle', () => ({ useAppLifecycleStore: () => context.lifecycle }));
vi.mock('../../../features/chat/voice/voiceConversation', () => ({
  createVoiceConversation: (options: { onSnapshot: (snapshot: VoiceCallSnapshot | null) => void }) => {
    context.onSnapshot = options.onSnapshot;
    return { begin: vi.fn(), start: vi.fn().mockResolvedValue('call'), stop: context.endCall,
      recover: context.recover, dispose: context.disposeConversation };
  },
}));
vi.mock('../../../features/chat/voice/microphoneMonitor', () => ({
  createMicrophoneMonitor: (onLevel: (level: number, bands: readonly number[]) => void,
    onEnded: (event: VoiceAudioEnd) => void, onFrame: (frame: VoiceAudioFrame) => void) => {
    context.onLevel = onLevel;
    context.onEnded = onEnded;
    context.onFrame = onFrame;
    return { start: context.start, stop: context.stop, setProbe: context.setProbe, detach: context.detach };
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
  context.detach.mockReset();
  context.endCall.mockReset();
  context.disposeConversation.mockReset();
  context.recover.mockReset().mockResolvedValue(null);
  context.setProbe.mockReset().mockResolvedValue(true);
  context.onLevel = undefined;
  context.onEnded = undefined;
  context.onFrame = undefined;
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
    context.start.mockRejectedValueOnce('voice_audio: startVoiceAudio: microphone_permission_denied');
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

    expect(action === 'mute' ? context.stop : context.detach).toHaveBeenCalledOnce();
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

    expect(context.detach).toHaveBeenCalledOnce();
    expect(context.stop).not.toHaveBeenCalled();
    expect(context.endCall).toHaveBeenCalledOnce();
    expect(store.active).toBe(false);
    expect(store.microphoneState).toBe('off');
  });

  it('reports a hardware interruption without leaving the call and cleans up on disposal', async () => {
    const store = useVoiceCallStore();
    store.start();
    await flushPromises();
    context.onEnded!({ kind: 'stopped', sessionId: 'capture', audioEpoch: 1, reason: 'interrupted' });
    expect(store.active).toBe(true);
    expect(store.microphoneState).toBe('error');
    expect(store.errorMessage).toContain('系统中断');

    store.$dispose();
    expect(context.stop).not.toHaveBeenCalled();
    expect(context.detach).toHaveBeenCalledOnce();
    expect(context.disposeConversation).toHaveBeenCalledOnce();
    expect(context.endCall).not.toHaveBeenCalled();
  });

  it('presents speech, route and playback from native frames and clears them when the mic closes', async () => {
    const store = useVoiceCallStore();
    store.start();
    await flushPromises();
    context.onFrame!({
      kind: 'frame', sessionId: 'capture', audioEpoch: 1, sampleIndex: 512,
      probability: 0.9, level: 0.8, bands: Array(32).fill(0.5), aecState: 'enabled',
      route: 'speaker', playbackActive: true, probeRevision: 1, speechActive: true, speechTransition: 'start', discontinuity: false,
    });
    expect(store.speechActive).toBe(true);
    expect(store.aecState).toBe('enabled');
    expect(store.route).toBe('speaker');
    expect(store.playbackActive).toBe(true);

    store.toggleMicrophone();
    expect(store.speechActive).toBe(false);
    expect(store.aecState).toBeNull();
    expect(store.route).toBe('');
    expect(store.playbackActive).toBe(false);
  });

  it('keeps the scene and switches off when native background stop arrives before the browser lifecycle event', async () => {
    const store = useVoiceCallStore();
    store.start();
    await flushPromises();
    context.onEnded!({ kind: 'stopped', sessionId: 'capture', audioEpoch: 1, reason: 'background' });
    expect(store.active).toBe(true);
    expect(store.microphoneState).toBe('off');
    expect(store.errorMessage).toBe('');
  });

  it('maps stable native errors without leaking diagnostic stacks into the UI', async () => {
    const store = useVoiceCallStore();
    store.start();
    await flushPromises();
    context.onEnded!({ kind: 'error', sessionId: 'capture', audioEpoch: 1, reason: 'voice_model_unavailable: ORT_INTERNAL_DETAILS' });
    expect(store.microphoneState).toBe('error');
    expect(store.errorMessage).toContain('本地人声检测模型暂不可用');
    expect(store.errorMessage).not.toContain('ORT_INTERNAL_DETAILS');
  });

  it('starts a local probe only during active capture and does not infer playback from the request', async () => {
    const store = useVoiceCallStore();
    await store.setProbe(true);
    expect(context.setProbe).not.toHaveBeenCalled();
    store.start();
    await flushPromises();
    await store.setProbe(true);
    expect(context.setProbe).toHaveBeenCalledExactlyOnceWith(true);
    expect(store.playbackActive).toBe(false);
    store.toggleMicrophone();
    await store.setProbe(true);
    expect(context.setProbe).toHaveBeenCalledOnce();
  });

  it('can stop a pending probe and discards its late error after backgrounding', async () => {
    const pending = deferred<boolean>();
    context.setProbe.mockReturnValueOnce(pending.promise);
    const store = useVoiceCallStore();
    store.start();
    await flushPromises();
    const probe = store.setProbe(true);
    expect(store.probePending).toBe(true);
    await store.setProbe(false);
    expect(context.setProbe).toHaveBeenLastCalledWith(false);
    context.lifecycle.isBackground = true;
    pending.reject('old playback error');
    await probe;
    expect(store.probePending).toBe(false);
    expect(store.probeError).toBe('');
    expect(store.playbackActive).toBe(false);
  });

  it('treats native VAD cancellation of a pending probe as an interruption rather than an error', async () => {
    context.setProbe.mockRejectedValueOnce('cancelled: playback probe intent was superseded');
    const store = useVoiceCallStore();
    store.start();
    await flushPromises();
    await store.setProbe(true);
    expect(store.probeError).toBe('');
    expect(store.microphoneState).toBe('on');
  });

  it('keeps unknown native errors bounded in diagnostics and generic in the interface', async () => {
    const warning = vi.spyOn(console, 'warn').mockImplementation(() => {});
    context.start.mockRejectedValueOnce(`NSError:${'long-detail'.repeat(100)}`);
    const store = useVoiceCallStore();
    store.start();
    await flushPromises();
    expect(store.errorMessage).toBe('麦克风启动失败，请重试。');
    expect(warning.mock.calls[0][1]).toHaveLength(512);
    warning.mockRestore();
  });

  it('restores closed input and ongoing playback as independent state without reacquiring the microphone', async () => {
    const store = useVoiceCallStore();
    const snapshot: VoiceCallSnapshot = {
      callId: 'restored', target: { ownerId: 'agent', ownerType: 'agent', topicId: 'topic' },
      revision: 8, active: true, micState: 'closed', inputState: 'transcribing',
      replyState: 'finished', ttsState: 'speaking', partial: '已录内容', error: null, notice: null,
      inputSessionId: null, originUserMessageId: 'msg_voice_user', ttsId: 'tts', lastPersistedMessageId: null,
    };
    context.recover.mockImplementation(async () => { context.onSnapshot!(snapshot); return snapshot; });
    await store.recover();
    expect(store.active).toBe(true);
    expect(store.microphoneState).toBe('off');
    expect(store.inputState).toBe('transcribing');
    expect(store.replyState).toBe('finished');
    expect(store.ttsState).toBe('speaking');
    expect(store.playbackActive).toBe(true);
    expect(context.start).not.toHaveBeenCalled();
    context.lifecycle.isBackground = true;
    expect(store.playbackActive).toBe(true);
    expect(context.endCall).not.toHaveBeenCalled();
  });
});

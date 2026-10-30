import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, disposePinia, setActivePinia, type Pinia } from 'pinia';
import { reactive } from 'vue';
import { flushPromises, mount } from '@vue/test-utils';
import { useVoiceCallStore } from '../../../features/chat/voice/voiceCallStore';
import VoiceCallControls from '../../../features/chat/voice/VoiceCallControls.vue';
import type { VoiceCallSnapshot } from '../../../features/chat/voice/voiceConversation';
import type { VoiceAudioFrame } from '../../../features/chat/voice/microphoneMonitor';
import type { ConversationKey } from '../../../core/stores/chatSessionStore';
import { emitTauriEvent, invokeMock, mockInvoke } from '../../mocks/tauri';

const context = vi.hoisted(() => ({
  session: {} as { currentConversationKey: ConversationKey; isConversationCurrent: (key: ConversationKey) => boolean },
  lifecycle: {} as { isBackground: boolean },
}));
vi.mock('../../../core/stores/chatSessionStore', () => ({ useChatSessionStore: () => context.session }));
vi.mock('../../../core/stores/appLifecycle', () => ({ useAppLifecycleStore: () => context.lifecycle }));

function deferred() {
  let resolve!: () => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<void>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

const target = { ownerId: 'agent', ownerType: 'agent' as const, topicId: 'topic' };
const recovered: VoiceCallSnapshot = {
  callId: 'surviving-call', target, revision: 7, active: true,
  micState: 'listening', micStopReason: null, inputState: 'recording', replyState: 'idle',
  ttsState: 'idle', partial: '', error: null, notice: null, inputSessionId: 'surviving-input',
  lastPersistedMessageId: null,
};
let pinia: Pinia;
let revision: number;
function publishSnapshot(update: Partial<VoiceCallSnapshot>) {
  emitTauriEvent('vcp-voice-call', { ...recovered, ...update, revision: ++revision });
}
beforeEach(() => {
  pinia = createPinia();
  setActivePinia(pinia);
  context.session = reactive({
    currentConversationKey: { ...target, epoch: 1 },
    isConversationCurrent: (key: ConversationKey) => key.epoch === context.session.currentConversationKey.epoch,
  });
  context.lifecycle = reactive({ isBackground: false });
  revision = recovered.revision;
  mockInvoke('allocate_voice_document', () => 'document-1');
  mockInvoke('get_voice_conversation_snapshot', () => recovered);
  mockInvoke('start_voice_microphone', () => ({ configured: true }));
  mockInvoke('close_voice_microphone', () => publishSnapshot({ micState: 'closed', micStopReason: 'requested', inputSessionId: null }));
});
afterEach(() => disposePinia(pinia));

describe('microphone control after WebView recovery', () => {
  it('restores only the display listener and rejects old capture frames after the snapshot changes', async () => {
    const store = useVoiceCallStore();
    await store.recover();
    await flushPromises();
    const frame: VoiceAudioFrame = {
      kind: 'frame', sessionId: 'surviving-input', audioEpoch: 1, sampleIndex: 512,
      probability: 0.8, level: 0.7, bands: Array(32).fill(0.5), aecState: 'enabled',
      route: 'speaker', playbackActive: false, speechActive: true, speechTransition: 'start', discontinuity: false,
    };
    emitTauriEvent('vcp-voice-audio', frame);
    expect(store.level).toBe(0.7);
    expect(store.bands).toEqual(Array(32).fill(0.5));
    emitTauriEvent('vcp-voice-call', { ...recovered, revision: 8, inputSessionId: 'next-input' });
    await flushPromises();
    emitTauriEvent('vcp-voice-audio', { ...frame, sampleIndex: 1024, level: 0.1 });
    expect(store.level).toBe(0.7);
    emitTauriEvent('vcp-voice-audio', { ...frame, sessionId: 'next-input', level: 0.9 });
    expect(store.level).toBe(0.9);
    emitTauriEvent('vcp-voice-call', { ...recovered, revision: 9, micState: 'closed', inputSessionId: null });
    emitTauriEvent('vcp-voice-audio', { ...frame, sessionId: 'next-input', sampleIndex: 1024 });
    expect(store.level).toBe(0);
    expect(invokeMock.mock.calls.map(([name]) => name)).toEqual(expect.arrayContaining([
      'allocate_voice_document', 'connect_voice_document', 'get_voice_conversation_snapshot',
    ]));
    expect(invokeMock).toHaveBeenCalledTimes(3);
  });

  it('closes the Rust call capture without reconstructing or starting a local monitor', async () => {
    const stopped = deferred();
    mockInvoke('close_voice_microphone', () => stopped.promise);
    const store = useVoiceCallStore();
    await store.recover();
    expect(store.microphoneState).toBe('on');
    store.toggleMicrophone();
    await flushPromises();
    expect(invokeMock).toHaveBeenLastCalledWith('close_voice_microphone', { callId: recovered.callId, documentId: 'document-1' });
    expect(store.microphoneState).toBe('on');
    stopped.resolve();
    await flushPromises();
    expect(store.microphoneState).toBe('on');
    publishSnapshot({ micState: 'closed', micStopReason: 'requested' });
    expect(store.microphoneState).toBe('off');
    expect(invokeMock.mock.calls.some(([name]) => name === 'start_voice_microphone' || name === 'begin_voice_conversation')).toBe(false);
  });

  it('shows a failed close while keeping the microphone close action retryable', async () => {
    const stopped = deferred();
    mockInvoke('close_voice_microphone', () => stopped.promise);
    const store = useVoiceCallStore();
    await store.recover();
    store.toggleMicrophone();
    await flushPromises();
    stopped.reject(new Error('native stop failed'));
    await flushPromises();
    expect(store.microphoneState).toBe('on');
    expect(store.errorMessage).toBe('麦克风关闭失败，请重试。');
    const controls = mount(VoiceCallControls, {
      props: { microphoneState: store.microphoneState, errorMessage: store.errorMessage, level: 0, bands: [] },
    });
    expect(controls.find('[aria-label="关闭麦克风"]').exists()).toBe(true);
    expect(controls.get('[role="status"]').text()).toContain('麦克风关闭失败');
    controls.unmount();

    mockInvoke('close_voice_microphone', () => publishSnapshot({ micState: 'closed', micStopReason: 'requested' }));
    store.toggleMicrophone();
    await flushPromises();
    expect(invokeMock.mock.calls.filter(([name]) => name === 'close_voice_microphone')).toHaveLength(2);
    expect(invokeMock.mock.calls.some(([name]) => name === 'start_voice_microphone')).toBe(false);
    expect(store.microphoneState).toBe('off');
    expect(store.errorMessage).toBe('');
  });

  it('keeps an initializing microphone cancellation scoped to its exact call and session', async () => {
    const started = deferred();
    mockInvoke('start_voice_microphone', () => started.promise);
    const store = useVoiceCallStore();
    await store.recover();
    store.toggleMicrophone();
    await flushPromises();
    store.toggleMicrophone();
    await flushPromises();
    const request = invokeMock.mock.calls.find(([name]) => name === 'start_voice_microphone')![1]!;
    expect(store.microphoneState).toBe('requesting');
    store.toggleMicrophone();
    await flushPromises();
    expect(invokeMock).toHaveBeenLastCalledWith('close_voice_microphone', {
      callId: recovered.callId, sessionId: request.sessionId, documentId: 'document-1',
    });
    expect(store.microphoneState).toBe('off');
    started.resolve();
    await flushPromises();
    expect(store.microphoneState).toBe('off');
  });

  it.each(['starting', 'stopping'] as const)('restores %s from a new view without replaying an open intent', async micState => {
    mockInvoke('get_voice_conversation_snapshot', () => ({ ...recovered, micState }));
    const store = useVoiceCallStore();
    await store.recover();
    expect(store.microphoneState).toBe('requesting');
    expect(invokeMock.mock.calls.some(([name]) => name === 'start_voice_microphone' || name === 'begin_voice_conversation')).toBe(false);
    store.toggleMicrophone();
    await flushPromises();
    expect(invokeMock).toHaveBeenLastCalledWith('close_voice_microphone', { callId: recovered.callId, documentId: 'document-1' });
    expect(store.microphoneState).toBe('off');
  });

  it('keeps a failed stopping snapshot retryable after recreating the entire view', async () => {
    disposePinia(pinia);
    pinia = createPinia();
    setActivePinia(pinia);
    mockInvoke('allocate_voice_document', () => 'document-2');
    mockInvoke('get_voice_conversation_snapshot', () => ({
      ...recovered, micState: 'stopping', micStopReason: 'native stop failed',
    }));
    const store = useVoiceCallStore();
    await store.recover();
    expect(store.microphoneState).toBe('requesting');
    expect(store.errorMessage).toContain('操作失败');
    store.toggleMicrophone();
    await flushPromises();
    expect(invokeMock).toHaveBeenLastCalledWith('close_voice_microphone', { callId: recovered.callId, documentId: 'document-2' });
    expect(store.microphoneState).toBe('off');
  });

  it.each([true, false])('keeps authoritative closure silent after a late open rejection, starting observed=%s', async observed => {
    const started = deferred();
    mockInvoke('get_voice_conversation_snapshot', () => ({ ...recovered, micState: 'closed', inputSessionId: null }));
    mockInvoke('start_voice_microphone', args => {
      if (observed) publishSnapshot({ micState: 'starting', inputSessionId: args!.sessionId as string });
      return started.promise;
    });
    const store = useVoiceCallStore();
    await store.recover();
    store.toggleMicrophone();
    await flushPromises();
    expect(store.microphoneState).toBe('requesting');
    publishSnapshot({ micState: 'closed', inputSessionId: null, micStopReason: 'document_left' });
    expect(store.microphoneState).toBe('off');
    started.reject('cancelled: document left');
    await flushPromises();
    expect(store.microphoneState).toBe('off');
    expect(store.errorMessage).toBe('');
    expect(invokeMock.mock.calls.some(([name]) => name === 'close_voice_microphone')).toBe(false);
  });
});

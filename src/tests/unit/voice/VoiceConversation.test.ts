import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushPromises } from '@vue/test-utils';
import { createVoiceConversation, type VoiceCallSnapshot, type VoiceServiceView } from '../../../features/chat/voice/voiceConversation';
import type { ConversationKey } from '../../../core/stores/chatSessionStore';
import { emitTauriEvent, invokeMock, listenMock, mockInvoke } from '../../mocks/tauri';

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

const key: ConversationKey = { ownerId: 'agent', ownerType: 'agent', topicId: 'topic', epoch: 1 };
let onView: ReturnType<typeof vi.fn<(view: VoiceServiceView) => void>>;
let onSnapshot: ReturnType<typeof vi.fn<(snapshot: VoiceCallSnapshot | null) => void>>;
let onTargetDeleted: ReturnType<typeof vi.fn<(snapshot: VoiceCallSnapshot) => void>>;
let controller: ReturnType<typeof createVoiceConversation>;
let connectDocument: ReturnType<typeof vi.fn<() => Promise<string>>>;
async function beginCall() {
  const id = controller.begin();
  await flushPromises();
  return id;
}

function snapshot(overrides: Partial<VoiceCallSnapshot> = {}): VoiceCallSnapshot {
  return {
    callId: 'restored-call', target: { ownerId: 'agent', ownerType: 'agent', topicId: 'topic' },
    revision: 1, active: true, micState: 'closed', micStopReason: null, inputState: 'idle',
    replyState: 'idle', ttsState: 'idle', partial: '', error: null, notice: null,
    inputSessionId: null, lastPersistedMessageId: null,
    ...overrides,
  };
}

beforeEach(() => {
  onView = vi.fn();
  onSnapshot = vi.fn();
  onTargetDeleted = vi.fn();
  connectDocument = vi.fn().mockResolvedValue('document-1');
  mockInvoke('get_voice_conversation_snapshot', () => null);
  mockInvoke('begin_voice_conversation', args => snapshot({ callId: args!.callId as string }));
  controller = createVoiceConversation({
    conversation: () => key, window: () => ({ documentId: 1, focusRevision: 4, windowRevision: 5, anchorId: null }),
    connectDocument, onView, onSnapshot, onTargetDeleted,
  });
});
afterEach(() => controller.dispose());

describe('Rust voice conversation projection', () => {
  it('binds an explicit start intent to its target without exporting the Vue epoch', async () => {
    const callId = await beginCall();
    expect(invokeMock).toHaveBeenCalledWith('begin_voice_conversation', {
      callId, target: { ownerId: 'agent', ownerType: 'agent', topicId: 'topic' }, documentId: 1, expectedFocusRevision: 4,
    });
    expect(invokeMock.mock.calls.map(([name]) => name)).toEqual(['begin_voice_conversation']);
    expect(listenMock).toHaveBeenCalledExactlyOnceWith('vcp-voice-call', expect.any(Function));
  });

  it('subscribes before restoring a snapshot and never opens input or starts a new call on restore', async () => {
    mockInvoke('get_voice_conversation_snapshot', () => {
      expect(listenMock).toHaveBeenCalledWith('vcp-voice-call', expect.any(Function));
      return snapshot({ inputState: 'transcribing', partial: '后台录好的话' });
    });
    await controller.recover();
    expect(onSnapshot).toHaveBeenLastCalledWith(expect.objectContaining({ micState: 'closed', inputState: 'transcribing' }));
    expect(onView).toHaveBeenLastCalledWith({ state: 'recognizing', message: '正在识别', partialText: '后台录好的话' });
    expect(invokeMock.mock.calls.map(([name]) => name)).toEqual(['get_voice_conversation_snapshot']);
  });

  it('keeps the newer event when an older recovery snapshot arrives afterwards', async () => {
    const pending = deferred<VoiceCallSnapshot>();
    mockInvoke('get_voice_conversation_snapshot', () => pending.promise);
    const recovering = controller.recover();
    await flushPromises();
    emitTauriEvent('vcp-voice-call', snapshot({ revision: 4, ttsState: 'speaking', replyState: 'finished' }));
    pending.resolve(snapshot({ revision: 3, replyState: 'generating' }));
    await expect(recovering).resolves.toMatchObject({ revision: 4, ttsState: 'speaking' });
    expect(onSnapshot).toHaveBeenCalledOnce();
    expect(onView).toHaveBeenLastCalledWith(expect.objectContaining({ state: 'speaking' }));
  });

  it('ignores duplicate events and does not turn transcript or playback events into business commands', async () => {
    await controller.recover();
    onView.mockClear();
    onSnapshot.mockClear();
    const event = snapshot({ revision: 7, inputState: 'submitting' });
    emitTauriEvent('vcp-voice-call', event);
    emitTauriEvent('vcp-voice-call', event);
    emitTauriEvent('vcp-voice-service', { kind: 'transcript', text: '已交给 Rust', offerId: 'offer' });
    emitTauriEvent('vcp-tts', { kind: 'stopped', ttsId: 'tts' });
    await flushPromises();
    expect(onSnapshot).toHaveBeenCalledOnce();
    expect(onView).toHaveBeenCalledOnce();
    expect(invokeMock.mock.calls.map(([name]) => name)).toEqual(['get_voice_conversation_snapshot']);
  });

  it('presents playback while the microphone is closed without restarting capture', async () => {
    mockInvoke('get_voice_conversation_snapshot', () => snapshot({ ttsState: 'speaking', replyState: 'finished' }));
    await controller.recover();
    expect(onView).toHaveBeenLastCalledWith({ state: 'speaking', message: '正在朗读', partialText: '' });
    expect(onSnapshot).toHaveBeenLastCalledWith(expect.objectContaining({ micState: 'closed' }));
    expect(invokeMock.mock.calls.map(([name]) => name)).toEqual(['get_voice_conversation_snapshot']);
  });

  it('sends one hangup intent and ignores late active snapshots for that call', async () => {
    const callId = await beginCall();
    controller.stop();
    emitTauriEvent('vcp-voice-call', snapshot({ callId, revision: 10, ttsState: 'speaking' }));
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledWith('end_voice_conversation', { callId });
    expect(onView).toHaveBeenLastCalledWith({ state: 'off', message: '', partialText: '' });
    expect(invokeMock.mock.calls.some(([name]) => ['stop_tts', 'interrupt_chat_attempt', 'claim_voice_transcript', 'finish_tts_chat'].includes(name))).toBe(false);
  });

  it('listener disposal leaves the Rust call running and releases late subscription installation', async () => {
    const subscription = deferred<() => void>();
    const release = vi.fn();
    listenMock.mockImplementationOnce(() => subscription.promise);
    const recovering = controller.recover();
    controller.dispose();
    subscription.resolve(release);
    await recovering;
    expect(release).toHaveBeenCalledOnce();
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it('announces a live deletion once and silently restores an already ended call', async () => {
    const ended = snapshot({ active: false, endReason: 'target_deleted', revision: 3 });
    mockInvoke('get_voice_conversation_snapshot', () => ended);
    await expect(controller.recover()).resolves.toMatchObject({ endReason: 'target_deleted' });
    emitTauriEvent('vcp-voice-call', { ...ended, revision: 4 });
    expect(onTargetDeleted).not.toHaveBeenCalled();
    const removed = { ...ended, callId: 'new-call', revision: 5 };
    emitTauriEvent('vcp-voice-call', removed);
    emitTauriEvent('vcp-voice-call', { ...removed, revision: 6, error: 'cleanup failed' });
    expect(onTargetDeleted).toHaveBeenCalledExactlyOnceWith(removed);
  });

  it('keeps a live call visible and retryable when the hangup command fails', async () => {
    const callId = await beginCall();
    const ending = deferred<void>();
    mockInvoke('end_voice_conversation', () => ending.promise);
    mockInvoke('get_voice_conversation_snapshot', () => snapshot({ callId, revision: 2 }));
    const stopped = controller.stop();
    expect(onSnapshot).toHaveBeenLastCalledWith(expect.objectContaining({ active: true, callId }));
    ending.reject(new Error('end failed'));
    await expect(stopped).rejects.toThrow('end failed');
    expect(onSnapshot).toHaveBeenLastCalledWith(expect.objectContaining({ active: true, callId, revision: 2 }));
    mockInvoke('end_voice_conversation', () => emitTauriEvent('vcp-voice-call', snapshot({ callId, revision: 3, active: false })));
    await controller.stop();
    expect(onSnapshot).toHaveBeenLastCalledWith(expect.objectContaining({ active: false, callId }));
  });

  it('adopts a surviving replacement call after its predecessor ended while the view was frozen', async () => {
    await beginCall();
    const target = { ownerId: 'group', ownerType: 'group' as const, topicId: 'new' };
    mockInvoke('get_voice_conversation_snapshot', () => snapshot({ callId: 'replacement', revision: 9, target }));
    await expect(controller.recover()).resolves.toMatchObject({ callId: 'replacement', target });
    await controller.stop();
    expect(invokeMock).toHaveBeenCalledWith('end_voice_conversation', { callId: 'replacement' });
  });

  it('does not hang up an established call when its listener is disposed', async () => {
    await beginCall();
    const count = invokeMock.mock.calls.length;
    controller.dispose();
    emitTauriEvent('vcp-voice-call', snapshot({ revision: 20 }));
    expect(invokeMock.mock.calls).toHaveLength(count);
    expect(invokeMock.mock.calls.some(([name]) => name === 'end_voice_conversation')).toBe(false);
  });

  it('ignores a recovery response that predates a new explicit start intent', async () => {
    const pending = deferred<VoiceCallSnapshot>();
    mockInvoke('get_voice_conversation_snapshot', () => pending.promise);
    const recovering = controller.recover();
    await flushPromises();
    const newCallId = await beginCall();
    pending.resolve(snapshot({ revision: 50, callId: 'old-call' }));
    await expect(recovering).resolves.toBeNull();
    expect(onSnapshot).toHaveBeenLastCalledWith(expect.objectContaining({ callId: newCallId }));
  });

  it('does not let an empty recovery snapshot cancel a begin intent still being accepted', async () => {
    const beginning = deferred<VoiceCallSnapshot>();
    let callId = '';
    mockInvoke('begin_voice_conversation', args => {
      callId = args!.callId as string;
      return beginning.promise;
    });
    const starting = controller.begin();
    await flushPromises();
    await controller.recover();
    expect(onSnapshot).not.toHaveBeenCalled();
    beginning.resolve(snapshot({ callId }));
    await flushPromises();
    expect(starting).toBe(callId);
    expect(onSnapshot).toHaveBeenLastCalledWith(expect.objectContaining({ active: true, callId }));
  });

  it('recovers the actual call target even if the saved Vue topic differs', async () => {
    const target = { ...key, topicId: 'other-topic' };
    mockInvoke('get_voice_conversation_snapshot', () => snapshot({ target, revision: 2, replyState: 'generating' }));
    await controller.recover();
    onSnapshot.mockClear();
    emitTauriEvent('vcp-voice-call', snapshot({ target, revision: 3, replyState: 'finished' }));
    expect(onSnapshot).toHaveBeenCalledWith(expect.objectContaining({ target, replyState: 'finished' }));
    expect(invokeMock.mock.calls.map(([name]) => name)).toEqual(['get_voice_conversation_snapshot']);
  });

  it('retains a generated text reply when the projected TTS state reports an error', async () => {
    await controller.recover();
    emitTauriEvent('vcp-voice-call', snapshot({ replyState: 'generating', ttsState: 'error', error: '朗读暂不可用' }));
    expect(onView).toHaveBeenLastCalledWith({ state: 'thinking', message: '朗读暂不可用', partialText: '' });
    expect(invokeMock.mock.calls.map(([name]) => name)).toEqual(['get_voice_conversation_snapshot']);
  });

  it('presents a saved invite-only voice message as a notice rather than an error', async () => {
    mockInvoke('get_voice_conversation_snapshot', () => snapshot({
      replyState: 'finished', notice: '语音文本已保存；当前群聊为邀请发言模式。',
    }));
    await controller.recover();
    expect(onView).toHaveBeenLastCalledWith({
      state: 'listening', message: '语音文本已保存；当前群聊为邀请发言模式。', partialText: '',
    });
  });

  it('subscribes before connecting a document and still reads the call when capture cleanup fails', async () => {
    connectDocument.mockImplementation(async () => {
      expect(listenMock).toHaveBeenCalledWith('vcp-voice-call', expect.any(Function));
      throw new Error('native stop failed');
    });
    mockInvoke('get_voice_conversation_snapshot', () => snapshot({ micState: 'stopping', micStopReason: 'native stop failed' }));
    await expect(controller.recover()).resolves.toMatchObject({ micState: 'stopping' });
    expect(invokeMock.mock.calls.map(([name]) => name)).toEqual(['get_voice_conversation_snapshot']);
  });

  it('reads and projects the call while document capture cleanup is still pending', async () => {
    const cleanup = deferred<string>();
    connectDocument.mockReturnValueOnce(cleanup.promise);
    mockInvoke('get_voice_conversation_snapshot', () => snapshot({ micState: 'stopping' }));
    await expect(controller.recover()).resolves.toMatchObject({ micState: 'stopping' });
    expect(onSnapshot).toHaveBeenLastCalledWith(expect.objectContaining({ micState: 'stopping' }));
    cleanup.resolve('document-1');
  });

  it('freezes the tap lifecycle epoch while document admission is pending', async () => {
    const connection = deferred<string>();
    connectDocument.mockReturnValueOnce(connection.promise);
    mockInvoke('start_voice_microphone', () => ({ configured: true }));
    const opening = controller.openMicrophone(3);
    await flushPromises();
    connection.resolve('document-1');
    await expect(opening).resolves.toEqual({ configured: true });
    expect(invokeMock).toHaveBeenLastCalledWith('start_voice_microphone', {
      callId: expect.any(String), sessionId: expect.any(String), documentId: 'document-1', expectedLifecycleEpoch: 3,
    });
  });

  it('cancels an unsubmitted microphone intent without a native start or close', async () => {
    const beginning = deferred<VoiceCallSnapshot>();
    let callId = '';
    mockInvoke('begin_voice_conversation', args => {
      callId = args!.callId as string;
      return beginning.promise;
    });
    const opening = controller.openMicrophone();
    await flushPromises();
    await controller.closeMicrophone();
    beginning.resolve(snapshot({ callId }));
    await expect(opening).resolves.toBeNull();
    expect(invokeMock.mock.calls.map(([name]) => name)).toEqual(['begin_voice_conversation']);
  });

  it('preserves native call initialization failure after the inactive event cleared pending capture', async () => {
    mockInvoke('begin_voice_conversation', args => {
      emitTauriEvent('vcp-voice-call', snapshot({ callId: args!.callId as string, active: false, error: 'route unavailable' }));
      throw new Error('route unavailable');
    });
    await expect(controller.openMicrophone()).rejects.toThrow('route unavailable');
    expect(invokeMock.mock.calls.some(([name]) => name === 'start_voice_microphone')).toBe(false);
  });

  it.each(['resolve', 'reject'] as const)('leaves admission ordering and a late %s to Rust after early close', async outcome => {
    const first = deferred<{ configured: boolean }>();
    const closing = deferred<void>();
    mockInvoke('start_voice_microphone', () => first.promise);
    mockInvoke('close_voice_microphone', () => closing.promise);
    const opening = controller.openMicrophone();
    await flushPromises();
    const original = invokeMock.mock.calls.find(([name]) => name === 'start_voice_microphone')![1]!;
    const stopping = controller.closeMicrophone();
    await flushPromises();
    expect(invokeMock).toHaveBeenLastCalledWith('close_voice_microphone', {
      callId: original.callId, sessionId: original.sessionId, documentId: 'document-1',
    });
    mockInvoke('start_voice_microphone', () => ({ configured: true }));
    await expect(controller.openMicrophone()).resolves.toEqual({ configured: true });
    if (outcome === 'resolve') first.resolve({ configured: true });
    else first.reject('old permission denied');
    await expect(opening).resolves.toBeNull();
    expect(invokeMock.mock.calls.filter(([name]) => name === 'close_voice_microphone')).toHaveLength(1);
    closing.resolve();
    await stopping;
  });

  it('does not require old JS to compensate a start that completes after disposal', async () => {
    const first = deferred<{ configured: boolean }>();
    mockInvoke('start_voice_microphone', () => first.promise);
    const opening = controller.openMicrophone();
    await flushPromises();
    controller.dispose();
    first.resolve({ configured: true });
    await expect(opening).resolves.toBeNull();
    expect(invokeMock.mock.calls.some(([name]) => name === 'close_voice_microphone')).toBe(false);
  });

});

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
let current = true;
let onView: ReturnType<typeof vi.fn<(view: VoiceServiceView) => void>>;
let onSnapshot: ReturnType<typeof vi.fn<(snapshot: VoiceCallSnapshot | null) => void>>;
let controller: ReturnType<typeof createVoiceConversation>;

function snapshot(overrides: Partial<VoiceCallSnapshot> = {}): VoiceCallSnapshot {
  return {
    callId: 'restored-call', target: { ownerId: 'agent', ownerType: 'agent', topicId: 'topic' },
    revision: 1, active: true, micState: 'closed', inputState: 'idle',
    replyState: 'idle', ttsState: 'idle', partial: '', error: null, notice: null,
    inputSessionId: null, originUserMessageId: null, ttsId: null, lastPersistedMessageId: null,
    ...overrides,
  };
}

beforeEach(() => {
  current = true;
  onView = vi.fn();
  onSnapshot = vi.fn();
  mockInvoke('get_voice_conversation_snapshot', () => null);
  mockInvoke('begin_voice_conversation', args => snapshot({ callId: args!.callId as string }));
  controller = createVoiceConversation({
    conversation: () => key, isConversationCurrent: () => current, onView, onSnapshot,
  });
});
afterEach(() => controller.dispose());

describe('Rust voice conversation projection', () => {
  it('binds an explicit start intent to its target without exporting the Vue epoch', async () => {
    const callId = await controller.start('mic');
    expect(invokeMock).toHaveBeenCalledWith('begin_voice_conversation', {
      callId, target: { ownerId: 'agent', ownerType: 'agent', topicId: 'topic' },
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
    const event = snapshot({ revision: 7, inputState: 'submitting', originUserMessageId: 'voice-user' });
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
    const callId = await controller.start('mic');
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

  it('does not hang up an established call when its listener is disposed', async () => {
    await controller.start('mic');
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
    const newCallId = await controller.start('new-mic');
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
    const starting = controller.start('mic');
    await flushPromises();
    await controller.recover();
    expect(onSnapshot).not.toHaveBeenCalled();
    beginning.resolve(snapshot({ callId }));
    await expect(starting).resolves.toBe(callId);
    expect(onSnapshot).toHaveBeenLastCalledWith(expect.objectContaining({ active: true, callId }));
  });

  it('keeps the Vue epoch a display guard and does not cancel background Rust work', async () => {
    mockInvoke('get_voice_conversation_snapshot', () => snapshot({ revision: 2, replyState: 'generating' }));
    await controller.recover();
    onSnapshot.mockClear();
    current = false;
    emitTauriEvent('vcp-voice-call', snapshot({ revision: 3, replyState: 'finished' }));
    expect(onSnapshot).not.toHaveBeenCalled();
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

});

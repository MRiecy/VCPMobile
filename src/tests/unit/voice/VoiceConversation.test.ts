import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushPromises } from '@vue/test-utils';
import { createVoiceConversation, type VoiceServiceEvent, type VoiceServiceView } from '../../../features/chat/voice/voiceConversation';
import type { ConversationKey } from '../../../core/stores/chatSessionStore';
import type { StreamEventDto } from '../../../core/types/chat';
import type { VoiceMessageScope } from '../../../core/stores/chatHistoryStore';
import { emitTauriEvent, invokeMock, mockInvoke } from '../../mocks/tauri';

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

const key: ConversationKey = { ownerId: 'agent', ownerType: 'agent', topicId: 'topic', epoch: 1 };
let current = true;
let listeners: Set<(event: StreamEventDto) => void>;
let finishSend: ReturnType<typeof deferred<void>>;
let sendMessage: ReturnType<typeof vi.fn<(text: string, key: ConversationKey, scope: VoiceMessageScope) => Promise<void>>>;
let onView: ReturnType<typeof vi.fn<(view: VoiceServiceView) => void>>;
let controller: ReturnType<typeof createVoiceConversation>;

function service(kind: VoiceServiceEvent['kind'], overrides: Partial<VoiceServiceEvent> = {}) {
  const calls = invokeMock.mock.calls.filter(([name]) => name === 'plugin:vcp-mobile|begin_voice_call');
  const callId = calls[calls.length - 1]?.[1]?.callId;
  const event = { sessionId: 'mic', callId, kind, turnId: 'turn-1', offerId: overrides.turnId ?? 'turn-1', ...overrides };
  if (kind === 'transcript' && event.text && !offeredText.has(event.offerId)) offeredText.set(event.offerId, event.text);
  emitTauriEvent('vcp-voice-service', event);
}
const offeredText = new Map<string, string>();
function origin() { return sendMessage.mock.calls[sendMessage.mock.calls.length - 1][2].messageId; }
function chat(type: StreamEventDto['type'], overrides: Partial<StreamEventDto> = {}) {
  const event: StreamEventDto = {
    type, messageId: 'reply-1', consumerRevision: 1,
    context: { ...key, originUserMessageId: origin(), requestId: 'attempt-1' },
    chunk: null, aurora: null, blocks: [], content: null, error: null, finishReason: null,
    timestamp: null, topicUpdatedAt: null, ...overrides,
  };
  for (const listener of listeners) listener(event);
}

beforeEach(() => {
  offeredText.clear();
  current = true;
  listeners = new Set();
  finishSend = deferred<void>();
  sendMessage = vi.fn(() => finishSend.promise);
  onView = vi.fn();
  mockInvoke('start_voice_service', () => ({ configured: true }));
  mockInvoke('start_tts_session', () => ({ configured: true }));
  mockInvoke('claim_voice_transcript', args => ({ accepted: true, text: offeredText.get(args?.offerId as string), turnId: args?.offerId }));
  controller = createVoiceConversation({
    conversation: () => key, isConversationCurrent: () => current,
    observeChat: callback => { listeners.add(callback); return () => { listeners.delete(callback); }; },
    sendMessage, onView,
  });
});
afterEach(async () => {
  controller.stop();
  finishSend.resolve();
  await flushPromises();
  vi.useRealTimers();
});

describe('independent input and reply lifetimes', () => {
  it('registers the call lease and leaves unconfigured input local', async () => {
    mockInvoke('start_voice_service', () => ({ configured: false }));
    const callId = await controller.start('mic');
    expect(invokeMock).toHaveBeenCalledWith('plugin:vcp-mobile|begin_voice_call', { callId });
    service('transcript', { text: 'local only' });
    await flushPromises();
    expect(sendMessage).not.toHaveBeenCalled();
    expect(onView).toHaveBeenLastCalledWith(expect.objectContaining({ state: 'local' }));
  });

  it('sends only the unique final transcript and binds TTS before sending chat without a JS text pump', async () => {
    await controller.start('mic');
    service('asr-partial', { text: '临时' });
    expect(sendMessage).not.toHaveBeenCalled();
    service('transcript', { text: '最终文字' });
    service('transcript', { text: '重复最终文字' });
    await flushPromises();
    expect(sendMessage).toHaveBeenCalledOnce();
    expect(sendMessage.mock.calls[0][0]).toBe('最终文字');
    expect(invokeMock).toHaveBeenCalledWith('start_tts_session', { ttsId: expect.any(String), originUserMessageId: origin() });
    chat('aurora', { aurora: { kind: 'delta', chunk: '正文。' } });
    chat('end', { content: '正文。', finishReason: 'stop' });
    expect(invokeMock.mock.calls.some(([name]) => name === 'submit_tts_text' || name === 'submit_voice_reply')).toBe(false);
  });

  it('drains accepted ASR after muting and retains the call lease and existing replies', async () => {
    const callId = await controller.start('mic');
    controller.closeInput('mic', { audioEpoch: 1, sampleIndex: 731 });
    expect(invokeMock).toHaveBeenCalledWith('close_voice_input', { sessionId: 'mic', audioEpoch: 1, sampleIndex: 731 });
    expect(invokeMock.mock.calls.some(([name]) => ['stop_voice_service', 'stop_tts', 'interrupt_chat_attempt', 'plugin:vcp-mobile|end_voice_call'].includes(name))).toBe(false);
    service('transcript', { text: '关麦前录好的话' });
    await flushPromises();
    expect(sendMessage).toHaveBeenCalledOnce();
    expect(await controller.start('mic-next')).toBe(callId);
    service('status', { stage: 'closed' });
    service('transcript', { text: '重复结果' });
    expect(sendMessage).toHaveBeenCalledOnce();
  });

  it('hangup stops this call input, model attempts and independent TTS and ignores late ASR', async () => {
    const callId = await controller.start('mic');
    service('transcript', { text: '你好' });
    await flushPromises();
    chat('request_activity', { chunk: { active: true } });
    const tts = invokeMock.mock.calls.find(([name]) => name === 'start_tts_session')![1]!.ttsId;
    controller.stop();
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledWith('stop_voice_service', { sessionId: 'mic' });
    expect(invokeMock).toHaveBeenCalledWith('stop_tts', { ttsId: tts });
    expect(invokeMock).toHaveBeenCalledWith('interrupt_chat_attempt', expect.objectContaining({ attemptId: 'attempt-1', kind: 'request' }));
    expect(invokeMock).toHaveBeenCalledWith('plugin:vcp-mobile|end_voice_call', { callId });
    service('transcript', { turnId: 'late', text: '不应发送' });
    expect(sendMessage).toHaveBeenCalledOnce();
  });

  it('barge-in cancels a late first activity precisely and never touches another origin', async () => {
    await controller.start('mic');
    service('transcript', { text: '你好' });
    await flushPromises();
    service('barge-in');
    chat('request_activity', { context: { ...key, originUserMessageId: 'typed', requestId: 'unrelated' }, chunk: { active: true } });
    chat('request_activity', { chunk: { active: true } });
    expect(invokeMock).toHaveBeenCalledWith('interrupt_chat_attempt', {
      ownerId: 'agent', ownerType: 'agent', topicId: 'topic', attemptId: 'attempt-1', kind: 'request',
    });
    expect(invokeMock.mock.calls.filter(([name, args]) => name === 'interrupt_chat_attempt' && args?.attemptId === 'unrelated')).toHaveLength(0);
    expect(invokeMock).toHaveBeenCalledWith('stop_tts', { ttsId: expect.any(String) });
  });

  it.each(['error', 'reconnecting'] as const)('does not treat automatic model %s as a user stop', async type => {
    await controller.start('mic');
    service('transcript', { text: '你好' });
    await flushPromises();
    chat('request_activity', { chunk: { active: true } });
    chat(type, { error: 'temporary network failure' });
    expect(invokeMock.mock.calls.some(([name]) => name === 'stop_tts' || name === 'interrupt_chat_attempt')).toBe(false);
    expect(sendMessage.mock.calls[0][2].isCurrent()).toBe(true);
  });

  it('keeps generation valid after TTS or ASR failure', async () => {
    await controller.start('mic');
    service('transcript', { text: '你好' });
    await flushPromises();
    const ttsId = invokeMock.mock.calls.find(([name]) => name === 'start_tts_session')![1]!.ttsId;
    emitTauriEvent('vcp-tts', { ttsId, kind: 'error', stage: 'tts', message: '朗读失败' });
    service('error', { turnId: 'another-asr', stage: 'asr', message: '识别失败' });
    expect(invokeMock.mock.calls.some(([name]) => name === 'interrupt_chat_attempt')).toBe(false);
    expect(sendMessage.mock.calls[0][2].isCurrent()).toBe(true);
  });

  it('still sends recognized text if independent TTS cannot start', async () => {
    mockInvoke('start_tts_session', () => Promise.reject('tts configuration failure'));
    await controller.start('mic');
    service('transcript', { text: '可以文字回复' });
    await flushPromises();
    expect(sendMessage).toHaveBeenCalledOnce();
    expect(sendMessage.mock.calls[0][0]).toBe('可以文字回复');
  });

  it('waits for its own cancelled group owner to release before sending the next recognized turn', async () => {
    const nextSend = deferred<void>();
    sendMessage.mockReturnValueOnce(finishSend.promise).mockReturnValueOnce(nextSend.promise);
    await controller.start('mic');
    service('transcript', { text: '第一句' });
    await flushPromises();
    chat('group_turn', { context: { ...key, originUserMessageId: origin(), turnId: 'group-attempt' }, chunk: { active: true } });
    service('barge-in');
    service('transcript', { turnId: 'turn-2', text: '第二句' });
    await flushPromises();
    expect(sendMessage).toHaveBeenCalledOnce();
    finishSend.resolve();
    await flushPromises();
    expect(sendMessage).toHaveBeenCalledTimes(2);
    expect(sendMessage.mock.calls[1][0]).toBe('第二句');
    controller.stop();
    nextSend.resolve();
    await flushPromises();
  });

  it('waits for the previous end-call acknowledgement without waiting for its late begin', async () => {
    const oldBegin = deferred<void>();
    const oldEnd = deferred<void>();
    let firstId = '';
    mockInvoke('plugin:vcp-mobile|begin_voice_call', args => {
      if (!firstId) { firstId = args!.callId as string; return oldBegin.promise; }
    });
    mockInvoke('plugin:vcp-mobile|end_voice_call', args => args!.callId === firstId ? oldEnd.promise : undefined);
    controller.begin();
    controller.stop();
    const starting = controller.start('mic-next');
    await flushPromises();
    expect(invokeMock.mock.calls.filter(([name]) => name === 'plugin:vcp-mobile|begin_voice_call')).toHaveLength(1);
    oldEnd.resolve();
    const nextId = await starting;
    expect(nextId).not.toBe(firstId);
    oldBegin.resolve();
    await flushPromises();
    expect(onView).toHaveBeenLastCalledWith(expect.objectContaining({ state: 'listening' }));
  });

  it('does not send after hangup while TTS registration is still pending', async () => {
    const tts = deferred<{ configured: boolean }>();
    mockInvoke('start_tts_session', () => tts.promise);
    await controller.start('mic');
    service('transcript', { text: '未发送' });
    await flushPromises();
    controller.stop();
    tts.resolve({ configured: true });
    await flushPromises();
    expect(sendMessage).not.toHaveBeenCalled();
    expect(invokeMock).toHaveBeenCalledWith('stop_tts', { ttsId: expect.any(String) });
  });

  it('releases a failed call reservation and allows the next microphone attempt to create a fresh call', async () => {
    let first = true;
    mockInvoke('plugin:vcp-mobile|begin_voice_call', () => {
      if (first) { first = false; return Promise.reject('call reservation failed'); }
    });
    await expect(controller.start('mic')).rejects.toBe('call reservation failed');
    const id = await controller.start('mic-retry');
    const calls = invokeMock.mock.calls.filter(([name]) => name === 'plugin:vcp-mobile|begin_voice_call');
    expect(calls).toHaveLength(2);
    expect(calls[0][1]?.callId).not.toBe(id);
    expect(calls[1][1]?.callId).toBe(id);
    expect(invokeMock.mock.calls.some(([name]) => name === 'stop_tts')).toBe(false);
  });

  it('claims the final server-merged offer once and discards an invalidated pre-send offer', async () => {
    mockInvoke('claim_voice_transcript', args => args?.offerId === 'turn-1'
      ? { accepted: false } : { accepted: true, text: '第一句。第二句。', turnId: 'turn-2' });
    await controller.start('mic');
    service('transcript', { text: '第一句。' });
    await flushPromises();
    expect(sendMessage).not.toHaveBeenCalled();
    expect(invokeMock).toHaveBeenCalledWith('stop_tts', { ttsId: expect.any(String) });
    controller.closeInput('mic');
    service('status', { stage: 'closed' });
    service('transcript', { turnId: 'turn-2', text: '候选结果' });
    await flushPromises();
    expect(sendMessage).toHaveBeenCalledOnce();
    expect(sendMessage.mock.calls[0][0]).toBe('第一句。第二句。');
  });

  it('closes an empty TTS producer when persistence fails without cancelling model or submitted speech', async () => {
    sendMessage.mockImplementationOnce(() => Promise.reject('append failed'));
    await controller.start('mic');
    service('transcript', { text: '还没保存' });
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledWith('finish_tts_chat', { ttsId: expect.any(String) });
    expect(invokeMock.mock.calls.some(([name]) => name === 'stop_tts')).toBe(false);
    expect(invokeMock.mock.calls.some(([name]) => name === 'interrupt_chat_attempt')).toBe(false);
  });

  it('returns empty recognition to listening without overwriting speech playback on input restart', async () => {
    await controller.start('mic');
    service('asr-partial', { text: '临时识别' });
    expect(onView).toHaveBeenLastCalledWith(expect.objectContaining({ state: 'recognizing' }));
    service('status', { stage: 'listening' });
    expect(onView).toHaveBeenLastCalledWith(expect.objectContaining({ state: 'listening', partialText: '' }));

    service('transcript', { text: '继续回复' });
    await flushPromises();
    const ttsId = invokeMock.mock.calls.find(([name]) => name === 'start_tts_session')![1]!.ttsId;
    emitTauriEvent('vcp-tts', { ttsId, kind: 'status', stage: 'speaking' });
    service('status', { stage: 'listening' });
    controller.closeInput('mic');
    await controller.start('mic-next');
    expect(onView).toHaveBeenLastCalledWith(expect.objectContaining({ state: 'speaking', message: '正在朗读' }));
  });
});

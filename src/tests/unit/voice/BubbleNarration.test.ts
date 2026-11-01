import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, disposePinia, setActivePinia, type Pinia } from 'pinia';
import { reactive } from 'vue';
import { flushPromises } from '@vue/test-utils';
import { useBubbleNarration } from '../../../features/chat/voice/useBubbleNarration';
import { useChatSessionStore } from '../../../core/stores/chatSessionStore';
import { installWindow, mockWindowRead } from '../../helpers/chatWindow';
import { useChatStreamStore } from '../../../core/stores/chatStreamStore';
import { useChatHistoryStore, type MessageActionKey } from '../../../core/stores/chatHistoryStore';
import { emitTauriEvent, invokeMock, listenMock, mockInvoke } from '../../mocks/tauri';

const context = vi.hoisted(() => ({ voice: {} as { active: boolean }, notify: vi.fn() }));
vi.mock('../../../features/chat/voice/voiceCallStore', () => ({ useVoiceCallStore: () => context.voice }));
vi.mock('../../../core/stores/notification', () => ({ useNotificationStore: () => ({ addNotification: context.notify }) }));

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(accept => { resolve = accept; });
  return { promise, resolve };
}
let pinia: Pinia;
const calls = (name: string) => invokeMock.mock.calls.filter(([command]) => command === name);
const key = (id: string): MessageActionKey => ({
  messageId: id, conversation: { ...useChatSessionStore().currentConversationKey! },
});
beforeEach(() => {
  pinia = createPinia();
  setActivePinia(pinia);
  context.voice = reactive({ active: false });
  context.notify.mockReset();
  mockInvoke('allocate_voice_document', () => '42');
  useChatSessionStore().setConversation({ id: 'agent', type: 'agent' }, 'topic');
  mockInvoke('start_tts_session', () => ({ configured: true }));
  mockInvoke('stop_tts', () => undefined);
});
afterEach(async () => { disposePinia(pinia); await flushPromises(); });

describe('application bubble narration owner', () => {
  it('submits the complete raw message and keeps exactly one listener across switches', async () => {
    const owner = useBubbleNarration();
    const raw = '<voice_guidance>自然</voice_guidance>\n第一段\n[brk]\n第二段';
    await owner.startBubbleNarration(key('a'), raw);
    const id = owner.activeTtsId;
    expect(calls('submit_tts_text')[0][1]).toEqual({ ttsId: id, text: raw, finished: true });
    expect(calls('start_tts_session')[0][1]?.bubbleSource).toEqual({
      topic: { ownerType: 'agent', ownerId: 'agent', topicId: 'topic' }, messageId: 'a',
    });
    expect(calls('start_tts_session')[0][1]?.documentId).toBe('42');
    await owner.startBubbleNarration(key('b'), '第二条');
    expect(calls('stop_tts')[0][1]?.ttsId).toBe(id);
    expect(listenMock.mock.calls.filter(([event]) => event === 'vcp-tts')).toHaveLength(1);
    emitTauriEvent('vcp-tts', { ttsId: id, kind: 'error', message: 'old' });
    expect(owner.activeMessageId).toBe('b');
    expect(calls('connect_tts_document')).toHaveLength(1);
    expect(context.notify).not.toHaveBeenCalled();
  });

  it('serializes A → B → C and waits for actual stop even after a terminal event', async () => {
    const owner = useBubbleNarration();
    await owner.startBubbleNarration(key('a'), 'A');
    const id = owner.activeTtsId!;
    const stopping = deferred<void>();
    mockInvoke('stop_tts', () => stopping.promise);
    emitTauriEvent('vcp-tts', { ttsId: id, kind: 'stopped' });
    const b = owner.startBubbleNarration(key('b'), 'B');
    await flushPromises();
    const c = owner.startBubbleNarration(key('c'), 'C');
    await flushPromises();
    expect(calls('start_tts_session')).toHaveLength(1);
    stopping.resolve();
    await Promise.all([b, c]);
    expect(calls('submit_tts_text').map(([, args]) => args?.text)).toEqual(['A', 'C']);
    expect(owner.activeMessageId).toBe('c');
  });

  it('stops a late start acknowledgement before allowing a new task', async () => {
    const starting = deferred<{ configured: boolean }>();
    mockInvoke('start_tts_session', () => starting.promise);
    const owner = useBubbleNarration();
    const start = owner.startBubbleNarration(key('a'), 'A');
    await flushPromises();
    const id = calls('start_tts_session')[0][1]?.ttsId;
    let released = false;
    const stop = owner.stopBubbleNarration().then(() => { released = true; });
    await flushPromises();
    expect(released).toBe(false);
    starting.resolve({ configured: true });
    await Promise.all([start, stop]);
    expect(calls('submit_tts_text')).toHaveLength(0);
    expect(calls('stop_tts').map(([, args]) => args?.ttsId)).toEqual([id]);
  });

  it('revokes a pending raw read when switching topics and when a newer click wins', async () => {
    const owner = useBubbleNarration();
    const reading = deferred<string>();
    const a = owner.startBubbleNarration(key('a'), () => reading.promise);
    await flushPromises();
    useChatSessionStore().setConversation({ id: 'other', type: 'agent' }, 'other-topic');
    const b = owner.startBubbleNarration(key('b'), 'B');
    reading.resolve('late A');
    await Promise.all([a, b]);
    expect(calls('submit_tts_text').map(([, args]) => args?.text)).toEqual(['B']);
  });

  it('binds stop to the captured message and task identity', async () => {
    const owner = useBubbleNarration();
    const message = key('same');
    await owner.startBubbleNarration(message, 'old');
    const id = owner.activeTtsId!;
    await owner.startBubbleNarration(message, 'new');
    const stops = calls('stop_tts').length;
    await owner.stopMessage(message, id);
    await owner.stopMessage({ ...message, conversation: { ...message.conversation, epoch: 99 } });
    expect(calls('stop_tts')).toHaveLength(stops);
    expect(owner.isNarrating(message)).toBe(true);
  });

  it('blocks calls and cancels bubble admission when a call starts', async () => {
    const owner = useBubbleNarration();
    context.voice.active = true;
    await owner.startBubbleNarration(key('a'), 'A');
    expect(calls('start_tts_session')).toHaveLength(0);
    expect(context.notify).toHaveBeenCalledOnce();
    context.voice.active = false;
    await owner.startBubbleNarration(key('a'), 'A');
    context.voice.active = true;
    await flushPromises();
    expect(owner.activeMessageId).toBeNull();
    expect(calls('stop_tts')).toHaveLength(1);
  });

  it('reports an error once and releases unconfigured or failed tasks', async () => {
    const owner = useBubbleNarration();
    mockInvoke('start_tts_session', () => ({ configured: false }));
    await owner.startBubbleNarration(key('a'), 'A');
    expect(owner.activeTtsId).toBeNull();
    expect(calls('stop_tts')).toHaveLength(1);
    mockInvoke('start_tts_session', () => ({ configured: true }));
    mockInvoke('submit_tts_text', args => {
      emitTauriEvent('vcp-tts', { ttsId: args?.ttsId, kind: 'error', message: '没有正文' });
      return Promise.reject('没有正文');
    });
    await owner.startBubbleNarration(key('b'), 'B');
    expect(context.notify).toHaveBeenCalledTimes(2);
    expect(calls('stop_tts')).toHaveLength(2);
  });

  it.each(['edit', 'delete'] as const)('actual %s waits for source playback release', async action => {
    installWindow(useChatSessionStore(), { id: 'agent', type: 'agent' }, 'topic');
    mockWindowRead([{ id: 'a', role: 'assistant', content: '原文', timestamp: 1 }]);
    mockInvoke('reconnect_chat_streams', () => ({ generations: [], activeRequests: [], groupTurns: [] }));
    mockInvoke('delete_messages', () => ({ msgCount: 0, updatedAt: 2 }));
    mockInvoke('patch_single_message', () => ({ blocks: [], topicUpdatedAt: 2 }));
    const history = useChatHistoryStore();
    await history.ensureWindow();
    await useChatStreamStore().checkAndRecoverInterruptedStreams();
    const owner = useBubbleNarration();
    await owner.startBubbleNarration(key('a'), '原文');
    const stopping = deferred<void>();
    mockInvoke('stop_tts', () => stopping.promise);
    const operation = action === 'edit' ? history.updateMessageContent(key('a'), '改文') : history.deleteMessage(key('a'));
    await flushPromises();
    const command = action === 'edit' ? 'patch_single_message' : 'delete_messages';
    expect(calls(command)).toHaveLength(0);
    stopping.resolve();
    await operation;
    expect(calls(command)).toHaveLength(1);
  });
});

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, disposePinia, setActivePinia, type Pinia } from 'pinia';
import { flushPromises } from '@vue/test-utils';
import { useChatHistoryStore } from '../../../core/stores/chatHistoryStore';
import { useChatSessionStore } from '../../../core/stores/chatSessionStore';
import { useChatStreamStore } from '../../../core/stores/chatStreamStore';
import { useAttachmentStore } from '../../../core/stores/attachmentStore';
import { useSettingsStore } from '../../../core/stores/settings';
import { useShareStore } from '../../../core/stores/shareStore';
import { invokeMock, mockInvoke, type MockChannel } from '../../mocks/tauri';

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(finish => { resolve = finish; });
  return { promise, resolve };
}

let pinia: Pinia;
beforeEach(() => {
  pinia = createPinia();
  setActivePinia(pinia);
  mockInvoke('reconnect_chat_streams', () => ({ generations: [], activeRequests: [], groupTurns: [] }));
  mockInvoke('load_chat_history', () => []);
  mockInvoke('append_single_message', () => ({ blocks: [], topicUpdatedAt: 1 }));
  mockInvoke('handle_agent_chat_message', () => ({ status: 'sent' }));
});
afterEach(() => disposePinia(pinia));

async function setup() {
  const session = useChatSessionStore();
  session.setConversation({ id: 'agent', type: 'agent' }, 'topic');
  useSettingsStore().settings = { userName: 'User' } as never;
  const history = useChatHistoryStore();
  await history.loadHistoryPaginated('agent', 'agent', 'topic');
  return { session, history, key: { ...session.currentConversationKey! } };
}

describe('voice submission isolation', () => {
  it('persists only recognized text and preserves staged uploads, share draft and edit-resend intent', async () => {
    const { history, key } = await setup();
    const attachments = useAttachmentStore();
    attachments.stagedAttachments = [{ id: 'upload', type: 'file', name: 'pending.pdf', size: 123, src: 'pending', status: 'loading' }];
    const share = useShareStore();
    share.current = {
      state: 'draft', generation: 1, deliveryId: 'share', text: '未发送草稿', items: [], attachments: [],
      agentId: 'agent', topicId: 'topic', revision: 1,
    };
    const editing = { key: { conversation: key, messageId: 'original' }, initialContent: '正在编辑' };
    history.editingMessage = editing;
    await history.sendVoiceMessage('语音文字', key, { messageId: 'voice-user', isCurrent: () => true });

    const append = invokeMock.mock.calls.find(([command]) => command === 'append_single_message')!;
    expect(append[1]).toMatchObject({ ownerId: 'agent', ownerType: 'agent', topicId: 'topic', message: { id: 'voice-user', content: '语音文字' } });
    expect((append[1]?.message as Record<string, unknown>).attachments).toBeUndefined();
    expect(attachments.stagedAttachments[0].id).toBe('upload');
    expect(share.activeDraft?.text).toBe('未发送草稿');
    expect(share.sending).toBe(false);
    expect(history.editingMessage).toEqual(editing);
    expect(invokeMock.mock.calls.some(([command]) => command === 'truncate_history_after_message' || command.includes('commit_share'))).toBe(false);
  });

  it('does not dispatch a generation or insert a late visible message after its frozen conversation changes', async () => {
    const { history, session, key } = await setup();
    const saved = deferred<{ blocks: []; topicUpdatedAt: number }>();
    mockInvoke('append_single_message', () => saved.promise);
    const sending = history.sendVoiceMessage('旧会话语音', key, { messageId: 'old-voice', isCurrent: () => true });
    await flushPromises();
    session.setConversation({ id: 'other', type: 'agent' }, 'other-topic');
    await history.loadHistoryPaginated('other', 'agent', 'other-topic');
    saved.resolve({ blocks: [], topicUpdatedAt: 2 });
    await sending;
    expect(history.currentChatHistory).toEqual([]);
    expect(invokeMock.mock.calls.some(([command]) => command === 'handle_agent_chat_message')).toBe(false);
  });

  it('observes origin identity from the registered reconnect output while preserving normal chat rendering', async () => {
    const { history } = await setup();
    let output: MockChannel<unknown> | undefined;
    mockInvoke('reconnect_chat_streams', args => {
      output = args?.channel as MockChannel<unknown>;
      return { generations: [], activeRequests: [], groupTurns: [] };
    });
    const streams = useChatStreamStore();
    const observed = vi.fn();
    const release = streams.observeEvents(observed);
    await streams.checkAndRecoverInterruptedStreams();
    output!.emit({
      type: 'request_activity', messageId: '', consumerRevision: 1, chunk: { active: true },
      context: { ownerId: 'agent', ownerType: 'agent', topicId: 'topic', requestId: 'attempt', originUserMessageId: 'voice-user' },
    });
    output!.emit({
      type: 'end', messageId: 'reply', consumerRevision: 2, content: '正常回复', blocks: [], finishReason: 'stop',
      context: { ownerId: 'agent', ownerType: 'agent', topicId: 'topic', originUserMessageId: 'voice-user' },
    });
    await flushPromises();
    expect(observed).toHaveBeenCalledWith(expect.objectContaining({ type: 'request_activity', context: expect.objectContaining({ originUserMessageId: 'voice-user' }) }));
    expect(history.currentChatHistory.some(message => message.id === 'reply' && message.content === '正常回复')).toBe(true);
    release();
  });
});

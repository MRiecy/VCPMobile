import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, disposePinia, setActivePinia, type Pinia } from 'pinia';
import { MAX_HISTORY_MESSAGES, useChatHistoryStore } from '../../../core/stores/chatHistoryStore';
import { useTopicStore } from '../../../core/stores/topicListManager';
import type { ChatMessage } from '../../../core/types/chat';
import { useChatSessionStore } from '../../../core/stores/chatSessionStore';
import { useAttachmentStore } from '../../../core/stores/attachmentStore';
import { useSettingsStore } from '../../../core/stores/settings';
import { useShareStore } from '../../../core/stores/shareStore';
import { invokeMock, mockInvoke } from '../../mocks/tauri';

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

describe('persisted voice message projection', () => {
  it('merges persisted text and preserves staged uploads, share draft and edit-resend intent', async () => {
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
    const increment = vi.spyOn(useTopicStore(), 'incrementTopicMsgCount');
    const message: ChatMessage = { id: 'voice-user', role: 'user', content: '语音文字', timestamp: 1 };
    history.mergePersistedVoiceMessage(key, message, 1);
    history.mergePersistedVoiceMessage(key, message, 1);

    expect(history.currentChatHistory).toHaveLength(1);
    expect(history.currentChatHistory[0]).toMatchObject(message);
    expect(increment).not.toHaveBeenCalled();
    expect(invokeMock.mock.calls.some(([command]) => command === 'append_single_message'
      || command === 'handle_agent_chat_message' || command.includes('haptic') || command.includes('tts'))).toBe(false);
    expect(attachments.stagedAttachments[0].id).toBe('upload');
    expect(share.activeDraft?.text).toBe('未发送草稿');
    expect(share.sending).toBe(false);
    expect(history.editingMessage).toEqual(editing);
    expect(invokeMock.mock.calls.some(([command]) => command === 'truncate_history_after_message' || command.includes('commit_share'))).toBe(false);
  });

  it('does not insert a late message or dispatch generation after its frozen conversation changes', async () => {
    const { history, session, key } = await setup();
    session.setConversation({ id: 'other', type: 'agent' }, 'other-topic');
    await history.loadHistoryPaginated('other', 'agent', 'other-topic');
    history.mergePersistedVoiceMessage(key, { id: 'old-voice', role: 'user', content: '旧会话语音', timestamp: 2 }, 2);
    expect(history.currentChatHistory).toEqual([]);
    expect(invokeMock.mock.calls.some(([command]) => command === 'handle_agent_chat_message')).toBe(false);
  });


  it('keeps a deep history window while merging corrections and showing the latest-message affordance', async () => {
    const { history, key } = await setup();
    history.currentChatHistory = [{ id: 'anchor', role: 'assistant', content: '旧内容', timestamp: 1 }];
    history.hasEvictedNewer = true;
    history.mergePersistedVoiceMessage(key, { id: 'voice-user', role: 'user', timestamp: 9 }, 9);
    mockInvoke('load_chat_history', () => [
      { id: 'anchor', role: 'assistant', content: '已保存内容', timestamp: 1 },
      { id: 'voice-user', role: 'user', timestamp: 9 },
    ]);
    await history.reconcileVoiceHistory(key);
    expect(history.currentChatHistory.map(message => message.id)).toEqual(['anchor']);
    expect(history.currentChatHistory[0].content).toBe('已保存内容');
    expect(history.hasEvictedNewer).toBe(true);
  });

  it('does not trim the visible oldest row when an automatic voice message fills the window', async () => {
    const { history, key } = await setup();
    history.currentChatHistory = Array.from({ length: MAX_HISTORY_MESSAGES }, (_, index) => ({
      id: 'existing-' + index, role: 'user', timestamp: index,
    }));
    history.mergePersistedVoiceMessage(key, { id: 'new-voice', role: 'user', timestamp: 1000 }, 1000);
    expect(history.currentChatHistory).toHaveLength(MAX_HISTORY_MESSAGES);
    expect(history.currentChatHistory[0].id).toBe('existing-0');
    expect(history.hasEvictedNewer).toBe(true);
  });

  it('reconciles missed user and reply rows by ID without loading older rows or replaying sends', async () => {
    const { history, key } = await setup();
    history.currentChatHistory = [{ id: 'anchor', role: 'assistant', timestamp: 10 }];
    mockInvoke('load_chat_history', () => [
      { id: 'older', role: 'user', timestamp: 1 },
      { id: 'anchor', role: 'assistant', timestamp: 10 },
      { id: 'voice-user', role: 'user', timestamp: 11 },
      { id: 'voice-reply', role: 'assistant', timestamp: 12 },
    ]);
    await history.reconcileVoiceHistory(key);
    await history.reconcileVoiceHistory(key);
    expect(history.currentChatHistory.map(message => message.id)).toEqual(['anchor', 'voice-user', 'voice-reply']);
    expect(invokeMock.mock.calls.some(([name]) => ['append_single_message', 'start_tts_session', 'handle_agent_chat_message'].includes(name))).toBe(false);
  });

  it('keeps an old anchor when the latest DB page no longer overlaps the current window', async () => {
    const { history, key } = await setup();
    history.currentChatHistory = [{ id: 'anchor', role: 'assistant', timestamp: 1 }];
    mockInvoke('load_chat_history', () => Array.from({ length: MAX_HISTORY_MESSAGES }, (_, index) => ({
      id: 'newer-' + index, role: 'user', timestamp: index + 100,
    })));
    await history.reconcileVoiceHistory(key);
    expect(history.currentChatHistory.map(message => message.id)).toEqual(['anchor']);
    expect(history.hasEvictedNewer).toBe(true);
  });

  it('ignores a DB reconciliation response after the visible conversation epoch changes', async () => {
    const { history, session, key } = await setup();
    const loaded = deferred<ChatMessage[]>();
    mockInvoke('load_chat_history', () => loaded.promise);
    const reconciliation = history.reconcileVoiceHistory(key);
    session.setConversation({ id: 'other', type: 'agent' }, 'other-topic');
    history.resetHistoryForConversation();
    loaded.resolve([{ id: 'late-voice', role: 'user', timestamp: 11 }]);
    await reconciliation;
    expect(history.currentChatHistory).toEqual([]);
  });
});

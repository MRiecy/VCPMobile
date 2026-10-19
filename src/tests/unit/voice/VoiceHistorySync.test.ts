import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushPromises } from '@vue/test-utils';
import { createVoiceHistorySync } from '../../../features/chat/voice/voiceHistorySync';
import { emitTauriEvent, invokeMock, mockInvoke } from '../../mocks/tauri';
import type { ConversationKey } from '../../../core/stores/chatSessionStore';

const state = vi.hoisted(() => ({
  current: null as ConversationKey | null,
  merge: vi.fn(), history: vi.fn(), recover: vi.fn(), count: vi.fn(), updated: vi.fn(),
}));
vi.mock('../../../core/stores/chatSessionStore', () => ({
  useChatSessionStore: () => ({
    get currentConversationKey() { return state.current; },
    isConversationCurrent: (key: ConversationKey) => JSON.stringify(state.current) === JSON.stringify(key),
  }),
}));
vi.mock('../../../core/stores/chatHistoryStore', () => ({
  useChatHistoryStore: () => ({ mergePersistedVoiceMessage: state.merge, reconcileVoiceHistory: state.history }),
}));
vi.mock('../../../core/stores/topicListManager', () => ({
  useTopicStore: () => ({ setTopicMsgCount: state.count, setTopicUpdatedAt: state.updated }),
}));
vi.mock('../../../features/chat/voice/voiceCallStore', () => ({
  useVoiceCallStore: () => ({ recover: state.recover }),
}));

const target = { ownerType: 'agent' as const, ownerId: 'agent', topicId: 'topic' };
const message = { id: 'voice-user', role: 'user', content: '已在后台发送', timestamp: 10 };
let sync: ReturnType<typeof createVoiceHistorySync>;

beforeEach(() => {
  state.current = { ...target, epoch: 1 };
  for (const mock of [state.merge, state.history, state.recover, state.count, state.updated]) mock.mockReset();
  state.history.mockResolvedValue(undefined);
  state.recover.mockResolvedValue(null);
  mockInvoke('get_topics', () => [{ id: 'topic', msgCount: 8, updatedAt: 20 }]);
  sync = createVoiceHistorySync();
});
afterEach(() => sync.dispose());

describe('voice committed-message projection', () => {
  it('recovers a background commit through authoritative history without sending or narrating again', async () => {
    state.recover.mockResolvedValue({ target, lastPersistedMessageId: message.id });
    await sync.reconcile();
    expect(state.merge).not.toHaveBeenCalled();
    expect(state.history).toHaveBeenCalledWith(state.current);
    expect(state.count).toHaveBeenCalledWith('agent', 'agent', 'topic', 8);
    expect(invokeMock.mock.calls.map(([command]) => command)).toEqual(['get_topics']);
  });

  it('does not apply a delayed activity result to a new conversation epoch', async () => {
    let resolve!: (rows: unknown[]) => void;
    mockInvoke('get_topics', () => new Promise(yes => { resolve = yes; }));
    await flushPromises();
    emitTauriEvent('vcp-voice-message-persisted', { callId: 'call', target, revision: 1, message, topicUpdatedAt: 10 });
    await flushPromises();
    state.current = { ...target, epoch: 2 };
    resolve([{ id: 'topic', msgCount: 8, updatedAt: 20 }]);
    await flushPromises();
    expect(state.count).not.toHaveBeenCalled();
  });

  it('refreshes an activity query overtaken by another committed message', async () => {
    let resolve!: (rows: unknown[]) => void;
    let queries = 0;
    mockInvoke('get_topics', () => ++queries === 1
      ? new Promise(yes => { resolve = yes; })
      : [{ id: 'topic', msgCount: 9, updatedAt: 30 }]);
    await flushPromises();
    const event = { callId: 'call', target, revision: 1, message, topicUpdatedAt: 10 };
    emitTauriEvent('vcp-voice-message-persisted', event);
    await flushPromises();
    emitTauriEvent('vcp-voice-message-persisted', { ...event, revision: 2, message: { ...message, id: 'next' } });
    resolve([{ id: 'topic', msgCount: 8, updatedAt: 20 }]);
    await flushPromises();
    expect(state.count).toHaveBeenCalledExactlyOnceWith('agent', 'agent', 'topic', 9);
  });

  it('detaches rendering without issuing any cancellation command', async () => {
    await flushPromises();
    sync.dispose();
    emitTauriEvent('vcp-voice-message-persisted', { callId: 'call', target, revision: 1, message, topicUpdatedAt: 10 });
    await sync.reconcile();
    expect(state.merge).not.toHaveBeenCalled();
    expect(state.recover).not.toHaveBeenCalled();
    expect(invokeMock).not.toHaveBeenCalled();
  });
});

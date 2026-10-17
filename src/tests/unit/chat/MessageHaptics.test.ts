import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { useChatHistoryStore } from '@/core/stores/chatHistoryStore';
import { useChatSessionStore } from '@/core/stores/chatSessionStore';
import { useChatStreamStore } from '@/core/stores/chatStreamStore';
import { useSettingsStore } from '@/core/stores/settings';
import { useChatPreferencesStore } from '@/features/chat/preferences/store';
import { invokeMock, mockInvoke } from '@/tests/mocks/tauri';

const context = { ownerId: 'agent-a', ownerType: 'agent' as const, topicId: 'topic-a', agentId: 'agent-a' };

async function finishResponse(messageId: string) {
  const stream = useChatStreamStore();
  const event = {
    consumerRevision: 1, messageId, context, chunk: null, aurora: null,
    finishReason: null, error: null, blocks: null, timestamp: null, topicUpdatedAt: null,
  };
  await stream.processStreamEvent({ ...event, type: 'thinking' });
  await stream.processStreamEvent({ ...event, type: 'end', finishReason: 'completed', blocks: [] });
}

describe('message haptics preference at the send and completion owners', () => {
  beforeEach(() => {
    localStorage.removeItem('vcp-message-haptics-enabled');
    setActivePinia(createPinia());
    const session = useChatSessionStore();
    session.setConversation({ id: 'agent-a', type: 'agent' }, 'topic-a');
    const history = useChatHistoryStore();
    history.loadedConversationKey = session.currentConversationKey;
    useSettingsStore().settings = { vcpServerUrl: 'http://vcp.test', vcpApiKey: 'key' } as any;
    mockInvoke('append_single_message', () => ({ blocks: [], topicUpdatedAt: 1 }));
    mockInvoke('handle_agent_chat_message', () => undefined);
  });

  it('uses one live preference for both events while preserving their existing durations', async () => {
    const history = useChatHistoryStore();
    const preferences = useChatPreferencesStore();
    await history.sendMessage('first');
    await finishResponse('response-1');
    expect(vi.mocked(navigator.vibrate).mock.calls).toEqual([[25], [40]]);

    preferences.setMessageHapticsEnabled(false);
    await history.sendMessage('second');
    await finishResponse('response-2');
    expect(vi.mocked(navigator.vibrate).mock.calls).toEqual([[25], [40]]);
    expect(invokeMock.mock.calls.filter(([command]) => command === 'handle_agent_chat_message')).toHaveLength(2);

    preferences.setMessageHapticsEnabled(true);
    await finishResponse('response-3');
    expect(vi.mocked(navigator.vibrate).mock.calls).toEqual([[25], [40], [40]]);
  });

  it('reads the setting when the response actually finishes, including after async compilation', async () => {
    let resolveBlocks!: (blocks: []) => void;
    mockInvoke('process_message_content', () => new Promise<[]>((resolve) => { resolveBlocks = resolve; }));
    const stream = useChatStreamStore();
    const onStreamFinished = vi.fn();
    await stream.processStreamEvent({
      type: 'end', consumerRevision: 1, messageId: 'late-compilation', context,
      chunk: null, aurora: null, content: 'response', finishReason: 'completed',
      error: null, blocks: null, timestamp: null, topicUpdatedAt: null,
    }, { onStreamFinished });
    expect(onStreamFinished).not.toHaveBeenCalled();
    useChatPreferencesStore().setMessageHapticsEnabled(false);
    resolveBlocks([]);
    await vi.waitFor(() => expect(onStreamFinished).toHaveBeenCalledWith('late-compilation', 'topic-a'));
    expect(navigator.vibrate).not.toHaveBeenCalled();
  });
});

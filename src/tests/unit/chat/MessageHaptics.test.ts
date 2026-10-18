import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { useChatHistoryStore } from '@/core/stores/chatHistoryStore';
import { useChatSessionStore } from '@/core/stores/chatSessionStore';
import { useChatStreamStore } from '@/core/stores/chatStreamStore';
import { useSettingsStore } from '@/core/stores/settings';
import { useChatPreferencesStore } from '@/features/chat/preferences/store';
import { invokeMock, mockInvoke } from '@/tests/mocks/tauri';

const hapticEffects = () => invokeMock.mock.calls
  .filter(([command]) => command === 'plugin:vcp-mobile|trigger_haptic_feedback')
  .map(([, args]) => (args as { payload: { type: string; effect: string } }).payload);

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
    mockInvoke('plugin:vcp-mobile|trigger_haptic_feedback', () => ({ submitted: true }));
  });

  it('uses one live preference for native send and completed-reply intents', async () => {
    const history = useChatHistoryStore();
    const preferences = useChatPreferencesStore();
    await history.sendMessage('first');
    await finishResponse('response-1');
    expect(hapticEffects()).toEqual([{ type: 'message', effect: 'send' }, { type: 'message', effect: 'complete' }]);

    preferences.setMessageHapticsEnabled(false);
    await history.sendMessage('second');
    await finishResponse('response-2');
    expect(hapticEffects()).toHaveLength(2);
    expect(invokeMock.mock.calls.filter(([command]) => command === 'handle_agent_chat_message')).toHaveLength(2);

    preferences.setMessageHapticsEnabled(true);
    await finishResponse('response-3');
    expect(hapticEffects()).toEqual([
      { type: 'message', effect: 'send' },
      { type: 'message', effect: 'complete' },
      { type: 'message', effect: 'complete' },
    ]);
    expect(navigator.vibrate).not.toHaveBeenCalled();
  });

  it('reads the setting when the response actually finishes, including after async compilation', async () => {
    let resolveBlocks!: (blocks: []) => void;
    mockInvoke('process_message_content', () => new Promise<[]>((resolve) => { resolveBlocks = resolve; }));
    const stream = useChatStreamStore();
    const onStreamFinished = vi.fn();
    await stream.processStreamEvent({
      type: 'thinking', consumerRevision: 1, messageId: 'late-compilation', context,
      chunk: null, aurora: null, finishReason: null, error: null, blocks: null,
      timestamp: null, topicUpdatedAt: null,
    });
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
    expect(hapticEffects()).toEqual([]);
  });

  it.each<[string, boolean]>([
    ['cancelled_by_user', false], ['error', false], ['content_filter', false], ['completed', true],
  ])('does not announce %s terminal results with replay=%s', async (finishReason, replayed) => {
    const stream = useChatStreamStore();
    const event = {
      consumerRevision: 1, messageId: 'quiet-result', context: { ...context, replayed },
      chunk: null, aurora: null, finishReason: null, error: null, blocks: [],
      timestamp: null, topicUpdatedAt: null,
    };
    await stream.processStreamEvent({ ...event, type: 'thinking' });
    await stream.processStreamEvent({ ...event, type: 'end', finishReason });
    expect(stream.isMessageActive(context.ownerId, context.ownerType, context.topicId, event.messageId)).toBe(false);
    expect(hapticEffects()).toEqual([]);
  });

  it('does not announce a historical end without a live response or repeat an end', async () => {
    const stream = useChatStreamStore();
    const event = {
      consumerRevision: 1, messageId: 'history-result', context,
      chunk: null, aurora: null, finishReason: 'completed', error: null, blocks: [],
      timestamp: null, topicUpdatedAt: null,
    };
    await stream.processStreamEvent({ ...event, type: 'end' });
    expect(hapticEffects()).toEqual([]);
    await finishResponse('live-result');
    await stream.processStreamEvent({ ...event, messageId: 'live-result', type: 'end' });
    expect(hapticEffects()).toEqual([{ type: 'message', effect: 'complete' }]);
  });

  it('does not delay sending for the optional native feedback result', async () => {
    mockInvoke('plugin:vcp-mobile|trigger_haptic_feedback', () => new Promise(() => {}));
    await useChatHistoryStore().sendMessage('send immediately');
    expect(invokeMock.mock.calls.some(([command]) => command === 'handle_agent_chat_message')).toBe(true);
    expect(hapticEffects()).toEqual([{ type: 'message', effect: 'send' }]);
  });

  it('does not trigger feedback for an empty send', async () => {
    await useChatHistoryStore().sendMessage('   ');
    expect(hapticEffects()).toEqual([]);
  });
});

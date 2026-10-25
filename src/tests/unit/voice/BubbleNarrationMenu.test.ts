import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, disposePinia, setActivePinia, type Pinia } from 'pinia';
import { reactive } from 'vue';
import { flushPromises, mount, type VueWrapper } from '@vue/test-utils';
import MessageRenderer from '../../../features/chat/MessageRenderer.vue';
import { useBubbleNarration } from '../../../features/chat/voice/useBubbleNarration';
import { useChatSessionStore } from '../../../core/stores/chatSessionStore';
import { useChatStreamStore } from '../../../core/stores/chatStreamStore';
import { useChatHistoryStore } from '../../../core/stores/chatHistoryStore';
import { useOverlayStore } from '../../../core/stores/overlay';
import type { ChatMessage } from '../../../core/types/chat';
import { invokeMock, mockInvoke } from '../../mocks/tauri';

const context = vi.hoisted(() => ({ voice: {} as { active: boolean } }));
vi.mock('../../../features/chat/voice/voiceCallStore', () => ({ useVoiceCallStore: () => context.voice }));
vi.mock('pele', () => ({ render: vi.fn() }));
let pinia: Pinia;
const wrappers: VueWrapper[] = [];
const calls = (command: string) => invokeMock.mock.calls.filter(([name]) => name === command);
const message = (id: string, role = 'assistant', timestamp = 1, content: string | undefined = '正文'): ChatMessage => ({
  id, role, timestamp, content, blocks: [], shell: { isUser: role === 'user', displayName: role, avatarColor: '#888' },
});
beforeEach(async () => {
  pinia = createPinia();
  setActivePinia(pinia);
  context.voice = reactive({ active: false });
  useChatSessionStore().setConversation({ id: 'agent', type: 'agent' }, 'topic');
  mockInvoke('start_tts_session', () => ({ configured: true }));
  mockInvoke('reconnect_chat_streams', () => ({ generations: [], activeRequests: [], groupTurns: [] }));
  await useChatStreamStore().checkAndRecoverInterruptedStreams();
});
afterEach(async () => {
  wrappers.splice(0).forEach(wrapper => wrapper.unmount());
  disposePinia(pinia);
  await flushPromises();
  vi.restoreAllMocks();
});
async function render(messages: ChatMessage[], target = messages[0]) {
  mockInvoke('load_chat_history', () => messages);
  const history = useChatHistoryStore();
  await history.loadHistoryPaginated('agent', 'agent', 'topic');
  const wrapper = mount(MessageRenderer, {
    props: { message: target },
    global: { plugins: [pinia],
      directives: { longpress: { mounted: (element, binding) => element.addEventListener('test-menu', binding.value) } },
      stubs: { VcpAvatar: true, AttachmentPreview: true, MermaidFullScreenViewer: true, ThinkingIndicator: true, StreamingTag: true },
    },
  });
  wrappers.push(wrapper);
  await wrapper.trigger('test-menu');
  return { wrapper, history };
}
const action = (label: string) => useOverlayStore().contextMenuConfig!.actions.find(item => item.label === label)!;

describe('bubble narration message menu', () => {
  it('reads the entire message and survives ordinary renderer unmount', async () => {
    const original = '<voice_guidance>轻快</voice_guidance>\n第一段<brk>第二段';
    const { wrapper } = await render([message('a', 'assistant', 1, original)]);
    const startAction = action('朗读气泡');
    expect(startAction.danger).toBeFalsy();
    await startAction.handler();
    expect(calls('submit_tts_text')[0][1]?.text).toBe(original);
    await wrapper.trigger('test-menu');
    const stopAction = action('停止朗读');
    expect(stopAction).toBeDefined();
    expect(stopAction.danger).toBe(true);
    wrappers.splice(wrappers.indexOf(wrapper), 1);
    wrapper.unmount();
    await flushPromises();
    expect(useBubbleNarration().activeMessageId).toBe('a');
    expect(calls('stop_tts')).toHaveLength(0);
  });

  it('does not admit a late raw-text read after conversation change', async () => {
    let resolve!: (text: string) => void;
    mockInvoke('fetch_raw_message_content', () => new Promise<string>(accept => { resolve = accept; }));
    const source = message('a');
    source.content = undefined;
    await render([source]);
    const start = action('朗读气泡').handler();
    await flushPromises();
    useChatSessionStore().setConversation({ id: 'other', type: 'agent' }, 'other-topic');
    resolve('旧消息原文');
    await start;
    expect(calls('start_tts_session')).toHaveLength(0);
  });

  it('old stop actions cannot stop a replacement; cancelling deletion leaves it playing', async () => {
    const a = message('a');
    const b = message('b', 'assistant', 2);
    const { wrapper, history } = await render([a, b]);
    await action('朗读气泡').handler();
    await wrapper.trigger('test-menu');
    const stopA = action('停止朗读');
    await useBubbleNarration().startBubbleNarration(history.captureMessageActionKey('b')!, 'B');
    const count = calls('stop_tts').length;
    await stopA.handler();
    expect(calls('stop_tts')).toHaveLength(count);
    await wrapper.setProps({ message: b });
    await wrapper.trigger('test-menu');
    vi.spyOn(useOverlayStore(), 'showConfirm').mockResolvedValue(false);
    await action('删除消息').handler();
    expect(calls('stop_tts')).toHaveLength(count);
    expect(calls('delete_messages')).toHaveLength(0);
    expect(useBubbleNarration().activeMessageId).toBe('b');
  });

  it('does not expose narration while the message is generating', async () => {
    useChatStreamStore().addSessionStream('agent', 'agent', 'topic', 'a');
    await render([message('a')]);
    expect(action('朗读气泡')).toBeUndefined();
  });

  it('does not expose narration for user messages', async () => {
    await render([message('user-1', 'user', 1, '用户发送的消息')]);
    expect(useOverlayStore().contextMenuConfig!.actions.find(item => item.label === '朗读气泡')).toBeUndefined();
  });

  it('regeneration stops a later affected message, even when it is not the clicked message', async () => {
    const messages = [message('user', 'user', 1), message('a', 'assistant', 2), message('b', 'assistant', 3)];
    const { history } = await render(messages, messages[1]);
    const owner = useBubbleNarration();
    await owner.startBubbleNarration(history.captureMessageActionKey('b')!, 'B');
    mockInvoke('regenerate_topic_response', () => ({ generation: {}, msgCount: 1 }));
    await history.regenerateResponse(history.captureMessageActionKey('a')!);
    expect(calls('stop_tts')).toHaveLength(1);
    expect(calls('regenerate_topic_response')).toHaveLength(1);
    expect(invokeMock.mock.calls.findIndex(([name]) => name === 'stop_tts')).toBeLessThan(
      invokeMock.mock.calls.findIndex(([name]) => name === 'regenerate_topic_response'),
    );
    expect(owner.activeMessageId).toBeNull();
  });
});

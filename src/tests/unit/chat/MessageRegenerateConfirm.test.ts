import { mount } from '@vue/test-utils';
import { createPinia, setActivePinia } from 'pinia';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import MessageRenderer from '@/features/chat/MessageRenderer.vue';
import type { ChatMessage } from '@/core/types/chat';
import { useChatHistoryStore } from '@/core/stores/chatHistoryStore';
import { useChatSessionStore } from '@/core/stores/chatSessionStore';
import { useChatStreamStore } from '@/core/stores/chatStreamStore';
import { useOverlayStore } from '@/core/stores/overlay';
import { invokeMock, mockInvoke } from '@/tests/mocks/tauri';

describe('MessageRenderer - 重新生成确认提示框', () => {
  let pinia: ReturnType<typeof createPinia>;

  beforeEach(async () => {
    pinia = createPinia();
    setActivePinia(pinia);
    mockInvoke('reconnect_chat_streams', () => ({ generations: [], groupTurns: [], activeRequests: [] }));
    await useChatStreamStore().checkAndRecoverInterruptedStreams();
  });

  it('长按消息菜单点击重新生成时弹出确认提示框，取消时不触发重新生成，确认时触发重新生成', async () => {
    const sessionStore = useChatSessionStore();
    sessionStore.setConversation({ id: 'agent-1', type: 'agent' }, 'topic-1');

    const historyStore = useChatHistoryStore();
    historyStore.loadedConversationKey = sessionStore.currentConversationKey;
    const overlayStore = useOverlayStore();

    const testMessage: ChatMessage = {
      id: 'msg-assistant-1',
      role: 'assistant',
      name: 'Agent',
      content: 'Hello from assistant',
      timestamp: Date.now(),
      topicId: 'topic-1',
      blocks: [],
      shell: {
        isUser: false,
        displayName: 'Agent',
        avatarColor: '#10a37f',
      },
    };

    historyStore.currentChatHistory = [testMessage];

    let longpressHandler: (() => Promise<void>) | null = null;

    mount(MessageRenderer, {
      props: { message: testMessage },
      global: {
        plugins: [pinia],
        directives: {
          longpress: {
            mounted(_el, binding) {
              longpressHandler = binding.value;
            },
          },
        },
        stubs: {
          VcpAvatar: true,
          ToolBlock: true,
          ThoughtBlock: true,
          HtmlPreviewBlock: true,
          ToolSummaryBlock: true,
          DiaryBlock: true,
          AttachmentPreview: true,
          MermaidFullScreenViewer: true,
          ThinkingIndicator: true,
          StreamingTag: true,
        },
      },
    });

    const openContextMenuSpy = vi.spyOn(overlayStore, 'openContextMenu');
    const showConfirmSpy = vi.spyOn(overlayStore, 'showConfirm');
    const regenerateSpy = vi.spyOn(historyStore, 'regenerateResponse').mockResolvedValue();

    // 触发长按打开上下文菜单
    await longpressHandler!();

    expect(openContextMenuSpy).toHaveBeenCalled();
    const [actions] = openContextMenuSpy.mock.calls[0];
    const regenerateAction = actions.find((a: any) => a.label === '重新生成');
    expect(regenerateAction).toBeDefined();

    // 1. 用户取消确认
    showConfirmSpy.mockResolvedValueOnce(false);
    await regenerateAction!.handler();

    expect(regenerateSpy).not.toHaveBeenCalled();

    // 2. 用户点击确认
    showConfirmSpy.mockResolvedValueOnce(true);
    await regenerateAction!.handler();

    expect(regenerateSpy).toHaveBeenCalledWith(
      expect.objectContaining({
        messageId: 'msg-assistant-1',
      }),
    );
  });

  it('用户消息只提供编辑重发，并且取消确认时不重发', async () => {
    const sessionStore = useChatSessionStore();
    sessionStore.setConversation({ id: 'agent-1', type: 'agent' }, 'topic-1');

    const historyStore = useChatHistoryStore();
    historyStore.loadedConversationKey = sessionStore.currentConversationKey;
    const overlayStore = useOverlayStore();

    const userMessage: ChatMessage = {
      id: 'msg-user-1',
      role: 'user',
      name: 'User',
      content: 'Hello from user',
      timestamp: Date.now(),
      topicId: 'topic-1',
      blocks: [],
      shell: {
        isUser: true,
        displayName: 'User',
        avatarColor: '#3b82f6',
      },
    };

    historyStore.currentChatHistory = [userMessage];

    let longpressHandler: (() => Promise<void>) | null = null;

    mount(MessageRenderer, {
      props: { message: userMessage },
      global: {
        plugins: [pinia],
        directives: {
          longpress: {
            mounted(_el, binding) {
              longpressHandler = binding.value;
            },
          },
        },
        stubs: {
          VcpAvatar: true,
          ToolBlock: true,
          ThoughtBlock: true,
          HtmlPreviewBlock: true,
          ToolSummaryBlock: true,
          DiaryBlock: true,
          AttachmentPreview: true,
          MermaidFullScreenViewer: true,
          ThinkingIndicator: true,
          StreamingTag: true,
        },
      },
    });

    const openContextMenuSpy = vi.spyOn(overlayStore, 'openContextMenu');
    const showConfirmSpy = vi.spyOn(overlayStore, 'showConfirm');
    const editResendSpy = vi.spyOn(historyStore, 'beginEditResend').mockResolvedValue();

    await longpressHandler!();

    expect(openContextMenuSpy).toHaveBeenCalled();
    const [actions] = openContextMenuSpy.mock.calls[0];
    expect(actions.find((a: any) => a.label === '重新生成')).toBeUndefined();

    const editResendAction = actions.find((a: any) => a.label === '编辑重发');
    expect(editResendAction).toBeDefined();

    // 1. 取消确认
    showConfirmSpy.mockResolvedValueOnce(false);
    await editResendAction!.handler();

    expect(editResendSpy).not.toHaveBeenCalled();

    // 2. 确认
    showConfirmSpy.mockResolvedValueOnce(true);
    await editResendAction!.handler();

    expect(editResendSpy).toHaveBeenCalledWith(
      expect.objectContaining({
        messageId: 'msg-user-1',
      }),
    );
  });

  it('菜单打开后开始生成时实时禁用删除，已打开的确认框也不能删除消息', async () => {
    const session = useChatSessionStore();
    session.setConversation({ id: 'agent-1', type: 'agent' }, 'topic-1');
    const history = useChatHistoryStore();
    history.loadedConversationKey = session.currentConversationKey;
    const message: ChatMessage = {
      id: 'old', role: 'assistant', content: 'saved', timestamp: 1, blocks: [],
      shell: { isUser: false, displayName: 'Agent', avatarColor: '' },
    };
    history.currentChatHistory = [message];
    let openMenu!: () => Promise<void>;
    const wrapper = mount(MessageRenderer, {
      props: { message },
      global: {
        plugins: [pinia],
        directives: { longpress: { mounted(_el, binding) { openMenu = binding.value; } } },
        stubs: { VcpAvatar: true, AttachmentPreview: true, MermaidFullScreenViewer: true, StreamingTag: true },
      },
    });
    await openMenu();
    const overlay = useOverlayStore();
    const remove = overlay.contextMenuConfig!.actions.find(action => action.label === '删除消息')!;
    expect(remove.disabled).toBe(false);
    let confirm!: (confirmed: boolean) => void;
    vi.spyOn(overlay, 'showConfirm').mockReturnValue(new Promise(resolve => { confirm = resolve; }));
    const deleting = remove.handler();
    const release = useChatStreamStore().beginConversationOperation('agent-1', 'agent', 'topic-1');
    expect(remove.disabled).toBe(true);
    confirm(true);
    await deleting;
    expect(invokeMock).not.toHaveBeenCalledWith('delete_messages', expect.anything());
    expect(history.currentChatHistory[0].id).toBe('old');
    release();
    expect(remove.disabled).toBe(false);
    wrapper.unmount();
  });

  it.each([
    { state: { streamRecoveryState: 'waiting_retry' as const }, label: '接续失败，等待重试' },
    { state: { isStopping: true }, label: '停止待确认' },
  ])('空正文等待状态只展示 $label，不继续显示正在思考', ({ state, label }) => {
    const session = useChatSessionStore();
    session.setConversation({ id: 'agent-1', type: 'agent' }, 'topic-1');
    useChatStreamStore().addSessionStream('agent-1', 'agent', 'topic-1', 'pending');
    const wrapper = mount(MessageRenderer, {
      props: { message: {
        id: 'pending', role: 'assistant', content: '', blocks: [], timestamp: 1,
        shell: { isUser: false, displayName: 'Agent', avatarColor: '' }, ...state,
      } },
      global: {
        plugins: [pinia], directives: { longpress: {} },
        stubs: { VcpAvatar: true, ThinkingIndicator: true, StreamingTag: true, MermaidFullScreenViewer: true },
      },
    });
    expect(wrapper.text()).toContain(label);
    expect(wrapper.find('thinking-indicator-stub').exists()).toBe(false);
    expect(wrapper.find('streaming-tag-stub').exists()).toBe(false);
    wrapper.unmount();
  });
});

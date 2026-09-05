import { mount } from '@vue/test-utils';
import { createPinia, setActivePinia } from 'pinia';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import MessageRenderer from '@/features/chat/MessageRenderer.vue';
import type { ChatMessage } from '@/core/types/chat';
import { useChatHistoryStore } from '@/core/stores/chatHistoryStore';
import { useChatSessionStore } from '@/core/stores/chatSessionStore';
import { useOverlayStore } from '@/core/stores/overlay';
import { mockInvoke } from '@/tests/mocks/tauri';

describe('MessageRenderer - 重新生成确认提示框', () => {
  let pinia: ReturnType<typeof createPinia>;

  beforeEach(() => {
    pinia = createPinia();
    setActivePinia(pinia);
    mockInvoke('get_active_generations', () => []);
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

    expect(longpressHandler).toBeTypeOf('function');

    const openContextMenuSpy = vi.spyOn(overlayStore, 'openContextMenu');
    const showConfirmSpy = vi.spyOn(overlayStore, 'showConfirm');
    const regenerateSpy = vi.spyOn(historyStore, 'regenerateResponse').mockResolvedValue();

    // 触发长按打开上下文菜单
    await longpressHandler!();

    expect(openContextMenuSpy).toHaveBeenCalled();
    const [actions] = openContextMenuSpy.mock.calls[0];
    const regenerateAction = actions.find((a: any) => a.label === '重新生成');
    expect(regenerateAction).toBeDefined();
    expect(regenerateAction!.danger).toBe(true);

    // 1. 用户取消确认
    showConfirmSpy.mockResolvedValueOnce(false);
    await regenerateAction!.handler();

    expect(showConfirmSpy).toHaveBeenCalledWith({
      title: '重新生成',
      message: '确定要重新生成这条消息吗？',
      isDanger: true,
    });
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

  it('用户消息长按菜单中不应包含“重新生成”，而应包含红色样式的“编辑重发”且带确认提示框', async () => {
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
    expect(editResendAction!.danger).toBe(true);

    // 1. 取消确认
    showConfirmSpy.mockResolvedValueOnce(false);
    await editResendAction!.handler();

    expect(showConfirmSpy).toHaveBeenCalledWith({
      title: '编辑重发',
      message: '确定要编辑重发这条消息吗？',
      isDanger: true,
    });
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
});

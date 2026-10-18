import { mount } from '@vue/test-utils';
import { createPinia, setActivePinia } from 'pinia';
import { nextTick } from 'vue';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import MessageRenderer from '@/features/chat/MessageRenderer.vue';
import type { ChatMessage, MarkdownNode } from '@/core/types/chat';
import { vLongpress } from '@/core/directives/longpress';
import { useChatHistoryStore } from '@/core/stores/chatHistoryStore';
import { useChatSessionStore } from '@/core/stores/chatSessionStore';
import { useChatStreamStore } from '@/core/stores/chatStreamStore';
import { useOverlayStore } from '@/core/stores/overlay';

describe('rendered message long press', () => {
  beforeEach(() => { vi.useFakeTimers(); });
  afterEach(() => { vi.useRealTimers(); });

  it.each(['stable', 'fallback', 'AST tail'] as const)('opens once from %s text when replaying the Android compatibility sequence', async (mode) => {
    const pinia = createPinia();
    setActivePinia(pinia);
    const session = useChatSessionStore();
    session.setConversation({ id: 'agent-press', type: 'agent' }, 'topic-press');
    const nodes: MarkdownNode[] = [{ type: 'paragraph', children: [{ type: 'text', value: '消息正文' }] }];
    const message: ChatMessage = {
      id: 'message-press', role: 'assistant', timestamp: 1, content: '消息正文',
      topicId: 'topic-press', agentId: 'agent-press',
      shell: { isUser: false, displayName: 'Agent', avatarColor: '#64748b' },
      blocks: mode === 'stable' ? [{ type: 'markdown', hash: 'body', nodes }] : [],
    };
    if (mode === 'AST tail') {
      useChatStreamStore().addSessionStream('agent-press', 'agent', 'topic-press', message.id);
      message.tailBlock = { type: 'markdown', content: '消息正文', hash: 'tail', render_mode: 'ast', nodes };
    }
    const history = useChatHistoryStore();
    history.loadedConversationKey = session.currentConversationKey;
    history.currentChatHistory = [message];
    const openMenu = vi.spyOn(useOverlayStore(), 'openContextMenu');
    const wrapper = mount(MessageRenderer, {
      attachTo: document.body,
      props: { message },
      global: {
        plugins: [pinia], directives: { longpress: vLongpress },
        stubs: { VcpAvatar: true, AttachmentPreview: true, MermaidFullScreenViewer: true,
          ThinkingIndicator: true, StreamingTag: true },
      },
    });
    try {
      await nextTick();
      await nextTick();
      if (mode === 'AST tail') expect(wrapper.get('.vcp-ast-sandbox').text()).toContain('消息正文');
      const prose = wrapper.get('p').element;
      const touchIdentity: PointerEventInit = {
        bubbles: true, cancelable: true, pointerId: 17, pointerType: 'touch', isPrimary: true,
        clientX: 40, clientY: 60,
      };
      prose.dispatchEvent(new PointerEvent('pointerdown', { ...touchIdentity, button: 0, buttons: 1 }));
      const contact = new Touch({ identifier: 17, target: prose, clientX: 40, clientY: 60 });
      prose.dispatchEvent(new TouchEvent('touchstart', {
        bubbles: true, cancelable: true, touches: [contact], targetTouches: [contact], changedTouches: [contact],
      }));
      // Synthetic replay of captured Android timings, through the real renderer
      // and menu store. It does not stand in for another physical-device run.
      vi.advanceTimersByTime(492);
      prose.dispatchEvent(new MouseEvent('mousemove', {
        bubbles: true, cancelable: true, clientX: 40, clientY: 60, buttons: 0,
      }));
      vi.advanceTimersByTime(2);
      prose.dispatchEvent(new PointerEvent('contextmenu', { ...touchIdentity, button: 2, buttons: 0 }));
      vi.advanceTimersByTime(106);
      expect(openMenu).toHaveBeenCalledTimes(1);
      expect(openMenu.mock.calls[0][0].some(action => action.label.includes('复制'))).toBe(true);
      vi.advanceTimersByTime(441);
      prose.dispatchEvent(new PointerEvent('pointerup', { ...touchIdentity, button: 0, buttons: 0 }));
      prose.dispatchEvent(new TouchEvent('touchend', {
        bubbles: true, cancelable: true, touches: [], targetTouches: [], changedTouches: [contact],
      }));
      expect(openMenu).toHaveBeenCalledTimes(1);
    } finally {
      wrapper.unmount();
    }
  });
});

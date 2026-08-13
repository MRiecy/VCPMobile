import { defineComponent, nextTick, ref } from 'vue';
import { mount } from '@vue/test-utils';
import { createPinia, setActivePinia } from 'pinia';
import { describe, expect, it } from 'vitest';
import MessageRenderer from '@/features/chat/MessageRenderer.vue';
import type { ChatMessage } from '@/core/types/chat';
import type { ChatPresentationMode } from '@/core/stores/theme';

const markerStub = (marker: string) => defineComponent({
  template: `<div data-strong-block="${marker}">${marker}</div>`,
});

describe('MessageRenderer presentation shell', () => {
  it('keeps the same multi-bubble and strong-content tree across all three modes', async () => {
    const pinia = createPinia();
    setActivePinia(pinia);
    const mode = ref<ChatPresentationMode>('bubble');
    const message: ChatMessage = {
      id: 'presentation-fixture',
      role: 'assistant',
      timestamp: 1_723_456_789_000,
      agentId: 'agent-fixture',
      shell: {
        avatarColor: '#64748b',
        bubbleBorderColor: '#64748b',
        bubbleBoxShadow: 'none',
        displayName: 'Fixture Agent',
        isUser: false,
      },
      blocks: [
        {
          type: 'markdown',
          nodes: [
            { type: 'paragraph', children: [{ type: 'text', value: 'first section' }] },
            { type: 'raw_html', content: '<!--brk-->' },
            { type: 'paragraph', children: [{ type: 'text', value: 'second section' }] },
          ],
        },
        { type: 'tool-use', content: '{"query":"fixture"}', tool_name: 'Search' },
        { type: 'thought', content: 'reasoning fixture' },
        { type: 'html-preview', content: '<main>preview fixture</main>' },
        { type: 'tool-call-summary', items: [{ tool_name: 'Search', status: 'success' }] },
        { type: 'diary', content: 'diary fixture' },
      ],
      attachments: [{
        id: 'attachment-fixture',
        type: 'text/plain',
        name: 'fixture.txt',
        size: 7,
        src: '/fixture.txt',
      }],
    };

    const Host = defineComponent({
      components: { MessageRenderer },
      setup: () => ({ message, mode }),
      template: `
        <main class="chat-view-container" :data-presentation-mode="mode">
          <MessageRenderer :message="message" />
        </main>
      `,
    });

    const wrapper = mount(Host, {
      global: {
        plugins: [pinia],
        directives: { longpress: {} },
        stubs: {
          VcpAvatar: markerStub('avatar'),
          ToolBlock: markerStub('tool'),
          ThoughtBlock: markerStub('thought'),
          HtmlPreviewBlock: markerStub('html-preview'),
          ToolSummaryBlock: markerStub('tool-summary'),
          DiaryBlock: markerStub('diary'),
          AttachmentPreview: markerStub('attachment'),
          MermaidFullScreenViewer: markerStub('mermaid-viewer'),
          ThinkingIndicator: markerStub('thinking'),
          StreamingTag: markerStub('streaming'),
        },
      },
    });
    await nextTick();

    const rendererElement = wrapper.get('[data-message-id="presentation-fixture"]').element;
    const expectedStrongBlocks = ['tool', 'thought', 'html-preview', 'tool-summary', 'diary', 'attachment'];
    const assertStableFixture = (expectedMode: ChatPresentationMode) => {
      expect(wrapper.get('.chat-view-container').attributes('data-presentation-mode')).toBe(expectedMode);
      expect(wrapper.get('[data-message-id="presentation-fixture"]').element).toBe(rendererElement);
      expect(wrapper.findAll('.vcp-chat-bubble')).toHaveLength(2);
      expect(wrapper.findAll('.vcp-message-header')).toHaveLength(2);
      for (const marker of expectedStrongBlocks) {
        expect(wrapper.findAll(`[data-strong-block="${marker}"]`)).toHaveLength(1);
      }
      expect(wrapper.text()).toContain('first section');
      expect(wrapper.text()).toContain('second section');
    };

    assertStableFixture('bubble');
    mode.value = 'panel';
    await nextTick();
    assertStableFixture('panel');
    mode.value = 'immersive';
    await nextTick();
    assertStableFixture('immersive');

    wrapper.unmount();
  });
});

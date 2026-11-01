import { defineComponent, h, ref, type Component } from 'vue';
import { flushPromises, mount, type VueWrapper } from '@vue/test-utils';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { useMessageEvents } from '@/core/composables/useMessageEvents';
import ToolBlock from '@/features/chat/blocks/ToolBlock.vue';
import ThoughtBlock from '@/features/chat/blocks/ThoughtBlock.vue';
import DiaryBlock from '@/features/chat/blocks/DiaryBlock.vue';
import type { ContentBlock, MarkdownNode } from '@/core/types/chat';

const { send, findImage, openExternal, scrollToMessage } = vi.hoisted(() => ({
  send: vi.fn<(content: string) => Promise<void>>(async () => {}),
  findImage: vi.fn(async () => null),
  openExternal: vi.fn(async () => {}),
  scrollToMessage: vi.fn(),
}));
vi.mock('@/core/stores/chatHistoryStore', () => ({ useChatHistoryStore: () => ({ sendMessage: send, openLoadedReference: scrollToMessage }) }));
vi.mock('@/core/stores/notification', () => ({ useNotificationStore: () => ({ addNotification: vi.fn() }) }));
vi.mock('@/core/utils/renderedImage', () => ({ findRenderedImagePayload: findImage }));
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: openExternal }));
vi.mock('@/core/utils/scrollToMessage', () => ({ scrollToMessageById: scrollToMessage }));
vi.mock('@/core/composables/useModalHistory', () => ({
  useModalHistory: () => ({ registerModal: vi.fn(), unregisterModal: vi.fn() }),
}));

const wrappers: VueWrapper[] = [];
afterEach(() => {
  for (const wrapper of wrappers.splice(0)) wrapper.unmount();
});

function mountEvents(renderContent: () => ReturnType<typeof h>) {
  const openMermaid = vi.fn();
  const wrapper = mount(defineComponent({
    setup() {
      const root = ref<HTMLElement | null>(null);
      useMessageEvents(root, openMermaid);
      return () => h('div', { ref: root }, [renderContent()]);
    },
  }));
  wrappers.push(wrapper);
  return { wrapper, openMermaid };
}

function mountBlock(component: Component, props: Record<string, unknown>) {
  return mountEvents(() => h('div', { 'data-message-render-region': 'stable' }, [h(component, props)]));
}

function rawHtml(content: string): MarkdownNode[] {
  return [{ type: 'raw_html', content }];
}

describe('message content event ownership', () => {
  it('keeps Tool copy and expansion controls local while its generated content remains interactive', async () => {
    const value = '<button data-send="generated tool action">continue</button>';
    const block: ContentBlock = {
      type: 'tool-result', tool_name: 'Example', status: 'success',
      details: [{ key: 'result', value }], footer: '<a href="https://example.com/footer">footer</a>',
    };
    const { wrapper, openMermaid } = mountBlock(ToolBlock, { type: 'tool-result', block, defaultExpanded: true });
    const copy = wrapper.get('.tool-content-scrollable button');
    await copy.get('svg').trigger('click');
    await flushPromises();
    expect(navigator.clipboard.writeText).toHaveBeenCalledWith(value);
    expect(copy.attributes('disabled')).toBeUndefined();
    expect(send).not.toHaveBeenCalled();
    expect(findImage).not.toHaveBeenCalled();

    const headerArrow = wrapper.get('.tool-header-content > .flex:last-child > svg');
    await headerArrow.trigger('click');
    expect(wrapper.get('.tool-content-scrollable').attributes('style')).toContain('display: none');
    await wrapper.get('.tool-header-content > .flex:last-child > svg').trigger('click');
    expect(wrapper.get('.tool-content-scrollable').attributes('style') || '').not.toContain('display: none');
    expect(findImage).not.toHaveBeenCalled();

    await wrapper.get('[data-message-content] button').trigger('click');
    await wrapper.get('[data-message-content] a').trigger('click');
    expect(send).toHaveBeenCalledExactlyOnceWith('[[点击按钮:generated tool action]]');
    expect(openExternal).toHaveBeenCalledExactlyOnceWith('https://example.com/footer');
    expect(openMermaid).not.toHaveBeenCalled();
  });

  it('keeps the Thought header SVG local and delegates only its generated body', async () => {
    const block: ContentBlock = {
      type: 'thought', theme: 'Thought', content: '', is_complete: true,
      nodes: rawHtml('<button data-send="thought action">act</button>'),
    };
    const { wrapper } = mountBlock(ThoughtBlock, { block, messageId: 'thought-events', defaultExpanded: false });
    await wrapper.get('.vcp-thought-header svg').trigger('click');
    expect(wrapper.get('.vcp-thought-content').attributes('style') || '').not.toContain('display: none');
    expect(send).not.toHaveBeenCalled();
    expect(findImage).not.toHaveBeenCalled();
    await wrapper.get('.thought-body button').trigger('click');
    expect(send).toHaveBeenCalledExactlyOnceWith('[[点击按钮:thought action]]');
  });

  it.each(['diary', 'diary-update'] as const)('retains generated actions in %s bodies', async type => {
    const block: ContentBlock = type === 'diary'
      ? { type, maid: 'Agent', date: '2026-10-18', content: '', nodes: rawHtml('<button data-send="diary">act</button>') }
      : {
          type, maid: 'Agent', target: 'before', replace: 'after',
          target_nodes: rawHtml('<button data-send="before">before</button>'),
          replace_nodes: rawHtml('<button data-send="after">after</button>'),
        };
    const { wrapper } = mountBlock(DiaryBlock, { block, messageId: `diary-events-${type}` });
    for (const button of wrapper.findAll('[data-message-content] button')) await button.trigger('click');
    expect(send.mock.calls.map(call => call[0])).toEqual(type === 'diary'
      ? ['[[点击按钮:diary]]']
      : ['[[点击按钮:before]]', '[[点击按钮:after]]']);
  });

  it('retains content buttons, links, references and images while excluding attachment and emoticon previews', async () => {
    const { wrapper, openMermaid } = mountEvents(() => h('div', {
      'data-message-content': '',
      innerHTML: '<button data-send="hello"><span>send</span></button><span data-vcp-button="vcp">vcp</span>'
        + '<a href="https://example.com/external">external</a><a href="#msg-target">reference</a>'
        + '<img data-image src="image.png"><svg data-image><path /></svg>'
        + '<img class="vcp-emoticon"><div class="vcp-attachment-preview"><img></div>',
    }));
    await wrapper.get('button span').trigger('click');
    await wrapper.get('[data-vcp-button]').trigger('click');
    await wrapper.get('a[href^="https:"]').trigger('click');
    await wrapper.get('a[href^="#msg-"]').trigger('click');
    await wrapper.get('img[data-image]').trigger('click');
    await wrapper.get('svg path').trigger('click');
    await wrapper.get('.vcp-emoticon').trigger('click');
    await wrapper.get('.vcp-attachment-preview img').trigger('click');
    expect(send.mock.calls.map(call => call[0])).toEqual(['[[点击按钮:hello]]', 'vcp']);
    expect(openExternal).toHaveBeenCalledExactlyOnceWith('https://example.com/external');
    expect(scrollToMessage).toHaveBeenCalledExactlyOnceWith('target');
    expect(findImage).toHaveBeenCalledTimes(2);
    expect(openMermaid).not.toHaveBeenCalled();
  });

  it('claims Mermaid pending and ready content before button, link and image routes', async () => {
    const { wrapper, openMermaid } = mountEvents(() => h('div', {
      'data-message-render-region': 'stable',
    }, [h('div', {
      'data-message-content': '',
      innerHTML: '<div data-content-kind="mermaid" data-mermaid-source="graph TD; A-->B"><button data-send="wrong">pending</button></div>',
    })]));
    await wrapper.get('button').trigger('click');
    expect(openMermaid).not.toHaveBeenCalled();
    wrapper.get('[data-content-kind]').element.innerHTML = '<svg><path/><foreignObject><a href="https://example.com"><button data-send="wrong"><span>label</span></button></a></foreignObject></svg>';
    await wrapper.get('path').trigger('click');
    await wrapper.get('span').trigger('click');
    expect(openMermaid).toHaveBeenCalledTimes(2);
    expect(openMermaid).toHaveBeenLastCalledWith(expect.stringContaining('<svg'), 'graph TD; A-->B');
    expect(send).not.toHaveBeenCalled();
    expect(openExternal).not.toHaveBeenCalled();
    expect(findImage).not.toHaveBeenCalled();
  });

  it('does not claim an application ancestor around a generated content host', async () => {
    const localClick = vi.fn();
    const { wrapper, openMermaid } = mountEvents(() => h('button', {
      'data-send': 'application control', onClick: localClick,
    }, [h('span', { 'data-message-content': '' }, 'content text')]));
    await wrapper.get('span').trigger('click');
    expect(localClick).toHaveBeenCalledOnce();
    expect(wrapper.get('button').attributes('disabled')).toBeUndefined();
    expect(send).not.toHaveBeenCalled();
    expect(findImage).not.toHaveBeenCalled();
    expect(openMermaid).not.toHaveBeenCalled();
  });

  it('keeps Tool fullscreen outside the message delegation boundary', async () => {
    const block: ContentBlock = {
      type: 'tool-result', tool_name: 'Example', status: 'success',
      details: [{ key: 'result', value: '<button data-send="fullscreen action">act</button>' }], footer: '',
    };
    const { wrapper } = mountBlock(ToolBlock, { type: 'tool-result', block, defaultExpanded: true });
    await wrapper.get('.tool-header-content button').trigger('click');
    const fullscreen = document.body.querySelector('.vcp-fullscreen-tool-panel');
    expect(fullscreen).not.toBeNull();
    fullscreen!.querySelector('[data-send]')!.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    expect(send).not.toHaveBeenCalled();
    expect(findImage).not.toHaveBeenCalled();
  });
});

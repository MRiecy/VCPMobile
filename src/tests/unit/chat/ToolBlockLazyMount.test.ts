import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { defineComponent, h, nextTick, ref } from 'vue';
import { flushPromises, mount, type VueWrapper } from '@vue/test-utils';
import { marked } from 'marked';
import ToolBlock from '@/features/chat/blocks/ToolBlock.vue';
import MessageRenderer from '@/features/chat/MessageRenderer.vue';
import type { ChatMessage, ContentBlock } from '@/core/types/chat';

const { renderDiagram } = vi.hoisted(() => ({
  renderDiagram: vi.fn(async () => '<svg viewBox="0 0 10 10"><text>rendered diagram</text></svg>'),
}));
vi.mock('@/features/chat/utils/mermaidRenderer', () => ({
  renderMermaidTemplate: renderDiagram,
  instantiateMermaidSvg: (svg: string) => svg,
}));

const result = (hash: string | number = 'stream'): ContentBlock => ({
  type: 'tool-result', tool_name: 'Search', status: 'success', hash,
  details: [{ key: 'answer', value: '**Evidence**\n\nA tool result.' }], footer: 'Result footer.',
});

let wrapper: VueWrapper | undefined;
beforeEach(() => setActivePinia(createPinia()));
afterEach(() => wrapper?.unmount());

describe('ToolBlock lazy content mounting', () => {
  it('does not parse or mount a collapsed result until its first expansion, then retains its DOM', async () => {
    const parse = vi.spyOn(marked, 'parse');
    const onContentMounted = vi.fn(() => wrapper?.find('.vcp-markdown-block strong').exists());
    wrapper = mount(ToolBlock, { props: { type: 'tool-result', block: result(), onContentMounted } });
    expect(parse).not.toHaveBeenCalled();
    expect(onContentMounted).not.toHaveBeenCalled();
    expect(wrapper.find('.vcp-markdown-block').exists()).toBe(false);

    await wrapper.get('.tool-header-content').trigger('click');
    expect(parse).toHaveBeenCalledTimes(2);
    expect(onContentMounted).toHaveReturnedWith(true);
    const rendered = wrapper.findAll('.vcp-markdown-block').map(node => node.element);
    expect(rendered[0].querySelector('strong')?.textContent).toBe('Evidence');
    await wrapper.get('.tool-header-content').trigger('click');
    expect(wrapper.get<HTMLElement>('.tool-content-scrollable').element.style.display).toBe('none');
    expect(wrapper.findAll('.vcp-markdown-block').map(node => node.element)).toEqual(rendered);
    await wrapper.get('.tool-header-content').trigger('click');
    expect(wrapper.findAll('.vcp-markdown-block').map(node => node.element)).toEqual(rendered);
    expect(parse).toHaveBeenCalledTimes(2);
    expect(onContentMounted).toHaveBeenCalledTimes(1);
  });

  it('mounts on the default expansion signal and preserves the user\'s later collapse', async () => {
    const parse = vi.spyOn(marked, 'parse');
    wrapper = mount(ToolBlock, { props: { type: 'tool-result', block: result(), defaultExpanded: false } });
    await wrapper.setProps({ defaultExpanded: true });
    expect(parse).toHaveBeenCalledTimes(2);
    const rendered = wrapper.get('.vcp-markdown-block').element;
    await wrapper.get('.tool-header-content').trigger('click');
    await wrapper.setProps({ defaultExpanded: false });
    await wrapper.setProps({ defaultExpanded: true });
    expect(wrapper.get<HTMLElement>('.tool-content-scrollable').element.style.display).toBe('none');
    expect(wrapper.get('.vcp-markdown-block').element).toBe(rendered);
    expect(parse).toHaveBeenCalledTimes(2);
  });

  it('inherits mounted content for a reused tool key and resets it for a new identity', async () => {
    const parse = vi.spyOn(marked, 'parse');
    const identity = ref('first-tool');
    const block = ref(result());
    wrapper = mount(defineComponent({
      setup: () => () => h(ToolBlock, { key: identity.value, type: 'tool-result', block: block.value }),
    }));
    await wrapper.get('.tool-header-content').trigger('click');
    const rendered = wrapper.get('.vcp-markdown-block').element;
    block.value = result(101);
    await nextTick();
    expect(wrapper.get('.vcp-markdown-block').element).toBe(rendered);
    expect(parse).toHaveBeenCalledTimes(2);

    identity.value = 'replacement-tool';
    await nextTick();
    expect(wrapper.find('.vcp-markdown-block').exists()).toBe(false);
    expect(parse).toHaveBeenCalledTimes(2);
    await wrapper.get('.tool-header-content').trigger('click');
    expect(wrapper.get('.vcp-markdown-block').element).not.toBe(rendered);
    expect(parse).toHaveBeenCalledTimes(4);
  });

  it('hands the newly mounted body to the message renderer for rich-content rendering', async () => {
    const block: ContentBlock = {
      type: 'tool-result', tool_name: 'Diagram', status: 'success', footer: '',
      details: [{ key: 'diagram', value: '<pre class="mermaid">flowchart LR\nA --> B</pre>' }],
    };
    const message: ChatMessage = {
      id: 'lazy-tool-diagram', role: 'assistant', timestamp: 1, blocks: [block],
      shell: { avatarColor: '#64748b', displayName: 'Assistant', isUser: false },
    };
    wrapper = mount(MessageRenderer, {
      props: { message },
      global: { directives: { longpress: {} }, stubs: { VcpAvatar: true } },
    });
    await flushPromises();
    expect(renderDiagram).not.toHaveBeenCalled();
    expect(wrapper.find('pre.mermaid').exists()).toBe(false);

    await wrapper.get('.vcp-tool-block > .tool-header-content').trigger('click');
    await flushPromises();
    expect(renderDiagram).toHaveBeenCalledTimes(1);
    expect(wrapper.get('.vcp-mermaid-wrapper svg').text()).toBe('rendered diagram');
  });
});

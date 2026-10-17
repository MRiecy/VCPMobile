import { defineComponent, markRaw, nextTick, reactive, watch } from 'vue';
import { mount, type VueWrapper } from '@vue/test-utils';
import { createPinia, disposePinia, setActivePinia } from 'pinia';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import MessageRenderer from '@/features/chat/MessageRenderer.vue';
import { useMessageStyleInjector } from '@/core/composables/useMessageStyleInjector';
import { useChatSessionStore } from '@/core/stores/chatSessionStore';
import { useChatStreamStore } from '@/core/stores/chatStreamStore';
import type { ChatMessage, MarkdownNode } from '@/core/types/chat';
import { separateMessageStyles } from '@/features/chat/utils/messageStyles';

let pinia: ReturnType<typeof createPinia>;
const mounted: VueWrapper[] = [];
beforeEach(() => {
  pinia = createPinia();
  setActivePinia(pinia);
});
afterEach(() => {
  for (const wrapper of mounted.splice(0)) wrapper.unmount();
  disposePinia(pinia);
});

function mountMessage(message: ChatMessage) {
  const wrapper = mount(MessageRenderer, {
    attachTo: document.body,
    props: { message },
    global: {
      plugins: [pinia], directives: { longpress: {} },
      stubs: { VcpAvatar: true, MermaidFullScreenViewer: true },
    },
  });
  mounted.push(wrapper);
  return wrapper;
}

function message(blocks: ChatMessage['blocks']): ChatMessage {
  return reactive({
    id: 'style-message', role: 'assistant', timestamp: 1, blocks,
    shell: { isUser: false, displayName: 'Agent', avatarColor: '#64748b' },
  });
}

describe('message style ownership', () => {
  it('aggregates scoped fragments in content order and removes deleted contributions', async () => {
    const current = message([
      { type: 'style', content: '.a { color: red }' },
      { type: 'markdown', nodes: [{ type: 'raw_html', content: '<style>.b { color: blue }</style>' }] },
      { type: 'style', content: '.a { color: green }' },
    ]);
    const wrapper = mountMessage(current);
    await nextTick();
    const style = document.head.querySelector('[data-vcp-scope-id="style-message"]')!;
    expect(style.textContent?.replace(/\s+/g, ' ').trim()).toBe('[data-message-id="style-message"] .a { color: red } [data-message-id="style-message"] .b { color: blue } [data-message-id="style-message"] .a { color: green }');
    expect(wrapper.find('style').exists()).toBe(false);
    current.blocks = [{ type: 'style', content: '.b { color: blue }' }];
    await nextTick();
    expect(document.head.querySelector('[data-vcp-scope-id="style-message"]')).toBe(style);
    expect(style.textContent).not.toContain('.a');
    current.blocks = [];
    await nextTick();
    expect(document.head.querySelector('[data-vcp-scope-id="style-message"]')).toBeNull();
  });

  it('retains a standalone style exposed by a brk bubble split', async () => {
    const wrapper = mountMessage(message([{
      type: 'markdown', hash: 'split-css', nodes: [
        { type: 'raw_html', content: '<style>.split { color: red }</style>' },
        { type: 'raw_html', content: '<!--brk-->' },
        { type: 'paragraph', children: [{ type: 'text', value: 'second bubble' }] },
      ],
    }]));
    await nextTick();
    expect(document.head.querySelector('[data-vcp-scope-id="style-message"]')?.textContent).toContain('[data-message-id="style-message"] .split');
    expect(wrapper.text()).toContain('second bubble');
    expect(wrapper.find('style').exists()).toBe(false);
  });

  it('does not traverse stable AST or rewrite its stylesheet for tail-only updates', async () => {
    let nodeReads = 0;
    let cssReads = 0;
    const node: MarkdownNode = markRaw({ type: 'paragraph', children: [{ type: 'text', value: 'stable' }] });
    Object.defineProperty(node, 'type', { get: () => { nodeReads++; return 'paragraph'; } });
    const styleNode: MarkdownNode = markRaw({ type: 'raw_html', content: '' });
    Object.defineProperty(styleNode, 'content', { get: () => { cssReads++; return '<style>.stable { color: red }</style>'; } });
    const current = message([
      { type: 'markdown', hash: 'stable-text', nodes: [node] },
      { type: 'markdown', hash: 'stable-style', nodes: [styleNode] },
    ]);
    useChatSessionStore().setConversation({ id: 'agent-a', type: 'agent' }, 'topic-a');
    useChatStreamStore().addSessionStream('agent-a', 'agent', 'topic-a', current.id);
    current.tailBlock = { type: 'markdown', content: 'tail', hash: 'tail-0', render_mode: 'plain' };
    const wrapper = mountMessage(current);
    await nextTick();
    await nextTick();
    nodeReads = 0;
    cssReads = 0;
    const style = document.head.querySelector('[data-vcp-scope-id="style-message"]')!;
    let styleWrites = 0;
    const observer = new MutationObserver(records => { styleWrites += records.length; });
    observer.observe(style, { childList: true, characterData: true, subtree: true });
    for (let i = 1; i <= 20; i++) {
      current.tailBlock = { type: 'markdown', content: `tail ${i}`, hash: `tail-${i}`, render_mode: 'plain' };
      await nextTick();
    }
    expect(wrapper.text()).toContain('tail 20');
    expect(nodeReads).toBe(0);
    expect(cssReads).toBe(0);
    expect(styleWrites + observer.takeRecords().length).toBe(0);
    observer.disconnect();
  });

  it('separates fallback HTML and CSS once, including an unfinished style tag', () => {
    const projected = separateMessageStyles('<p>before</p><style>.a { color: red }</style><p>after</p><style>.b { color: blue }');
    expect(projected.html).toBe('<p>before</p><p>after</p>');
    expect(projected.css).toBe('.a { color: red }\n.b { color: blue }');
  });

  it('does not let an old mount remove the new mount stylesheet for the same message', () => {
    const StyleOwner = defineComponent({
      props: { css: { type: String, required: true } },
      setup(props) {
        const { injectScopedCss } = useMessageStyleInjector();
        watch(() => props.css, css => injectScopedCss(css, 'overlap'), { immediate: true });
        return {};
      },
      template: '<div />',
    });
    const old = mount(StyleOwner, { props: { css: '.old { color: red }' } });
    const next = mount(StyleOwner, { props: { css: '.new { color: blue }' } });
    mounted.push(old, next);
    old.unmount();
    const styles = document.head.querySelectorAll('[data-vcp-scope-id="overlap"]');
    expect(styles).toHaveLength(1);
    expect(styles[0].textContent).toContain('.new');
    next.unmount();
    expect(document.head.querySelector('[data-vcp-scope-id="overlap"]')).toBeNull();
  });
});

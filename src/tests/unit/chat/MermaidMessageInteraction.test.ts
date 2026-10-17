import { defineComponent, nextTick, reactive, ref } from 'vue';
import { flushPromises, mount, type VueWrapper } from '@vue/test-utils';
import { createPinia, disposePinia, setActivePinia } from 'pinia';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import MessageRenderer from '@/features/chat/MessageRenderer.vue';
import { useMessageEvents } from '@/core/composables/useMessageEvents';
import { useChatHistoryStore } from '@/core/stores/chatHistoryStore';
import { useChatSessionStore } from '@/core/stores/chatSessionStore';
import { useChatStreamStore } from '@/core/stores/chatStreamStore';
import { useThemeStore } from '@/core/stores/theme';
import type { ChatMessage, ContentBlock, MarkdownNode, StreamBlock } from '@/core/types/chat';
import { mockInvoke } from '@/tests/mocks/tauri';

const { render, findImage } = vi.hoisted(() => ({
  render: vi.fn<(id: string, source: string) => Promise<{ svg: string }>>(),
  findImage: vi.fn(async () => null),
}));
vi.mock('mermaid', () => ({ default: { initialize: vi.fn(), render } }));
vi.mock('@/core/utils/renderedImage', () => ({ findRenderedImagePayload: findImage }));

const svg = (label: string) => `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><text>${label}</text></svg>`;
const Viewer = defineComponent({
  props: ['visible', 'svgHtml', 'sourceCode'],
  emits: ['close'],
  template: '<div data-viewer />',
});
const wrappers: VueWrapper[] = [];
let pinia: ReturnType<typeof createPinia>;
let sequence = 0;

beforeEach(() => {
  pinia = createPinia();
  setActivePinia(pinia);
  render.mockReset().mockResolvedValue({ svg: svg('ready') });
  useThemeStore().isDarkResolved = false;
});

afterEach(async () => {
  for (const wrapper of wrappers.splice(0)) wrapper.unmount();
  disposePinia(pinia);
  await flushPromises();
});

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(done => { resolve = done; });
  return { promise, resolve };
}

function code(source: string): MarkdownNode {
  return { type: 'code_block', lang: 'mermaid', code: source, highlighted_html: null, theme: null };
}

function markdown(source: string, hash: string): Extract<StreamBlock, { type: 'markdown' }> {
  return { type: 'markdown', content: source, hash, nodes: [code(source)] };
}

function mountMessage(message: ChatMessage) {
  const wrapper = mount(MessageRenderer, {
    props: { message },
    global: {
      plugins: [pinia],
      directives: { longpress: {} },
      stubs: { VcpAvatar: true, MermaidFullScreenViewer: Viewer },
    },
  });
  wrappers.push(wrapper);
  return wrapper;
}

function staticMessage(blocks: ContentBlock[]) {
  return reactive<ChatMessage>({
    id: `mermaid-static-${++sequence}`, role: 'assistant', timestamp: 1,
    shell: { displayName: 'Agent', avatarColor: '#64748b', isUser: false }, blocks,
  });
}

async function startStream(stableBlocks: StreamBlock[], tailType: 'markdown' | 'thought' = 'markdown', source = 'tail', tailNodes: MarkdownNode[] = [code(source)]) {
  const id = `mermaid-stream-${++sequence}`;
  useChatSessionStore().setConversation({ id: 'agent-a', type: 'agent' }, 'topic-a');
  const store = useChatStreamStore();
  mockInvoke('reconnect_chat_streams', () => ({ generations: [], groupTurns: [], activeRequests: [] }));
  await store.checkAndRecoverInterruptedStreams();
  const base = {
    consumerRevision: 1, messageId: id,
    context: { ownerId: 'agent-a', ownerType: 'agent' as const, topicId: 'topic-a', agentId: 'agent-a' },
    chunk: null, finishReason: null, error: null, blocks: null, timestamp: null, topicUpdatedAt: null,
  };
  await store.processStreamEvent({
    ...base, type: 'aurora',
    aurora: {
      kind: 'snapshot', streamId: 1, content: source, stableBlocks,
      tailBlock: tailType === 'thought'
        ? { type: 'thought', content: source, hash: `${id}-tail`, theme: '思维链', is_complete: false }
        : { type: 'markdown', content: source, hash: `${id}-tail` },
      tailMode: 'ast',
      tailFrame: {
        streamId: 1, epoch: 1, revision: 1, frameSeq: 1, reset: true, mutations: [], snapshot: tailNodes,
      },
    },
  });
  await vi.waitFor(() => expect(store.getActiveStreamMessage('agent-a', 'agent', 'topic-a', id)?.tailFrame).toBeDefined());
  const message = store.getActiveStreamMessage('agent-a', 'agent', 'topic-a', id)!;
  return {
    message,
    end: async (blocks: ContentBlock[]) => {
      mockInvoke('process_message_content', () => blocks);
      await store.processStreamEvent({ ...base, type: 'end', aurora: null, finishReason: 'completed', content: source });
      await vi.waitFor(() => expect(store.isMessageActive('agent-a', 'agent', 'topic-a', id)).toBe(false));
      await nextTick();
    },
  };
}

describe('Mermaid message rendering and delegation', () => {
  it.each([
    ['before', false], ['before', true], ['after', false], ['after', true],
  ] as const)('opens on the first click when compilation finishes %s the terminal AST (same hash=%s)', async (timing, sameHash) => {
    const source = `graph TD; ${timing}${sameHash}-->end`;
    const completion = deferred<{ svg: string }>();
    render.mockReturnValue(completion.promise);
    const stream = await startStream([markdown(source, `${timing}-stable`)]);
    const wrapper = mountMessage(stream.message);
    const send = vi.spyOn(useChatHistoryStore(), 'sendMessage').mockResolvedValue(undefined);
    await vi.waitFor(() => expect(render).toHaveBeenCalled());
    if (timing === 'before') {
      completion.resolve({ svg: svg(timing) });
      await vi.waitFor(() => expect(wrapper.find('[data-message-render-region="stable"] svg').exists()).toBe(true));
    }
    await stream.end([markdown(source, sameHash ? `${timing}-stable` : `${timing}-final`)]);
    completion.resolve({ svg: svg(timing) });
    await vi.waitFor(() => expect(wrapper.find('.vcp-mermaid-wrapper svg').exists()).toBe(true));
    await wrapper.get('.vcp-mermaid-wrapper svg text').trigger('click');
    expect(wrapper.getComponent(Viewer).props()).toMatchObject({ visible: true, sourceCode: source });
    wrapper.getComponent(Viewer).vm.$emit('close');
    await nextTick();
    await wrapper.get('.vcp-mermaid-wrapper button > div').trigger('click');
    expect(wrapper.getComponent(Viewer).props('visible')).toBe(true);
    expect(send).not.toHaveBeenCalled();
    expect(findImage).not.toHaveBeenCalled();
  });

  it('reads replacement SVG and source without installing another element listener', async () => {
    const wrapper = mountMessage(staticMessage([{
      type: 'markdown', nodes: [{ type: 'raw_html', content:
        `<div class="mermaid" data-mermaid-source="old" data-vcp-mermaid-enhanced="true">${svg('old')}</div>`,
      }],
    }]));
    await vi.waitFor(() => expect(wrapper.find('.vcp-mermaid-wrapper').exists()).toBe(true));
    const diagram = wrapper.get('.mermaid').element as HTMLElement;
    // Simulate content replacement after decoration, without a Vue update or an enhancement pass.
    diagram.innerHTML = svg('current');
    diagram.dataset.mermaidSource = 'current source';
    await wrapper.get('.mermaid text').trigger('click');
    expect(wrapper.getComponent(Viewer).props()).toMatchObject({ visible: true, sourceCode: 'current source' });
    expect(wrapper.getComponent(Viewer).props('svgHtml')).toContain('current');
    expect(render).not.toHaveBeenCalled();
    expect(findImage).not.toHaveBeenCalled();
  });

  it.each(['replace', 'unmount'] as const)('discards an outstanding SVG after %s', async action => {
    const completion = deferred<{ svg: string }>();
    render.mockReturnValueOnce(completion.promise);
    const message = staticMessage([markdown(`graph TD; abandoned${action}-->X`, 'old')]);
    const wrapper = mountMessage(message);
    await vi.waitFor(() => expect(render).toHaveBeenCalled());
    const oldNode = wrapper.get('.mermaid-placeholder').element;
    if (action === 'unmount') wrapper.unmount();
    else {
      message.blocks = [{ type: 'markdown', hash: 'replacement', nodes: [{ type: 'paragraph', children: [{ type: 'text', value: 'replacement' }] }] }];
      await nextTick();
    }
    completion.resolve({ svg: svg('obsolete') });
    await flushPromises();
    expect(oldNode.querySelector('svg')).toBeNull();
    if (action === 'replace') expect(wrapper.text()).toContain('replacement');
  });

  it('keeps the current theme when an older theme finishes late', async () => {
    const source = 'graph TD; themeRace-->X';
    const wrapper = mountMessage(staticMessage([markdown(source, 'theme') ]));
    await vi.waitFor(() => expect(wrapper.find('.mermaid svg').exists()).toBe(true));
    const dark = deferred<{ svg: string }>();
    render.mockReturnValueOnce(dark.promise);
    useThemeStore().isDarkResolved = true;
    await vi.waitFor(() => expect(render).toHaveBeenCalledTimes(2));
    useThemeStore().isDarkResolved = false;
    await vi.waitFor(() => expect(wrapper.find('.mermaid svg').exists()).toBe(true));
    dark.resolve({ svg: svg('obsolete dark') });
    await flushPromises();
    await wrapper.get('.mermaid svg').trigger('click');
    expect(wrapper.getComponent(Viewer).props('sourceCode')).toBe(source);
    expect(wrapper.getComponent(Viewer).props('svgHtml')).not.toContain('obsolete dark');
  });

  it.each(['markdown', 'thought'] as const)('leaves the %s AST tail untouched until stable content takes over', async tailType => {
    const source = `graph TD; ${tailType}Tail-->X`;
    const stream = await startStream([], tailType, source);
    const wrapper = mountMessage(stream.message);
    await vi.waitFor(() => expect(wrapper.find('.vcp-ast-sandbox .mermaid-placeholder').exists()).toBe(true));
    const placeholder = wrapper.get('.vcp-ast-sandbox .mermaid-placeholder').element;
    useThemeStore().isDarkResolved = true;
    await flushPromises();
    expect(placeholder.textContent).toBe(source);
    expect(render).not.toHaveBeenCalled();
    const blocks: ContentBlock[] = tailType === 'thought'
      ? [{ type: 'thought', content: source, theme: '思维链', is_complete: true, nodes: [code(source)] }]
      : [markdown(source, 'stable-tail')];
    await stream.end(blocks);
    await vi.waitFor(() => expect(wrapper.find('.vcp-mermaid-wrapper svg').exists()).toBe(true));
    await wrapper.get('.vcp-mermaid-wrapper svg').trigger('click');
    expect(wrapper.getComponent(Viewer).props()).toMatchObject({ visible: true, sourceCode: source });
  });

  it('renders nested AST, diary and marked tool diagrams through the same stable boundary', async () => {
    const wrapper = mountMessage(staticMessage([
      { type: 'markdown', nodes: [{ type: 'blockquote', children: [{ type: 'list', ordered: false, items: [[code('graph TD; nested-->X')]] }] }] },
      { type: 'diary', maid: 'Agent', date: '2026-10-18', content: 'diary', nodes: [code('graph TD; diary-->X')] },
      { type: 'tool-result', tool_name: 'Tool', status: 'success', details: [{ key: 'result', value: '```mermaid\ngraph TD; tool-->X\n```' }], footer: '' },
    ]));
    await vi.waitFor(() => expect(wrapper.findAll('.vcp-mermaid-wrapper')).toHaveLength(3));
    for (const diagram of wrapper.findAll('[data-content-kind="mermaid"]')) {
      await diagram.get('svg').trigger('click');
      expect(wrapper.getComponent(Viewer).props('sourceCode')).toBe((diagram.element as HTMLElement).dataset.mermaidSource);
    }
  });

  it('preserves URL repair and image routing for emoticons in the active AST tail', async () => {
    mockInvoke('fix_emoticon_url', () => 'https://example.com/fixed.png');
    const stream = await startStream([], 'markdown', 'emoticon tail', [{
      type: 'paragraph', children: [{
        type: 'image', src: 'https://example.com/表情包/smile.png', alt: '表情包', title: null, needs_asset_conversion: false,
      }],
    }]);
    const wrapper = mountMessage(stream.message);
    await vi.waitFor(() => expect(wrapper.get('.vcp-ast-sandbox img').attributes('src')).toBe('https://example.com/fixed.png'));
    expect(wrapper.get('.vcp-ast-sandbox img').classes()).toContain('vcp-emoticon');
    await wrapper.get('.vcp-ast-sandbox img').trigger('click');
    expect(findImage).not.toHaveBeenCalled();
  });
});

describe('Mermaid event ownership', () => {
  function mountEvents(html: string) {
    const open = vi.fn();
    const wrapper = mount(defineComponent({
      setup() {
        const root = ref<HTMLElement | null>(null);
        useMessageEvents(root, open);
        return { root, html };
      },
      template: '<div ref="root"><div data-message-render-region="stable" v-html="html" /></div>',
    }), { global: { plugins: [pinia] } });
    wrappers.push(wrapper);
    return { wrapper, open };
  }

  it('owns pending diagrams and SVG labels/buttons before generic routes, without decoration', async () => {
    const { wrapper, open } = mountEvents('<div data-content-kind="mermaid" data-mermaid-source="source"><button data-send="wrong">pending</button></div>');
    const send = vi.spyOn(useChatHistoryStore(), 'sendMessage').mockResolvedValue(undefined);
    await wrapper.get('button').trigger('click');
    expect(open).not.toHaveBeenCalled();
    wrapper.get('[data-content-kind]').element.innerHTML = '<svg><path/><foreignObject><a href="https://example.com"><button data-vcp-button="wrong"><span>label</span></button></a></foreignObject></svg>';
    for (const selector of ['svg', 'path', 'span']) await wrapper.get(selector).trigger('click');
    expect(open).toHaveBeenCalledTimes(3);
    expect(open).toHaveBeenLastCalledWith(expect.stringContaining('<svg'), 'source');
    expect(send).not.toHaveBeenCalled();
    expect(findImage).not.toHaveBeenCalled();
    const detachedRoot = wrapper.element;
    wrapper.unmount();
    detachedRoot.querySelector('path')!.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    expect(open).toHaveBeenCalledTimes(3);
  });

  it('preserves ordinary image, SVG, attachment, emoticon and message button routes', async () => {
    const { wrapper, open } = mountEvents('<img data-image src="image.png"><svg data-image/><img class="vcp-emoticon"><div class="vcp-attachment-preview"><img></div><button data-send="hello">send</button><div data-vcp-button="vcp">vcp</div>');
    const send = vi.spyOn(useChatHistoryStore(), 'sendMessage').mockResolvedValue(undefined);
    for (const el of wrapper.findAll('[data-image]')) await el.trigger('click');
    expect(findImage).toHaveBeenCalledTimes(2);
    await wrapper.get('.vcp-emoticon').trigger('click');
    await wrapper.get('.vcp-attachment-preview img').trigger('click');
    expect(findImage).toHaveBeenCalledTimes(2);
    await wrapper.get('button').trigger('click');
    await wrapper.get('[data-vcp-button]').trigger('click');
    expect(send.mock.calls.map(args => args[0])).toEqual(['[[点击按钮:hello]]', 'vcp']);
    expect(open).not.toHaveBeenCalled();
  });
});

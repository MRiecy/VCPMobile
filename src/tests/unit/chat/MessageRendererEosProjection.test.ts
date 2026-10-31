import { nextTick } from 'vue';
import { mount, type VueWrapper } from '@vue/test-utils';
import { createPinia, disposePinia, setActivePinia } from 'pinia';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import MessageRenderer from '@/features/chat/MessageRenderer.vue';
import { useChatSessionStore } from '@/core/stores/chatSessionStore';
import { useChatStreamStore } from '@/core/stores/chatStreamStore';
import { clearHtmlCache } from '@/core/utils/astRenderer';
import type { ContentBlock, MarkdownNode, StreamBlock, StreamEventDto } from '@/core/types/chat';

vi.mock('pele', () => ({ render: vi.fn(() => ({ svg: '<svg></svg>' })) }));

const context = { ownerId: 'eos-agent', ownerType: 'agent' as const, topicId: 'eos-topic' };
const messageId = 'eos-projection';
const clone = <T,>(value: T): T => JSON.parse(JSON.stringify(value));
const paragraph = (value: string, hash: number): MarkdownNode => ({
  type: 'paragraph', children: [{ type: 'text', value }], hash,
});
const markdown = (content: string, nodes: MarkdownNode[], hash = content): StreamBlock => ({
  type: 'markdown', content, nodes, hash,
});

describe('authoritative EOS presentation handoff', () => {
  let pinia: ReturnType<typeof createPinia>;
  let wrapper: VueWrapper | undefined;

  beforeEach(() => {
    pinia = createPinia();
    setActivePinia(pinia);
    clearHtmlCache();
    useChatSessionStore().setConversation({ id: context.ownerId, type: 'agent' }, context.topicId);
  });

  afterEach(() => {
    wrapper?.unmount();
    wrapper = undefined;
    disposePinia(pinia);
  });

  function event(update: Partial<StreamEventDto>): StreamEventDto {
    return {
      consumerRevision: 1, messageId, context, type: 'aurora', chunk: null,
      finishReason: null, error: null, aurora: null, blocks: null,
      timestamp: null, topicUpdatedAt: null, ...update,
    };
  }

  async function start(stableBlocks: StreamBlock[], tailBlock?: StreamBlock) {
    const store = useChatStreamStore();
    await store.processStreamEvent(event({
      aurora: {
        kind: 'snapshot', streamId: 1,
        content: stableBlocks.map(block => block.content || '').join('\n\n') + (tailBlock?.content || ''),
        stableBlocks, tailBlock, tailMode: 'plain',
        tailFrame: { streamId: 1, epoch: 1, revision: 1, frameSeq: 1, reset: true, snapshot: [], mutations: [] },
      },
    }));
    const message = store.getActiveStreamMessage(context.ownerId, 'agent', context.topicId, messageId)!;
    await vi.waitFor(() => expect(message.blocks).toHaveLength(stableBlocks.length));
    message.shell = { isUser: false, displayName: 'Agent', avatarColor: '' };
    wrapper = mount(MessageRenderer, {
      props: { message }, attachTo: document.body,
      global: { plugins: [pinia], directives: { longpress: {} }, stubs: { VcpAvatar: true } },
    });
    await nextTick();
    return { store, message, view: wrapper };
  }

  async function finish(blocks: ContentBlock[], content = 'authoritative response') {
    await useChatStreamStore().processStreamEvent(event({
      type: 'end', finishReason: 'completed', content, blocks,
    }));
    await nextTick();
  }

  it('keeps paragraph, table and code DOM while the complete terminal result replaces stream blocks', async () => {
    const first = paragraph('first paragraph', 101);
    const last = paragraph('last paragraph', 102);
    const table: MarkdownNode = {
      type: 'table', hash: 103, wrapper_class: 'vcp-scrollable no-swipe',
      header: [[{ type: 'text', value: 'Header' }]],
      rows: [[[{ type: 'text', value: 'Body' }]]],
    };
    const code: MarkdownNode = {
      type: 'code_block', hash: 104, lang: 'text', code: 'wide code', theme: 'github-dark',
      highlighted_html: '<pre class="vcp-code-block vcp-scrollable"><code>wide code</code></pre>',
    };
    const { message, view, store } = await start([
      markdown('first paragraph', [first]),
      markdown('| Header |\n| --- |\n| Body |', [table]),
      markdown('last paragraph', [last]),
      markdown('```text\nwide code\n```', [code]),
    ]);
    const oldParagraphs = view.findAll('.vcp-markdown-block > p').map(node => node.element);
    const oldTable = view.get<HTMLElement>('.vcp-scrollable > table').element;
    const oldCode = view.get<HTMLElement>('pre.vcp-code-block').element;
    oldTable.scrollLeft = 73;
    oldCode.scrollLeft = 41;

    // parse_content combines ordinary Markdown; a closed code fence remains its own block.
    // Wire-level block hashes change from source-text hex strings to structural u64 numbers.
    const terminal: ContentBlock[] = [
      { type: 'markdown', nodes: clone([first, table, last]), hash: 9001 },
      { type: 'markdown', nodes: clone([code]), hash: 9002 },
    ];
    await finish(terminal);

    expect(message.blocks).toEqual(terminal);
    expect(message.blocks).toHaveLength(2);
    expect(message.tailBlock).toBeUndefined();
    expect(store.isMessageActive(context.ownerId, 'agent', context.topicId, messageId)).toBe(false);
    expect(view.findAll('.vcp-markdown-block > p').map(node => node.element)).toEqual(oldParagraphs);
    expect(view.get('.vcp-scrollable > table').element).toBe(oldTable);
    expect(view.get('pre.vcp-code-block').element).toBe(oldCode);
    expect(oldTable.scrollLeft).toBe(73);
    expect(oldCode.scrollLeft).toBe(41);
    expect(oldTable.isConnected && oldCode.isConnected).toBe(true);
  });

  it('keeps repeated paragraphs distinct and renders changed content even when u64 hashes round alike', async () => {
    const firstHash = Number('18446744073709551614');
    const secondHash = Number('18446744073709551615');
    expect(firstHash).toBe(secondHash);
    const repeated = paragraph('repeated', 77);
    const { view } = await start([
      markdown('repeated', [clone(repeated)], 'same-source-hash'),
      markdown('repeated', [clone(repeated)], 'same-source-hash'),
      markdown('before', [paragraph('before', firstHash)]),
    ]);
    const before = view.findAll('.vcp-markdown-block > p').map(node => node.element);
    expect(before[0]).not.toBe(before[1]);
    await finish([{
      type: 'markdown', hash: 999,
      nodes: [clone(repeated), clone(repeated), paragraph('after', secondHash)],
    }]);

    const after = view.findAll('.vcp-markdown-block > p').map(node => node.element);
    expect(after.map(node => node.textContent)).toEqual(['repeated', 'repeated', 'after']);
    expect(after[0]).toBe(before[0]);
    expect(after[1]).toBe(before[1]);
    expect(after[0]).not.toBe(after[1]);
    expect(view.text()).not.toContain('before');
  });

  it('replaces a speculative thought tail with the authoritative Markdown interpretation', async () => {
    const prefix = paragraph('prefix', 201);
    const { view, message } = await start([markdown('prefix', [prefix])], {
      type: 'thought', theme: '思维链', content: 'unfinished thought', hash: 'thought-tail', is_complete: false,
    });
    expect(view.find('.vcp-thought-block').exists()).toBe(true);
    expect(view.text()).toContain('unfinished thought');
    const terminal: ContentBlock[] = [
      { type: 'markdown', nodes: clone([prefix]), hash: 2001 },
      { type: 'markdown', nodes: [{ type: 'raw_html', content: '<think>', hash: 202 }], hash: 2002 },
      { type: 'markdown', nodes: [paragraph('unfinished thought', 203)], hash: 2003 },
    ];
    await finish(terminal, 'prefix\n\n<think>unfinished thought');

    expect(message.blocks).toEqual(terminal);
    expect(message.content).toBe('prefix\n\n<think>unfinished thought');
    expect(message.tailBlock).toBeUndefined();
    expect(view.find('.vcp-thought-block').exists()).toBe(false);
    expect(view.find('[data-message-render-region="tail"]').exists()).toBe(false);
    expect(view.text()).toContain('unfinished thought');
  });

  it('keeps raw HTML containers intact across brk bubbles and preserves scoped style ownership', async () => {
    const container: MarkdownNode[] = [
      { type: 'raw_html', content: '<div class="rich-container">' },
      paragraph('inside container', 301),
      { type: 'raw_html', content: '</div>' },
    ];
    const css = '.rich-container { color: red }';
    const afterBreak = paragraph('second bubble', 302);
    const { view } = await start([
      { type: 'style', content: css, hash: 'style-source' },
      markdown('<div class="rich-container">inside container</div>', clone(container)),
      markdown('<!--brk-->', [{ type: 'raw_html', content: '<!--brk-->' }]),
      markdown('second bubble', [clone(afterBreak)]),
    ]);
    await finish([
      { type: 'style', content: css, hash: 3001 },
      {
        type: 'markdown', hash: 3002,
        nodes: [...clone(container), { type: 'raw_html', content: '<!--brk-->' }, clone(afterBreak)],
      },
    ]);

    const containerElement = view.get('.rich-container').element;
    expect(containerElement.children).toHaveLength(1);
    expect(containerElement.firstElementChild?.tagName).toBe('P');
    expect(containerElement.textContent).toBe('inside container');
    const bubbleContents = view.findAll('.vcp-content-blocks');
    expect(bubbleContents).toHaveLength(2);
    expect(bubbleContents[0].text()).toContain('inside container');
    expect(bubbleContents[0].text()).not.toContain('second bubble');
    expect(bubbleContents[1].text()).toBe('second bubble');
    expect(view.find('style').exists()).toBe(false);
    const styles = document.head.querySelectorAll('[data-vcp-scope-id="eos-projection"]');
    expect(styles).toHaveLength(1);
    expect(styles[0].textContent).toContain('[data-message-id="eos-projection"] .rich-container');
  });
});

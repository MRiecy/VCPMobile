import { defineComponent, nextTick, reactive } from 'vue';
import { mount } from '@vue/test-utils';
import { createPinia, setActivePinia, type Pinia } from 'pinia';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import MessageRenderer from '@/features/chat/MessageRenderer.vue';
import type { ChatMessage, StreamBlock } from '@/core/types/chat';
import { useChatSessionStore } from '@/core/stores/chatSessionStore';
import { useChatStreamStore } from '@/core/stores/chatStreamStore';
import { useChatPreferencesStore } from '@/features/chat/preferences/store';
import { invokeMock, mockInvoke } from '@/tests/mocks/tauri';
import * as astRenderer from '@/core/utils/astRenderer';

const { mermaidRenderMock } = vi.hoisted(() => ({
  mermaidRenderMock: vi.fn(() => ({ svg: '<svg viewBox="0 0 10 10"></svg>' })),
}));

vi.mock('pele', () => ({
  render: mermaidRenderMock,
}));


const markerStub = (marker: string) => defineComponent({
  template: `<div data-strong-block="${marker}">${marker}</div>`,
});

const streamRendererStubs = {
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
};

function mountStreamRenderer(message: ChatMessage, pinia: Pinia) {
  return mount(MessageRenderer, {
    props: { message },
    global: {
      plugins: [pinia],
      directives: { longpress: {} },
      stubs: streamRendererStubs,
    },
  });
}

function installRunningAnimationMock(): () => void {
  const original = Object.getOwnPropertyDescriptor(Element.prototype, 'animate');
  const originalCss = Object.getOwnPropertyDescriptor(globalThis, 'CSS');
  Object.defineProperty(globalThis, 'CSS', {
    configurable: true,
    value: {
      supports: vi.fn(() => true),
      registerProperty: vi.fn(),
    },
  });
  Object.defineProperty(Element.prototype, 'animate', {
    configurable: true,
    value: vi.fn(() => ({
      playState: 'running',
      onfinish: null,
      oncancel: null,
      cancel: vi.fn(),
    } as unknown as Animation)),
  });
  return () => {
    if (original) {
      Object.defineProperty(Element.prototype, 'animate', original);
    } else {
      Reflect.deleteProperty(Element.prototype, 'animate');
    }
    if (originalCss) {
      Object.defineProperty(globalThis, 'CSS', originalCss);
    } else {
      Reflect.deleteProperty(globalThis, 'CSS');
    }
  };
}

beforeEach(() => {
  localStorage.removeItem('vcp-smooth-streaming-enabled');
  localStorage.removeItem('vcp-collapse-tools-while-streaming');
});

function deferred<T>() {
  let resolve!: (value: T | PromiseLike<T>) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

async function seedRemountTail(
  streamStore: ReturnType<typeof useChatStreamStore>,
  messageId: string,
  streamId: number,
) {
  const context = {
    ownerId: 'agent-a',
    ownerType: 'agent' as const,
    topicId: 'topic-a',
    agentId: 'agent-a',
  };
  const eventBase = {
    chunk: null,
    finishReason: null,
    error: null,
    blocks: null,
    timestamp: null,
    topicUpdatedAt: null,
  };
  await streamStore.processStreamEvent({
    consumerRevision: 1,
    ...eventBase,
    type: 'thinking',
    messageId,
    context,
    aurora: null,
  });

  let fullContent = '';
  for (let frameSeq = 1; frameSeq <= 5; frameSeq += 1) {
    const chunk = String.fromCharCode(96 + frameSeq);
    fullContent += chunk;
    await streamStore.processStreamEvent({
      consumerRevision: 1,
      ...eventBase,
      type: 'aurora',
      messageId,
      context,
      aurora: {
        kind: 'delta',
        streamId,
        chunk,
        tailOp: {
          op: 'replace',
          content: fullContent,
          hash: `${messageId}-${frameSeq}`,
          mode: 'ast',
          blockType: 'markdown',
        },
        tailFrame: {
          streamId,
          epoch: 1,
          revision: frameSeq,
          frameSeq,
          mutations: frameSeq === 1
            ? [{
                op: 'add',
                id: 't0',
                parent: 'root',
                node: {
                  type: 'paragraph',
                  children: [{ type: 'text', value: chunk }],
                },
              }]
            : [{ op: 'append', id: 't0.i0', chunk }],
        },
      },
    });
  }
  await vi.waitFor(() => {
    expect(streamStore.getActiveStreamMessage(
      'agent-a',
      'agent',
      'topic-a',
      messageId,
    )?.tailFrame?.frameSeq).toBe(5);
  });

  return {
    context,
    eventBase,
    message: streamStore.getActiveStreamMessage(
      'agent-a',
      'agent',
      'topic-a',
      messageId,
    )!,
  };
}

describe('MessageRenderer presentation shell', () => {
  it('does not re-render stable Markdown in other bubbles while the tail advances', async () => {
    const pinia = createPinia();
    setActivePinia(pinia);
    const stream = useChatStreamStore();
    useChatSessionStore().setConversation({ id: 'agent-a', type: 'agent' }, 'topic-a');
    stream.addSessionStream('agent-a', 'agent', 'topic-a', 'split-stream');
    const markdown = (content: string): StreamBlock => ({
      type: 'markdown', content, hash: content,
      nodes: [{ type: 'paragraph', children: [{ type: 'text', value: content }] }],
    });
    const brk: StreamBlock = { type: 'markdown', content: '<!--brk-->', hash: 'brk' };
    const message = reactive<ChatMessage>({
      id: 'split-stream', role: 'assistant', timestamp: 1,
      shell: { avatarColor: '', displayName: 'Agent', isUser: false },
      blocks: [markdown('first'), brk, markdown('second'), brk],
      tailBlock: { type: 'markdown', content: 'tail', hash: 'tail', render_mode: 'plain' },
    });
    const render = vi.spyOn(astRenderer, 'renderMarkdownNodes');
    const wrapper = mountStreamRenderer(message, pinia);
    try {
      await nextTick();
      expect(render).toHaveBeenCalledTimes(2);
      const stableElements = wrapper.findAll('[data-message-render-region="stable"]').map(block => block.element);
      for (const content of ['tail a', 'tail ab', 'tail abc']) {
        message.tailBlock = { type: 'markdown', content, hash: content, render_mode: 'plain' };
        await nextTick();
        expect(wrapper.get('.streaming-tail').text()).toBe(content);
        expect(render).toHaveBeenCalledTimes(2);
      }
      // A new stable bubble changes the outer list, but existing ASTs still have
      // their own cached HTML instead of contending for a nested v-memo slot.
      message.blocks = [...message.blocks!, markdown('third'), brk];
      await nextTick();
      expect(render).toHaveBeenCalledTimes(3);
      expect(wrapper.findAll('[data-message-render-region="stable"]').slice(0, 2).map(block => block.element))
        .toEqual(stableElements);
    } finally {
      wrapper.unmount();
      stream.$dispose();
      render.mockRestore();
    }
  });

  it('renders guidance as plain tail and preserves expansion through closure and final hashes', async () => {
    const pinia = createPinia();
    setActivePinia(pinia);
    useChatSessionStore().setConversation({ id: 'agent-a', type: 'agent' }, 'topic-a');
    const stream = useChatStreamStore();
    stream.addSessionStream('agent-a', 'agent', 'topic-a', 'voice-guidance');
    const message = reactive<ChatMessage>({
      id: 'voice-guidance', role: 'assistant', timestamp: 1,
      content: '<voice_guidance>\n自然轻快', blocks: [],
      tailContent: '自然轻快',
      tailBlock: { type: 'voice-guidance', content: '自然轻快', is_complete: false, hash: 'tail', render_mode: 'plain' },
      shell: { isUser: false, displayName: 'Agent', avatarColor: '#888' },
    });
    const wrapper = mountStreamRenderer(message, pinia);
    expect(wrapper.text()).not.toContain('<voice_guidance>');
    expect(wrapper.find('.vcp-ast-sandbox').exists()).toBe(false);
    const toggle = wrapper.get('.voice-guidance-toggle');
    expect(toggle.attributes('aria-expanded')).toBe('false');
    await toggle.trigger('click');
    message.blocks = [{ type: 'voice-guidance', content: '自然轻快', is_complete: true, hash: 'stream-hash' }];
    message.tailBlock = undefined;
    message.tailContent = '';
    await nextTick();
    expect(wrapper.findAll('.voice-guidance-toggle')).toHaveLength(1);
    expect(wrapper.get('.voice-guidance-toggle').attributes('aria-expanded')).toBe('true');
    message.blocks = [
      { type: 'voice-guidance', content: '自然轻快', is_complete: true, hash: 123456 },
      { type: 'markdown', content: '[轻笑]你好。', hash: 7654321 },
    ];
    await nextTick();
    expect(wrapper.get('.voice-guidance-toggle').attributes('aria-expanded')).toBe('true');
    expect(wrapper.text()).toContain('[轻笑]你好。');
    expect(wrapper.text()).not.toContain('<voice_guidance>');
    wrapper.unmount();
  });

  it('recovers a plain guidance snapshot and appends without creating a Markdown sandbox', async () => {
    const pinia = createPinia();
    setActivePinia(pinia);
    useChatSessionStore().setConversation({ id: 'agent-a', type: 'agent' }, 'topic-a');
    const stream = useChatStreamStore();
    const context = { ownerId: 'agent-a', ownerType: 'agent' as const, topicId: 'topic-a', agentId: 'agent-a' };
    const base = {
      consumerRevision: 1, messageId: 'guidance-snapshot', context, chunk: null,
      finishReason: null, error: null, blocks: null, timestamp: null, topicUpdatedAt: null,
    };
    await stream.processStreamEvent({ ...base, type: 'thinking', aurora: null });
    await stream.processStreamEvent({ ...base, type: 'aurora', aurora: {
      kind: 'snapshot', streamId: 31, content: '<voice_guidance>自然', stableBlocks: [],
      tailBlock: { type: 'voice-guidance', content: '自然', is_complete: false, hash: 'g1' }, tailMode: 'plain',
    } });
    const message = stream.getActiveStreamMessage('agent-a', 'agent', 'topic-a', 'guidance-snapshot')!;
    const wrapper = mountStreamRenderer(message, pinia);
    await vi.waitFor(() => expect(wrapper.find('.voice-guidance-toggle').exists()).toBe(true));
    await wrapper.get('.voice-guidance-toggle').trigger('click');
    expect(message.tailOp).toMatchObject({ blockType: 'voice-guidance', mode: 'plain' });
    await stream.processStreamEvent({ ...base, type: 'aurora', aurora: {
      kind: 'delta', streamId: 31, chunk: '轻快',
      tailOp: { op: 'append', content: '轻快', hash: 'g2', previousHash: 'g1', mode: 'plain', blockType: 'voice-guidance' },
    } });
    await vi.waitFor(() => expect(wrapper.get('.voice-guidance-content').text()).toBe('自然轻快'));
    expect(wrapper.find('.vcp-ast-sandbox').exists()).toBe(false);
    expect(wrapper.findAll('.voice-guidance-content')).toHaveLength(1);
    await stream.processStreamEvent({ ...base, type: 'aurora', aurora: {
      kind: 'delta', streamId: 31, chunk: '</voice_guidance>\n正文',
      stableAppend: { baseCount: 0, blocks: [{ type: 'voice-guidance', content: '自然轻快', is_complete: true, hash: 'stable-guidance' }] },
      tailOp: { op: 'replace', content: '正文', hash: 'body', mode: 'ast', blockType: 'markdown' },
      tailFrame: {
        streamId: 31, epoch: 1, revision: 1, frameSeq: 1, reset: true, mutations: [],
        snapshot: [{ type: 'paragraph', children: [{ type: 'text', value: '正文' }] }],
      },
    } });
    await vi.waitFor(() => expect(wrapper.get('.vcp-ast-sandbox').text()).toBe('正文'));
    expect(wrapper.findAll('.voice-guidance-toggle')).toHaveLength(1);
    expect(wrapper.get('.voice-guidance-toggle').attributes('aria-expanded')).toBe('true');
    wrapper.unmount();
  });

  it.each(['tool-use', 'jev-tool-use'] as const)('keeps manual tool expansion across preference changes and terminal recompilation (%s)', async requestType => {
    const pinia = createPinia();
    setActivePinia(pinia);
    const preferences = useChatPreferencesStore();
    preferences.setCollapseToolsWhileStreaming(true);
    const stream = useChatStreamStore();
    useChatSessionStore().setConversation({ id: 'agent-a', type: 'agent' }, 'topic-a');
    const context = { ownerId: 'agent-a', ownerType: 'agent' as const, topicId: 'topic-a', agentId: 'agent-a' };
    const base = {
      consumerRevision: 1, messageId: 'tool-preferences', context, chunk: null,
      finishReason: null, error: null, timestamp: null, topicUpdatedAt: null,
    };
    const stableBlocks: StreamBlock[] = [
      { type: 'markdown', content: 'first', hash: 'text-a' },
      { type: 'markdown', content: 'second', hash: 'text-b' },
      { type: requestType, tool_name: 'Search', content: 'query: example', hash: 'tool-a' },
      { type: 'tool-use', tool_name: 'Search', content: 'query: example', hash: 'tool-a' },
      { type: 'tool-result', tool_name: 'Search', status: 'success', details: [{ key: 'answer', value: 'found' }], footer: '', hash: 'result-a' },
      { type: 'thought', theme: 'reasoning', content: 'thinking', is_complete: true, hash: 'thought-a' },
    ];
    await stream.processStreamEvent({
      ...base, type: 'aurora', blocks: null,
      aurora: { kind: 'snapshot', streamId: 11, content: 'response', stableBlocks, tailMode: 'ast' },
    });
    const message = stream.getActiveStreamMessage('agent-a', 'agent', 'topic-a', 'tool-preferences')!;
    await vi.waitFor(() => expect(message.blocks).toHaveLength(stableBlocks.length));
    const { ToolBlock: _toolStub, ThoughtBlock: _thoughtStub, ...stubs } = streamRendererStubs;
    const mountTools = () => mount(MessageRenderer, {
      props: { message },
      global: { plugins: [pinia], directives: { longpress: {} }, stubs },
    });
    const wrapper = mountTools();
    const toolElements = wrapper.findAll('.vcp-tool-block').map(tool => tool.element);
    const expanded = () => wrapper.findAll<HTMLElement>('.vcp-tool-block .tool-content-scrollable').map(content => content.element.style.display !== 'none');
    expect(expanded()).toEqual([false, false, false]);
    expect(wrapper.get('.vcp-thought-content').isVisible()).toBe(true);

    await wrapper.findAll('.vcp-tool-block > .tool-header-content:first-child')[0].trigger('click');
    expect(expanded()).toEqual([true, false, false]);
    preferences.setCollapseToolsWhileStreaming(false);
    await nextTick();
    expect(expanded()).toEqual([true, true, true]);
    await wrapper.findAll('.vcp-tool-block > .tool-header-content:first-child')[2].trigger('click');
    preferences.setCollapseToolsWhileStreaming(true);
    await nextTick();
    expect(expanded()).toEqual([true, false, false]);
    preferences.setCollapseToolsWhileStreaming(false);
    await nextTick();
    expect(expanded()).toEqual([true, true, false]);

    // 持久化解析合并了相邻 markdown，并把字符串 hash 换成数值 hash。
    await stream.processStreamEvent({
      ...base, type: 'end', finishReason: 'completed', aurora: null,
      blocks: [
        { type: 'markdown', content: 'first\nsecond', hash: 100 },
        { type: requestType, tool_name: 'Search', content: 'query: example', is_complete: true, hash: 101 },
        { type: 'tool-use', tool_name: 'Search', content: 'query: example', is_complete: true, hash: 101 },
        { type: 'tool-result', tool_name: 'Search', status: 'success', details: [{ key: 'answer', value: 'found' }], footer: '', hash: 102 },
        { type: 'thought', theme: 'reasoning', content: 'thinking', is_complete: true, hash: 103 },
      ],
    });
    await nextTick();
    expect(wrapper.findAll('.vcp-tool-block').map(tool => tool.element)).toEqual(toolElements);
    expect(expanded()).toEqual([true, false, false]);
    wrapper.unmount();
    const remounted = mountTools();
    expect(remounted.findAll<HTMLElement>('.vcp-tool-block .tool-content-scrollable').every(content => content.element.style.display === 'none')).toBe(true);
    remounted.unmount();
  });

  it('renders named and unnamed JEV requests as literal text', async () => {
    const pinia = createPinia();
    setActivePinia(pinia);
    const raw = "JEV:「始」使用 &#039;B站获取&#039;\n<img src=x onerror=alert(1)>「末」";
    const { ToolBlock: _toolStub, ...stubs } = streamRendererStubs;
    const wrapper = mount(MessageRenderer, {
      props: {
        message: {
          id: 'jev-presentation', role: 'assistant', timestamp: 1,
          shell: { avatarColor: '#64748b', displayName: 'Agent', isUser: false },
          blocks: [
            { type: 'jev-tool-use', tool_name: 'B站获取', content: raw, hash: 1 },
            { type: 'jev-tool-use', tool_name: '', content: 'JEV:「始」我要睡10分钟「末」', hash: 2 },
          ],
        },
      },
      global: { plugins: [pinia], directives: { longpress: {} }, stubs },
    });
    const cards = wrapper.findAll('.is-jev-tool-use');
    expect(cards).toHaveLength(2);
    expect(cards[0].get('.tool-label').text()).toBe('JEVToolUse');
    expect(cards[0].get('.tool-name').text()).toBe('B站获取');
    await cards[0].get('.tool-header-content').trigger('click');
    expect(cards[0].get('pre').element.textContent).toBe(raw);
    expect(cards[0].find('img').exists()).toBe(false);
    expect(cards[1].get('.tool-label').text()).toBe('JEVToolUse');
    expect(cards[1].find('.tool-name').exists()).toBe(false);
    expect(wrapper.text()).not.toContain('Unknown Tool');
    await cards[1].get('.tool-header-content').trigger('click');
    expect(cards[1].get('.tool-content-scrollable').isVisible()).toBe(true);
    wrapper.unmount();
  });

  it('resets tool interaction identity when the request protocol changes', async () => {
    const pinia = createPinia();
    setActivePinia(pinia);
    const message = reactive<ChatMessage>({
      id: 'protocol-change', role: 'assistant', timestamp: 1,
      shell: { avatarColor: '#64748b', displayName: 'Agent', isUser: false },
      blocks: [{ type: 'tool-use', tool_name: 'Search', content: 'request', hash: 1 }],
    });
    const { ToolBlock: _toolStub, ...stubs } = streamRendererStubs;
    const wrapper = mount(MessageRenderer, {
      props: { message },
      global: { plugins: [pinia], directives: { longpress: {} }, stubs },
    });
    await wrapper.get('.tool-header-content').trigger('click');
    const oldElement = wrapper.get('.vcp-tool-block').element;
    message.blocks = [{ type: 'jev-tool-use', tool_name: 'Search', content: 'request', hash: 1 }];
    await nextTick();
    expect(wrapper.get('.vcp-tool-block').element).not.toBe(oldElement);
    expect(wrapper.get<HTMLElement>('.tool-content-scrollable').element.style.display).toBe('none');
    wrapper.unmount();
  });

  it('preserves the default streaming behavior and treats changed tool content as a new block', async () => {
    const pinia = createPinia();
    setActivePinia(pinia);
    const stream = useChatStreamStore();
    useChatSessionStore().setConversation({ id: 'agent-a', type: 'agent' }, 'topic-a');
    stream.addSessionStream('agent-a', 'agent', 'topic-a', 'tool-default');
    const message = reactive<ChatMessage>({
      id: 'tool-default', role: 'assistant', timestamp: 1,
      shell: { avatarColor: '#64748b', displayName: 'Agent', isUser: false },
      blocks: [{ type: 'tool-use', tool_name: 'Search', content: 'original', hash: 'original' }],
    });
    const { ToolBlock: _toolStub, ...stubs } = streamRendererStubs;
    const wrapper = mount(MessageRenderer, {
      props: { message },
      global: { plugins: [pinia], directives: { longpress: {} }, stubs },
    });
    expect(wrapper.get<HTMLElement>('.tool-content-scrollable').element.style.display).not.toBe('none');
    await wrapper.get('.vcp-tool-block > .tool-header-content').trigger('click');
    expect(wrapper.get<HTMLElement>('.tool-content-scrollable').element.style.display).toBe('none');
    message.blocks = [{ type: 'tool-use', tool_name: 'Search', content: 'new request', hash: 'replacement' }];
    await nextTick();
    expect(wrapper.get<HTMLElement>('.tool-content-scrollable').element.style.display).not.toBe('none');
    stream.removeSessionStream('agent-a', 'agent', 'topic-a', 'tool-default');
    await nextTick();
    expect(wrapper.get<HTMLElement>('.tool-content-scrollable').element.style.display).toBe('none');
    wrapper.unmount();
  });

  it('preserves large tool results across hash changes and resets when their trailing content changes', async () => {
    const pinia = createPinia();
    setActivePinia(pinia);
    const stream = useChatStreamStore();
    useChatSessionStore().setConversation({ id: 'agent-a', type: 'agent' }, 'topic-a');
    stream.addSessionStream('agent-a', 'agent', 'topic-a', 'large-tool-result');
    const image = `data:image/png;base64,${'A'.repeat(256 * 1024)}`;
    const resultBlock = (value: string, hash: string | number) => ({
      type: 'tool-result' as const, tool_name: 'Image', status: 'success',
      details: [{ key: 'image', value }], footer: '', hash,
    });
    const message = reactive<ChatMessage>({
      id: 'large-tool-result', role: 'assistant', timestamp: 1,
      shell: { avatarColor: '#64748b', displayName: 'Agent', isUser: false },
      blocks: [resultBlock(image, 'stream-hash')],
    });
    const { ToolBlock: _toolStub, ...stubs } = streamRendererStubs;
    const wrapper = mount(MessageRenderer, {
      props: { message },
      global: { plugins: [pinia], directives: { longpress: {} }, stubs },
    });
    await wrapper.get('.vcp-tool-block > .tool-header-content').trigger('click');
    const toolElement = wrapper.get('.vcp-tool-block').element;
    message.blocks = [resultBlock(image, 12345)];
    await nextTick();
    expect(wrapper.get('.vcp-tool-block').element).toBe(toolElement);
    expect(wrapper.get<HTMLElement>('.tool-content-scrollable').element.style.display).toBe('none');
    message.blocks = [resultBlock(`${image.slice(0, -1)}B`, 12346)];
    await nextTick();
    expect(wrapper.get('.vcp-tool-block').element).not.toBe(toolElement);
    expect(wrapper.get<HTMLElement>('.tool-content-scrollable').element.style.display).not.toBe('none');
    wrapper.unmount();
  });

  it('does not carry tool interaction state into another message with identical content', async () => {
    const pinia = createPinia();
    setActivePinia(pinia);
    const stream = useChatStreamStore();
    useChatSessionStore().setConversation({ id: 'agent-a', type: 'agent' }, 'topic-a');
    const makeMessage = (id: string): ChatMessage => ({
      id, role: 'assistant', timestamp: 1,
      shell: { avatarColor: '#64748b', displayName: 'Agent', isUser: false },
      blocks: [{ type: 'tool-use', tool_name: 'Search', content: 'same request', hash: 'same-hash' }],
    });
    stream.addSessionStream('agent-a', 'agent', 'topic-a', 'previous-message');
    stream.addSessionStream('agent-a', 'agent', 'topic-a', 'next-message');
    const { ToolBlock: _toolStub, ...stubs } = streamRendererStubs;
    const wrapper = mount(MessageRenderer, {
      props: { message: makeMessage('previous-message') },
      global: { plugins: [pinia], directives: { longpress: {} }, stubs },
    });
    await wrapper.get('.vcp-tool-block > .tool-header-content').trigger('click');
    expect(wrapper.get<HTMLElement>('.tool-content-scrollable').element.style.display).toBe('none');
    await wrapper.setProps({ message: makeMessage('next-message') });
    expect(wrapper.get<HTMLElement>('.tool-content-scrollable').element.style.display).not.toBe('none');
    wrapper.unmount();
  });

  it.each([false, true])('keeps stable text and the tail visible after terminal rendering (failed=%s)', async failed => {
    const pinia = createPinia();
    setActivePinia(pinia);
    const sessionStore = useChatSessionStore();
    const streamStore = useChatStreamStore();
    sessionStore.setConversation({ id: 'agent-a', type: 'agent' }, 'topic-a');
    mockInvoke('reconnect_chat_streams', () => ({ generations: [], groupTurns: [], activeRequests: [] }));
    await streamStore.checkAndRecoverInterruptedStreams();
    const context = { ownerId: 'agent-a', ownerType: 'agent' as const, topicId: 'topic-a', agentId: 'agent-a' };
    const base = {
      consumerRevision: 1, messageId: 'terminal-tail', context, chunk: null,
      finishReason: null, error: null, blocks: null, timestamp: null, topicUpdatedAt: null,
    };
    await streamStore.processStreamEvent({
      ...base, type: 'aurora',
      aurora: {
        kind: 'snapshot', streamId: 1, content: 'stable prefix\nvisible tail',
        stableBlocks: [{ type: 'markdown', content: 'stable prefix', hash: 'prefix', nodes: [{
          type: 'paragraph', children: [{ type: 'text', value: 'stable prefix' }],
        }] }],
        tailBlock: { type: 'markdown', content: 'visible tail', hash: 'tail' },
        tailMode: 'ast',
        tailFrame: {
          streamId: 1, epoch: 1, revision: 1, frameSeq: 1, reset: true, mutations: [],
          snapshot: [{ type: 'paragraph', children: [{ type: 'text', value: 'visible tail' }] }],
        },
      },
    });
    await vi.waitFor(() => expect(streamStore.getActiveStreamMessage(
      'agent-a', 'agent', 'topic-a', 'terminal-tail',
    )?.tailFrame).toBeDefined());
    const message = streamStore.getActiveStreamMessage('agent-a', 'agent', 'topic-a', 'terminal-tail')!;
    const wrapper = mountStreamRenderer(message, pinia);
    const errorLog = vi.spyOn(console, 'error').mockImplementation(() => {});
    try {
      await vi.waitFor(() => {
        expect(wrapper.text()).toContain('stable prefix');
        expect(wrapper.get('.vcp-ast-sandbox').text()).toContain('visible tail');
      });
      const originalSandbox = wrapper.get('.vcp-ast-sandbox').element;
      mockInvoke('process_message_content', () => failed
        ? Promise.reject(new Error('renderer unavailable'))
        : [{ type: 'markdown', content: 'stable prefix\nvisible tail', nodes: [{
            type: 'paragraph', children: [{ type: 'text', value: 'stable prefix\nvisible tail' }],
          }] }]);
      await streamStore.processStreamEvent({
        ...base, type: 'end', aurora: null, finishReason: 'completed', content: 'stable prefix\nvisible tail',
      });
      await vi.waitFor(() => expect(streamStore.isMessageActive('agent-a', 'agent', 'topic-a', 'terminal-tail')).toBe(false));
      await nextTick();
      expect(wrapper.text()).toContain('stable prefix');
      expect(wrapper.text().match(/visible tail/g)).toHaveLength(1);
      expect(wrapper.find('[data-strong-block="streaming"]').exists()).toBe(false);
      expect(streamStore.isConversationBusy('agent-a', 'agent', 'topic-a')).toBe(false);
      if (failed) {
        expect(wrapper.get('.vcp-ast-sandbox').element).toBe(originalSandbox);
      } else {
        expect(wrapper.find('.streaming-tail').exists()).toBe(false);
        expect(message.tailFrame).toBeUndefined();
        expect(message.tailSnapshot).toBeUndefined();
      }
    } finally {
      wrapper.unmount();
      streamStore.$dispose();
      errorLog.mockRestore();
    }
  });

  it('enhances an already-rendered first-pass Mermaid SVG immediately', async () => {
    const pinia = createPinia();
    setActivePinia(pinia);
    const message: ChatMessage = {
      id: 'existing-mermaid-svg',
      role: 'assistant',
      timestamp: 1,
      agentId: 'agent-a',
      shell: {
        avatarColor: '#64748b',
        displayName: 'Agent A',
        isUser: false,
      },
      blocks: [{
        type: 'markdown',
        hash: 'existing-mermaid-svg',
        nodes: [{
          type: 'raw_html',
          content: '<div class="mermaid" data-mermaid-source="graph TD;A--&gt;B"><svg viewBox="0 0 10 10"><text>A</text></svg></div>',
        }],
      }],
    };
    const MermaidViewerStub = defineComponent({
      props: { visible: Boolean },
      template: '<div data-mermaid-viewer :data-visible="String(visible)" />',
    });

    const wrapper = mount(MessageRenderer, {
      props: { message },
      global: {
        plugins: [pinia],
        directives: { longpress: {} },
        stubs: {
          VcpAvatar: markerStub('avatar'),
          MermaidFullScreenViewer: MermaidViewerStub,
        },
      },
    });

    await vi.waitFor(() => {
      expect(wrapper.find('.vcp-mermaid-wrapper').exists()).toBe(true);
    });
    await wrapper.get('.vcp-mermaid-wrapper').trigger('click');
    expect(wrapper.get('[data-mermaid-viewer]').attributes('data-visible')).toBe('true');
    expect(mermaidRenderMock).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it('lets a new Aurora stream reset a lower frame sequence from the previous stream', async () => {
    const pinia = createPinia();
    setActivePinia(pinia);
    const sessionStore = useChatSessionStore();
    const streamStore = useChatStreamStore();
    sessionStore.setConversation({
      id: 'agent-a',
      type: 'agent',
      name: 'Agent A',
    } as any, 'topic-a');
    streamStore.addSessionStream('agent-a', 'agent', 'topic-a', 'stream-message');

    const oldNodes = [{
      type: 'paragraph' as const,
      children: [{ type: 'text' as const, value: 'old tail' }],
    }];
    const message = reactive<ChatMessage>({
      id: 'stream-message',
      role: 'assistant',
      timestamp: 1,
      agentId: 'agent-a',
      shell: {
        avatarColor: '#64748b',
        displayName: 'Agent A',
        isUser: false,
      },
      blocks: [],
      tailContent: 'old tail',
      tailBlock: {
        type: 'markdown',
        content: 'old tail',
        nodes: oldNodes,
        hash: 'old',
      },
      tailSnapshot: oldNodes,
      tailFrame: {
        streamId: 1,
        epoch: 1,
        revision: 100,
        frameSeq: 100,
        reset: true,
        snapshot: oldNodes,
        mutations: [],
      },
    });

    const wrapper = mount(MessageRenderer, {
      props: { message },
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
    await nextTick();
    expect(wrapper.get('.vcp-ast-sandbox').text()).toContain('old tail');

    const newNodes = [{
      type: 'paragraph' as const,
      children: [{ type: 'text' as const, value: 'new tail' }],
    }];
    message.tailContent = 'new tail';
    message.tailBlock = {
      type: 'markdown',
      content: 'new tail',
      nodes: newNodes,
      hash: 'new',
    };
    message.tailSnapshot = newNodes;
    message.tailFrame = {
      streamId: 2,
      epoch: 1,
      revision: 1,
      frameSeq: 1,
      mutations: [],
    };
    await nextTick();
    await nextTick();

    expect(wrapper.get('.vcp-ast-sandbox').text()).toContain('new tail');
    wrapper.unmount();
  });

  it('applies mutations that were coalesced behind a reset snapshot', async () => {
    const pinia = createPinia();
    setActivePinia(pinia);
    const sessionStore = useChatSessionStore();
    const streamStore = useChatStreamStore();
    sessionStore.setConversation({
      id: 'agent-a',
      type: 'agent',
      name: 'Agent A',
    } as any, 'topic-a');
    streamStore.addSessionStream('agent-a', 'agent', 'topic-a', 'reset-batch');

    const message = reactive<ChatMessage>({
      id: 'reset-batch',
      role: 'assistant',
      timestamp: 1,
      agentId: 'agent-a',
      shell: {
        avatarColor: '#64748b',
        displayName: 'Agent A',
        isUser: false,
      },
      blocks: [],
      tailContent: 'base!',
      tailBlock: {
        type: 'markdown',
        content: 'base!',
        hash: 'base-next',
        render_mode: 'ast',
      },
      tailFrame: {
        streamId: 3,
        epoch: 1,
        revision: 2,
        frameSeq: 2,
        reset: true,
        snapshot: [{
          type: 'paragraph',
          children: [{ type: 'text', value: 'base' }],
        }],
        mutations: [{ op: 'append', id: 't0.i0', chunk: '!' }],
      },
    });

    const wrapper = mount(MessageRenderer, {
      props: { message },
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
    await nextTick();
    expect(wrapper.get('.vcp-ast-sandbox').text()).toContain('base!');
    wrapper.unmount();
  });

  it('reveals complete AST appends immediately and settles at lifecycle boundaries', async () => {
    const restoreAnimations = installRunningAnimationMock();
    const pinia = createPinia();
    setActivePinia(pinia);
    const sessionStore = useChatSessionStore();
    const streamStore = useChatStreamStore();
    const chatPreferences = useChatPreferencesStore();
    sessionStore.setConversation({
      id: 'agent-a',
      type: 'agent',
      name: 'Agent A',
    } as any, 'topic-a');
    streamStore.addSessionStream('agent-a', 'agent', 'topic-a', 'smooth-append');
    chatPreferences.setSmoothStreamingEnabled(true);

    const message = reactive<ChatMessage>({
      id: 'smooth-append',
      role: 'assistant',
      timestamp: 1,
      agentId: 'agent-a',
      shell: {
        avatarColor: '#64748b',
        displayName: 'Agent A',
        isUser: false,
      },
      blocks: [],
      tailContent: 'base',
      tailBlock: {
        type: 'markdown',
        content: 'base',
        hash: 'base',
        render_mode: 'ast',
      },
      tailFrame: {
        streamId: 4,
        epoch: 1,
        revision: 1,
        frameSeq: 1,
        reset: true,
        snapshot: [{
          type: 'paragraph',
          children: [{ type: 'text', value: 'base' }],
        }],
        mutations: [],
      },
    });
    const wrapper = mountStreamRenderer(message, pinia);

    try {
      await nextTick();
      await nextTick();
      message.tailBlock = {
      type: 'markdown',
      content: 'base完整新增',
      hash: 'append-1',
      render_mode: 'ast',
    };
    message.tailFrame = {
      streamId: 4,
      epoch: 1,
      revision: 2,
      frameSeq: 2,
      mutations: [{ op: 'append', id: 't0.i0', chunk: '完整新增' }],
    };
    await nextTick();
    await nextTick();

    expect(wrapper.get('.vcp-ast-sandbox').text()).toBe('base完整新增');
    expect(wrapper.find('[data-vcp-stream-fragment]').exists()).toBe(true);

    await wrapper.setProps({ isBackground: true });
    expect(wrapper.find('[data-vcp-stream-fragment]').exists()).toBe(false);
    expect(wrapper.get('.vcp-ast-sandbox').text()).toBe('base完整新增');

    await wrapper.setProps({ isBackground: false });
    message.tailBlock = {
      type: 'markdown',
      content: 'base完整新增继续',
      hash: 'append-2',
      render_mode: 'ast',
    };
    message.tailFrame = {
      streamId: 4,
      epoch: 1,
      revision: 3,
      frameSeq: 3,
      mutations: [{ op: 'append', id: 't0.i0', chunk: '继续' }],
    };
    await nextTick();
    await nextTick();
    expect(wrapper.find('[data-vcp-stream-fragment]').exists()).toBe(true);

    chatPreferences.setSmoothStreamingEnabled(false);
    await nextTick();
    expect(wrapper.find('[data-vcp-stream-fragment]').exists()).toBe(false);
    expect(wrapper.get('.vcp-ast-sandbox').text()).toBe('base完整新增继续');

    message.content = '严格终态';
    message.blocks = [{
      type: 'markdown',
      content: '严格终态',
      nodes: [{
        type: 'paragraph',
        children: [{ type: 'text', value: '严格终态' }],
      }],
    }];
    message.tailBlock = undefined;
    message.tailContent = '';
    streamStore.removeSessionStream('agent-a', 'agent', 'topic-a', message.id);
    await nextTick();
    await nextTick();

      expect(wrapper.text()).toContain('严格终态');
      expect(wrapper.find('[data-vcp-stream-fragment]').exists()).toBe(false);
    } finally {
      wrapper.unmount();
      restoreAnimations();
    }
  });

  it('reveals only stable Markdown blocks that were not already visible as the tail', async () => {
    const restoreAnimations = installRunningAnimationMock();
    const pinia = createPinia();
    setActivePinia(pinia);
    const sessionStore = useChatSessionStore();
    const streamStore = useChatStreamStore();
    const chatPreferences = useChatPreferencesStore();
    sessionStore.setConversation({
      id: 'agent-a',
      type: 'agent',
      name: 'Agent A',
    } as any, 'topic-a');
    streamStore.addSessionStream('agent-a', 'agent', 'topic-a', 'stable-reveal');
    chatPreferences.setSmoothStreamingEnabled(true);

    const message = reactive<ChatMessage>({
      id: 'stable-reveal',
      role: 'assistant',
      timestamp: 1,
      agentId: 'agent-a',
      shell: {
        avatarColor: '#64748b',
        displayName: 'Agent A',
        isUser: false,
      },
      blocks: [],
      tailContent: '第一段',
      tailBlock: {
        type: 'markdown',
        content: '第一段',
        hash: 'tail-first',
        render_mode: 'ast',
      },
      tailFrame: {
        streamId: 5,
        epoch: 1,
        revision: 1,
        frameSeq: 1,
        reset: true,
        snapshot: [{
          type: 'paragraph',
          children: [{ type: 'text', value: '第一段' }],
        }],
        mutations: [],
      },
    });
    const wrapper = mountStreamRenderer(message, pinia);

    try {
      await nextTick();
      await nextTick();
      message.blocks = [
        {
          type: 'markdown',
          content: '第一段',
          hash: 'stable-first',
          nodes: [{
            type: 'paragraph',
            children: [{ type: 'text', value: '第一段' }],
          }],
        },
        {
          type: 'markdown',
          content: '第二段',
          hash: 'stable-second',
          nodes: [{
            type: 'paragraph',
            children: [{ type: 'text', value: '第二段' }],
          }],
        },
      ];
      message.tailFrame = {
        streamId: 5,
        epoch: 2,
        revision: 0,
        frameSeq: 2,
        reset: true,
        snapshot: [],
        mutations: [],
      };
      message.tailContent = '';
      message.tailBlock = undefined;
      await nextTick();
      await nextTick();

      const stableBlocks = wrapper.findAll('.vcp-content-blocks > div');
      expect(stableBlocks).toHaveLength(2);
      expect(stableBlocks[0].classes()).not.toContain('vcp-stream-element-reveal');
      expect(stableBlocks[1].classes()).toContain('vcp-stream-element-reveal');
      expect(wrapper.text()).toContain('第一段');
      expect(wrapper.text()).toContain('第二段');

      stableBlocks[1].element.dispatchEvent(new Event('animationend'));
      expect(stableBlocks[1].classes()).not.toContain('vcp-stream-element-reveal');

      message.tailContent = '按钮拆分前的旧尾部';
      message.tailBlock = {
        type: 'markdown',
        content: '按钮拆分前的旧尾部',
        hash: 'button-tail',
        render_mode: 'ast',
        nodes: [{
          type: 'paragraph',
          children: [{ type: 'text', value: '按钮拆分前的旧尾部' }],
        }],
      };
      message.tailFrame = {
        streamId: 5,
        epoch: 2,
        revision: 1,
        frameSeq: 3,
        mutations: [],
      };
      await nextTick();
      await nextTick();

      message.blocks = [
        ...message.blocks!,
        {
          type: 'markdown',
          content: '按钮前',
          hash: 'button-before',
          nodes: [{ type: 'paragraph', children: [{ type: 'text', value: '按钮前' }] }],
        },
        { type: 'button-click', content: '确认', hash: 'button-middle' },
        {
          type: 'markdown',
          content: '按钮后',
          hash: 'button-after',
          nodes: [{ type: 'paragraph', children: [{ type: 'text', value: '按钮后' }] }],
        },
      ];
      message.tailFrame = {
        streamId: 5,
        epoch: 3,
        revision: 0,
        frameSeq: 4,
        reset: true,
        snapshot: [],
        mutations: [],
      };
      message.tailContent = '';
      message.tailBlock = undefined;
      await nextTick();
      await nextTick();

      expect(wrapper.findAll('.vcp-stream-element-reveal')).toHaveLength(0);
    } finally {
      wrapper.unmount();
      restoreAnimations();
    }
  });

  it('starts a fresh same-batch stable reveal simultaneously for eligible blocks', async () => {
    const restoreAnimations = installRunningAnimationMock();
    const pinia = createPinia();
    setActivePinia(pinia);
    const sessionStore = useChatSessionStore();
    const streamStore = useChatStreamStore();
    const chatPreferences = useChatPreferencesStore();
    sessionStore.setConversation({
      id: 'agent-a',
      type: 'agent',
      name: 'Agent A',
    } as any, 'topic-a');
    streamStore.addSessionStream('agent-a', 'agent', 'topic-a', 'fresh-stable-reveal');
    chatPreferences.setSmoothStreamingEnabled(true);

    const message = reactive<ChatMessage>({
      id: 'fresh-stable-reveal',
      role: 'assistant',
      timestamp: 1,
      agentId: 'agent-a',
      shell: {
        avatarColor: '#64748b',
        displayName: 'Agent A',
        isUser: false,
      },
      blocks: [],
      tailContent: '',
      tailFrame: {
        streamId: 6,
        epoch: 1,
        revision: 0,
        frameSeq: 1,
        reset: true,
        snapshot: [],
        mutations: [],
      },
    });
    const wrapper = mountStreamRenderer(message, pinia);

    try {
      await nextTick();
      message.blocks = [
        {
          type: 'markdown',
          content: '甲',
          hash: 'fresh-a',
          nodes: [{ type: 'paragraph', children: [{ type: 'text', value: '甲' }] }],
        },
        {
          type: 'markdown',
          content: '乙',
          hash: 'fresh-b',
          nodes: [{ type: 'heading', level: 2, children: [{ type: 'text', value: '乙' }] }],
        },
        {
          type: 'markdown',
          content: '```ts\nconst x = 1\n```',
          hash: 'fresh-code',
          nodes: [{
            type: 'code_block',
            lang: 'ts',
            code: 'const x = 1',
            highlighted_html: null,
            theme: null,
          }],
        },
      ];
      message.tailFrame = {
        streamId: 6,
        epoch: 2,
        revision: 0,
        frameSeq: 2,
        reset: true,
        snapshot: [],
        mutations: [],
      };
      await nextTick();
      await nextTick();

      const stableBlocks = wrapper.findAll('.vcp-content-blocks > div');
      expect(stableBlocks).toHaveLength(3);
      expect(stableBlocks[0].classes()).toContain('vcp-stream-element-reveal');
      expect(stableBlocks[1].classes()).toContain('vcp-stream-element-reveal');
      expect(stableBlocks[2].classes()).not.toContain('vcp-stream-element-reveal');
    } finally {
      wrapper.unmount();
      restoreAnimations();
    }
  });

  it('requests one canonical snapshot when an AST tail remounts after earlier deltas', async () => {
    const pinia = createPinia();
    setActivePinia(pinia);
    const sessionStore = useChatSessionStore();
    const streamStore = useChatStreamStore();
    sessionStore.setConversation({
      id: 'agent-a',
      type: 'agent',
      name: 'Agent A',
    } as any, 'topic-a');

    const context = {
      ownerId: 'agent-a',
      ownerType: 'agent' as const,
      topicId: 'topic-a',
      agentId: 'agent-a',
    };
    const eventBase = {
      chunk: null,
      finishReason: null,
      error: null,
      blocks: null,
      timestamp: null,
      topicUpdatedAt: null,
    };
    await streamStore.processStreamEvent({
      consumerRevision: 1,
      ...eventBase,
      type: 'thinking',
      messageId: 'remount-message',
      context,
      aurora: null,
    });

    let fullContent = '';
    for (let frameSeq = 1; frameSeq <= 5; frameSeq += 1) {
      const chunk = String.fromCharCode(96 + frameSeq);
      fullContent += chunk;
      await streamStore.processStreamEvent({
        consumerRevision: 1,
        ...eventBase,
        type: 'aurora',
        messageId: 'remount-message',
        context,
        aurora: {
          kind: 'delta',
          streamId: 9,
          chunk,
          tailOp: {
            op: 'replace',
            content: fullContent,
            hash: `tail-${frameSeq}`,
            mode: 'ast',
            blockType: 'markdown',
          },
          tailFrame: {
            streamId: 9,
            epoch: 1,
            revision: frameSeq,
            frameSeq,
            mutations: frameSeq === 1
              ? [{
                  op: 'add',
                  id: 't0',
                  parent: 'root',
                  node: {
                    type: 'paragraph',
                    children: [{ type: 'text', value: chunk }],
                  },
                }]
              : [{ op: 'append', id: 't0.i0', chunk }],
          },
        },
      });
    }

    await vi.waitFor(() => {
      expect(streamStore.getActiveStreamMessage(
        'agent-a',
        'agent',
        'topic-a',
        'remount-message',
      )?.tailFrame?.frameSeq).toBe(5);
    });
    mockInvoke('rebuild_aurora_snapshot', (args) => {
      const content = String(args?.content ?? '');
      return {
        stableBlocks: [],
        tailBlock: { type: 'markdown', content, hash: 'snapshot' },
        tailMode: 'ast',
        tailSnapshot: [{
          type: 'paragraph',
          children: [{ type: 'text', value: content }],
        }],
      };
    });

    const message = streamStore.getActiveStreamMessage(
      'agent-a',
      'agent',
      'topic-a',
      'remount-message',
    )!;
    const wrapper = mount(MessageRenderer, {
      props: { message },
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

    await vi.waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith(
        'rebuild_aurora_snapshot',
        { content: 'abcde' },
      );
      expect(wrapper.get('.vcp-ast-sandbox').text()).toContain('abcde');
    });
    expect(invokeMock.mock.calls.filter(
      ([command]) => command === 'rebuild_aurora_snapshot',
    )).toHaveLength(1);
    wrapper.unmount();
  });

  it('downgrades to the canonical text fallback after repeated snapshot failures', async () => {
    const pinia = createPinia();
    setActivePinia(pinia);
    const sessionStore = useChatSessionStore();
    const streamStore = useChatStreamStore();
    sessionStore.setConversation({
      id: 'agent-a',
      type: 'agent',
      name: 'Agent A',
    } as any, 'topic-a');
    const seeded = await seedRemountTail(streamStore, 'failed-recovery', 30);
    mockInvoke('rebuild_aurora_snapshot', () => Promise.reject(new Error('offline')));

    const wrapper = mount(MessageRenderer, {
      props: { message: seeded.message },
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

    await vi.waitFor(() => {
      expect(invokeMock.mock.calls.filter(
        ([command]) => command === 'rebuild_aurora_snapshot',
      )).toHaveLength(2);
      expect(wrapper.get('.streaming-tail').text()).toContain('abcde');
    });
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(invokeMock.mock.calls.filter(
      ([command]) => command === 'rebuild_aurora_snapshot',
    )).toHaveLength(2);
    wrapper.unmount();
  });

  it('prevents an unmounted recovery callback from clearing a new renderer registry', async () => {
    const pinia = createPinia();
    setActivePinia(pinia);
    const sessionStore = useChatSessionStore();
    const streamStore = useChatStreamStore();
    sessionStore.setConversation({
      id: 'agent-a',
      type: 'agent',
      name: 'Agent A',
    } as any, 'topic-a');
    const seeded = await seedRemountTail(streamStore, 'recovery-owner', 40);
    const delayedFailure = deferred<never>();
    let recoveryAttempt = 0;
    mockInvoke('rebuild_aurora_snapshot', () => {
      recoveryAttempt += 1;
      if (recoveryAttempt === 1) return Promise.reject(new Error('first failure'));
      return delayedFailure.promise;
    });

    const mountRenderer = () => mount(MessageRenderer, {
      props: { message: seeded.message },
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
    const oldWrapper = mountRenderer();
    await vi.waitFor(() => {
      expect(invokeMock.mock.calls.filter(
        ([command]) => command === 'rebuild_aurora_snapshot',
      )).toHaveLength(2);
    });
    oldWrapper.unmount();

    seeded.message.tailBlock = {
      ...seeded.message.tailBlock!,
      nodes: [{
        type: 'paragraph',
        children: [{ type: 'text', value: 'abcde' }],
      }],
    };
    const newWrapper = mountRenderer();
    await nextTick();
    await nextTick();
    expect(newWrapper.get('.vcp-ast-sandbox').text()).toContain('abcde');

    delayedFailure.reject(new Error('late failure'));
    await new Promise((resolve) => setTimeout(resolve, 0));
    seeded.message.tailContent = 'abcde!';
    seeded.message.tailBlock = {
      type: 'markdown',
      content: 'abcde!',
      hash: 'recovery-owner-6',
      render_mode: 'ast',
      nodes: seeded.message.tailBlock.nodes,
    };
    seeded.message.tailFrame = {
      streamId: 40,
      epoch: 1,
      revision: 6,
      frameSeq: 6,
      mutations: [{ op: 'append', id: 't0.i0', chunk: '!' }],
    };
    await nextTick();
    await nextTick();

    expect(newWrapper.get('.vcp-ast-sandbox').text()).toContain('abcde!');
    expect(invokeMock.mock.calls.filter(
      ([command]) => command === 'rebuild_aurora_snapshot',
    )).toHaveLength(2);
    newWrapper.unmount();
  });

  it('does not downgrade a replacement owner when its predecessor cancels a second recovery', async () => {
    const pinia = createPinia();
    setActivePinia(pinia);
    const stream = useChatStreamStore();
    useChatSessionStore().setConversation({ id: 'agent-a', type: 'agent' }, 'topic-a');
    const seeded = await seedRemountTail(stream, 'consumer-replacement', 40);
    const obsolete = deferred<never>();
    let attempts = 0;
    mockInvoke('rebuild_aurora_snapshot', () => ++attempts === 1
      ? Promise.reject(new Error('first failure')) : obsolete.promise);
    const wrapper = mountStreamRenderer(seeded.message, pinia);
    try {
      await vi.waitFor(() => expect(attempts).toBe(2));
      await stream.processStreamEvent({
        ...seeded.eventBase, type: 'connected', consumerRevision: 2, messageId: seeded.message.id,
        context: seeded.context, aurora: null,
      });
      await nextTick();
      await new Promise(resolve => setTimeout(resolve, 0));
      expect(attempts).toBe(2);
      expect(wrapper.get('.streaming-tail').text()).toContain('abcde');
      await stream.processStreamEvent({
        ...seeded.eventBase, type: 'aurora', consumerRevision: 2, messageId: seeded.message.id,
        context: seeded.context,
        aurora: {
          kind: 'snapshot', streamId: 41, content: 'new', stableBlocks: [], tailMode: 'ast',
          tailBlock: { type: 'markdown', content: 'new', hash: 'new' },
          tailFrame: { streamId: 41, epoch: 1, revision: 1, frameSeq: 1, reset: true,
            snapshot: [{ type: 'paragraph', children: [{ type: 'text', value: 'new' }] }], mutations: [] },
        },
      });
      await vi.waitFor(() => expect(wrapper.get('.vcp-ast-sandbox').text()).toBe('new'));
      obsolete.reject(new Error('obsolete command failure'));
      await stream.processStreamEvent({
        ...seeded.eventBase, type: 'aurora', consumerRevision: 2, messageId: seeded.message.id,
        context: seeded.context,
        aurora: {
          kind: 'delta', streamId: 41, chunk: '!',
          tailOp: { op: 'append', previousHash: 'new', content: '!', hash: 'new!', mode: 'ast', blockType: 'markdown' },
          tailFrame: { streamId: 41, epoch: 1, revision: 2, frameSeq: 2,
            mutations: [{ op: 'append', id: 't0.i0', chunk: '!' }] },
        },
      });
      await vi.waitFor(() => expect(wrapper.get('.vcp-ast-sandbox').text()).toBe('new!'));
      expect(attempts).toBe(2);
    } finally {
      wrapper.unmount();
      stream.$dispose();
    }
  });

  it('renders an AST-less streaming tail as literal plaintext', async () => {
    const pinia = createPinia();
    setActivePinia(pinia);
    const sessionStore = useChatSessionStore();
    const streamStore = useChatStreamStore();
    sessionStore.setConversation({
      id: 'agent-a',
      type: 'agent',
      name: 'Agent A',
    } as any, 'topic-a');
    streamStore.addSessionStream('agent-a', 'agent', 'topic-a', 'plain-tail-message');

    const literalTail = '<div>unfinished\n<script>alert("x")</script>';
    const message = reactive<ChatMessage>({
      id: 'plain-tail-message',
      role: 'assistant',
      timestamp: 1,
      agentId: 'agent-a',
      shell: {
        avatarColor: '#64748b',
        displayName: 'Agent A',
        isUser: false,
      },
      blocks: [],
      tailContent: literalTail,
      tailBlock: {
        type: 'markdown',
        content: literalTail,
        hash: 'plain-tail',
      },
    });

    const wrapper = mount(MessageRenderer, {
      props: { message },
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

    const plaintext = wrapper.get('[data-tail-render-mode="plaintext"]');
    expect(plaintext.text()).toBe(literalTail);
    expect(plaintext.find('script').exists()).toBe(false);
    expect(wrapper.find('.vcp-ast-sandbox').exists()).toBe(false);
    wrapper.unmount();
  });

  it('renders an HTML tail in the preview shell while PatchCode retains the code DOM', async () => {
    const pinia = createPinia();
    setActivePinia(pinia);
    const sessionStore = useChatSessionStore();
    const streamStore = useChatStreamStore();
    sessionStore.setConversation({
      id: 'agent-a',
      type: 'agent',
      name: 'Agent A',
    } as any, 'topic-a');
    const context = {
      ownerId: 'agent-a',
      ownerType: 'agent' as const,
      topicId: 'topic-a',
      agentId: 'agent-a',
    };
    const eventBase = {
      chunk: null,
      finishReason: null,
      error: null,
      blocks: null,
      timestamp: null,
      topicUpdatedAt: null,
    };
    await streamStore.processStreamEvent({
      consumerRevision: 1,
      ...eventBase,
      type: 'thinking',
      messageId: 'html-tail-message',
      context,
      aurora: null,
    });
    await streamStore.processStreamEvent({
      consumerRevision: 1,
      ...eventBase,
      type: 'aurora',
      messageId: 'html-tail-message',
      context,
      aurora: {
        kind: 'delta',
        streamId: 31,
        chunk: '```html\n<main>streaming',
        tailOp: {
          op: 'replace',
          content: '```html\n<main>streaming',
          hash: 'html-tail',
          mode: 'ast',
          blockType: 'html-preview',
        },
        tailFrame: {
          streamId: 31,
          epoch: 1,
          revision: 1,
          frameSeq: 1,
          reset: true,
          snapshot: [{
            type: 'code_block',
            lang: 'html',
            code: '<main>streaming',
            highlighted_html: '<pre class="vcp-code-block"><code data-vcp-stream-code><span data-vcp-code-stable></span><span data-vcp-code-active>&lt;main&gt;streaming</span></code></pre>',
            theme: null,
          }],
          mutations: [],
        },
      },
    });
    await vi.waitFor(() => {
      expect(streamStore.getActiveStreamMessage(
        'agent-a', 'agent', 'topic-a', 'html-tail-message',
      )?.tailBlock?.type).toBe('html-preview');
    });
    const message = streamStore.getActiveStreamMessage(
      'agent-a', 'agent', 'topic-a', 'html-tail-message',
    )!;

    const wrapper = mount(MessageRenderer, {
      props: { message },
      global: {
        plugins: [pinia],
        directives: { longpress: {} },
        stubs: {
          VcpAvatar: markerStub('avatar'),
          ToolBlock: markerStub('tool'),
          ThoughtBlock: markerStub('thought'),
          ToolSummaryBlock: markerStub('tool-summary'),
          DiaryBlock: markerStub('diary'),
          AttachmentPreview: markerStub('attachment'),
          MermaidFullScreenViewer: markerStub('mermaid-viewer'),
          ThinkingIndicator: markerStub('thinking'),
          StreamingTag: markerStub('streaming'),
        },
      },
    });

    await vi.waitFor(() => {
      expect(wrapper.get('.html-preview-block').text()).toContain('<main>streaming');
    });
    expect(wrapper.find('.vcp-ast-sandbox [data-vcp-stream-code]').exists()).toBe(true);
    expect(wrapper.find('iframe').exists()).toBe(false);
    expect(wrapper.text()).not.toContain('代码');
    expect(wrapper.text()).not.toContain('预览');
    expect(wrapper.findAll('.html-preview-block button')).toHaveLength(2);
    expect(wrapper.findAll('.html-preview-block button').every(
      (button) => button.attributes('disabled') !== undefined,
    )).toBe(true);
    wrapper.unmount();
  });

  it('keeps one thought shell while applying tail AST deltas, then hands off atomically', async () => {
    const pinia = createPinia();
    setActivePinia(pinia);
    const sessionStore = useChatSessionStore();
    const streamStore = useChatStreamStore();
    useChatPreferencesStore().setSmoothStreamingEnabled(true);
    sessionStore.setConversation({
      id: 'agent-a',
      type: 'agent',
      name: 'Agent A',
    } as any, 'topic-a');
    const context = {
      ownerId: 'agent-a',
      ownerType: 'agent' as const,
      topicId: 'topic-a',
      agentId: 'agent-a',
    };
    const eventBase = {
      finishReason: null,
      error: null,
      blocks: null,
      timestamp: null,
      topicUpdatedAt: null,
    };

    await streamStore.processStreamEvent({
      consumerRevision: 1,
      ...eventBase,
      type: 'thinking',
      messageId: 'thought-tail-message',
      context,
      chunk: null,
      aurora: null,
    });
    await streamStore.processStreamEvent({
      consumerRevision: 1,
      ...eventBase,
      type: 'aurora',
      messageId: 'thought-tail-message',
      context,
      chunk: null,
      aurora: {
        kind: 'delta',
        streamId: 41,
        chunk: '<think>分析',
        tailOp: {
          op: 'replace',
          content: '分析',
          hash: 'thought-tail-1',
          mode: 'ast',
          blockType: 'thought',
          thoughtTheme: '思维链',
        },
        tailFrame: {
          streamId: 41,
          epoch: 1,
          revision: 1,
          frameSeq: 1,
          reset: true,
          snapshot: [{
            type: 'paragraph',
            children: [{ type: 'text', value: '分析' }],
          }],
          mutations: [],
        },
      },
    });
    await vi.waitFor(() => {
      expect(streamStore.getActiveStreamMessage(
        'agent-a', 'agent', 'topic-a', 'thought-tail-message',
      )?.tailBlock?.type).toBe('thought');
    });
    const message = streamStore.getActiveStreamMessage(
      'agent-a', 'agent', 'topic-a', 'thought-tail-message',
    )!;
    expect(message.tailBlock).toMatchObject({
      type: 'thought',
      theme: '思维链',
      content: '分析',
      is_complete: false,
    });

    const wrapper = mount(MessageRenderer, {
      props: { message },
      global: {
        plugins: [pinia],
        directives: { longpress: {} },
        stubs: {
          VcpAvatar: markerStub('avatar'),
          ToolBlock: markerStub('tool'),
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

    await vi.waitFor(() => {
      expect(wrapper.get('.vcp-thought-block').text()).toContain('分析');
    });
    const thoughtShell = wrapper.get('.vcp-thought-block').element;
    const tailSandbox = wrapper.get('.vcp-thought-block .vcp-ast-sandbox').element;
    expect(wrapper.get('.vcp-thought-block').classes()).not.toContain('vcp-stream-element-reveal');
    expect(wrapper.get('.vcp-thought-content').classes()).not.toContain('animate-slide-down');
    expect(wrapper.text()).not.toContain('<think>');
    expect(wrapper.find('.custom-spin').exists()).toBe(false);

    await streamStore.processStreamEvent({
      consumerRevision: 1,
      ...eventBase,
      type: 'aurora',
      messageId: 'thought-tail-message',
      context,
      chunk: null,
      aurora: {
        kind: 'delta',
        streamId: 41,
        chunk: '继续',
        tailOp: {
          op: 'append',
          previousHash: 'thought-tail-1',
          content: '继续',
          hash: 'thought-tail-2',
          mode: 'ast',
          blockType: 'thought',
          thoughtTheme: '思维链',
        },
        tailFrame: {
          streamId: 41,
          epoch: 1,
          revision: 2,
          frameSeq: 2,
          mutations: [{ op: 'append', id: 't0.i0', chunk: '继续' }],
        },
      },
    });
    await vi.waitFor(() => {
      expect(wrapper.get('.vcp-thought-block').text()).toContain('分析继续');
    });
    expect(wrapper.get('.vcp-thought-block').element).toBe(thoughtShell);
    expect(wrapper.get('.vcp-thought-block .vcp-ast-sandbox').element).toBe(tailSandbox);

    await streamStore.processStreamEvent({
      consumerRevision: 1,
      ...eventBase,
      type: 'aurora',
      messageId: 'thought-tail-message',
      context,
      chunk: null,
      aurora: {
        kind: 'delta',
        streamId: 41,
        chunk: '</think>',
        stableAppend: {
          baseCount: 0,
          blocks: [{
            type: 'thought',
            theme: '思维链',
            content: '分析继续',
            is_complete: true,
            hash: 'stable-thought',
            nodes: [{
              type: 'paragraph',
              children: [{ type: 'text', value: '分析继续' }],
            }],
          }],
        },
        tailOp: { op: 'clear' },
        tailFrame: {
          streamId: 41,
          epoch: 2,
          revision: 0,
          frameSeq: 3,
          reset: true,
          snapshot: [],
          mutations: [],
        },
      },
    });
    await vi.waitFor(() => {
      expect(message.tailBlock).toBeUndefined();
      expect(message.blocks).toHaveLength(1);
      expect(wrapper.findAll('.vcp-thought-block')).toHaveLength(1);
    });
    expect(wrapper.get('.vcp-thought-block').text()).toContain('分析继续');
    expect(wrapper.get('.vcp-thought-block').classes()).not.toContain('vcp-stream-element-reveal');
    wrapper.unmount();
  });

  it('思维链 Plain 模式下使用底层 DOM appendData 增量追加且闭合时沉淀', async () => {
    const pinia = createPinia();
    setActivePinia(pinia);
    const sessionStore = useChatSessionStore();
    const streamStore = useChatStreamStore();
    sessionStore.setConversation({
      id: 'agent-a',
      type: 'agent',
      name: 'Agent A',
    } as any, 'topic-a');
    const context = {
      ownerId: 'agent-a',
      ownerType: 'agent' as const,
      topicId: 'topic-a',
      agentId: 'agent-a',
    };
    const eventBase = {
      finishReason: null,
      error: null,
      blocks: null,
      timestamp: null,
      topicUpdatedAt: null,
    };

    await streamStore.processStreamEvent({
      consumerRevision: 1,
      ...eventBase,
      type: 'thinking',
      messageId: 'thought-plain-message',
      context,
      chunk: null,
      aurora: null,
    });

    await streamStore.processStreamEvent({
      consumerRevision: 1,
      ...eventBase,
      type: 'aurora',
      messageId: 'thought-plain-message',
      context,
      chunk: null,
      aurora: {
        kind: 'delta',
        streamId: 42,
        chunk: '<think>分析第一阶段',
        tailOp: {
          op: 'replace',
          content: '分析第一阶段',
          hash: 'thought-p1',
          mode: 'plain',
          blockType: 'thought',
          thoughtTheme: '思维链',
        },
      },
    });

    const message = streamStore.getActiveStreamMessage(
      'agent-a', 'agent', 'topic-a', 'thought-plain-message',
    )!;

    const wrapper = mount(MessageRenderer, {
      props: { message },
      global: {
        plugins: [pinia],
        directives: { longpress: {} },
        stubs: {
          VcpAvatar: markerStub('avatar'),
          ToolBlock: markerStub('tool'),
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

    await vi.waitFor(() => {
      expect(wrapper.get('.vcp-thought-block').text()).toContain('分析第一阶段');
    });

    const appendDataSpy = vi.spyOn(CharacterData.prototype, 'appendData');

    await streamStore.processStreamEvent({
      consumerRevision: 1,
      ...eventBase,
      type: 'aurora',
      messageId: 'thought-plain-message',
      context,
      chunk: null,
      aurora: {
        kind: 'delta',
        streamId: 42,
        chunk: ' -> 深入推理',
        tailOp: {
          op: 'append',
          previousHash: 'thought-p1',
          content: ' -> 深入推理',
          hash: 'thought-p2',
          mode: 'plain',
          blockType: 'thought',
          thoughtTheme: '思维链',
        },
      },
    });

    await vi.waitFor(() => {
      expect(appendDataSpy).toHaveBeenCalledWith(' -> 深入推理');
      expect(wrapper.get('.vcp-thought-block').text()).toContain('分析第一阶段 -> 深入推理');
    });

    appendDataSpy.mockRestore();

    await streamStore.processStreamEvent({
      consumerRevision: 1,
      ...eventBase,
      type: 'aurora',
      messageId: 'thought-plain-message',
      context,
      chunk: null,
      aurora: {
        kind: 'delta',
        streamId: 42,
        chunk: '</think>',
        stableAppend: {
          baseCount: 0,
          blocks: [{
            type: 'thought',
            theme: '思维链',
            content: '分析第一阶段 -> 深入推理',
            is_complete: true,
            hash: 'stable-thought-p',
            nodes: [{
              type: 'paragraph',
              children: [{ type: 'text', value: '分析第一阶段 -> 深入推理' }],
            }],
          }],
        },
        tailOp: { op: 'clear' },
      },
    });

    await vi.waitFor(() => {
      expect(message.tailBlock).toBeUndefined();
      expect(message.blocks).toHaveLength(1);
      expect(wrapper.findAll('.vcp-thought-block')).toHaveLength(1);
      expect(wrapper.get('.vcp-thought-block').text()).toContain('分析第一阶段 -> 深入推理');
    });

    wrapper.unmount();
  });
});

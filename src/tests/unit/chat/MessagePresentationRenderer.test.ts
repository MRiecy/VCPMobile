import { defineComponent, nextTick, reactive, ref } from 'vue';
import { mount } from '@vue/test-utils';
import { createPinia, setActivePinia, type Pinia } from 'pinia';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import MessageRenderer from '@/features/chat/MessageRenderer.vue';
import type { ChatMessage } from '@/core/types/chat';
import { useChatSessionStore } from '@/core/stores/chatSessionStore';
import { useChatStreamStore } from '@/core/stores/chatStreamStore';
import { useThemeStore, type ChatPresentationMode } from '@/core/stores/theme';
import { invokeMock, mockInvoke } from '@/tests/mocks/tauri';

const { mermaidRenderMock } = vi.hoisted(() => ({
  mermaidRenderMock: vi.fn(async () => ({ svg: '<svg viewBox="0 0 10 10"></svg>' })),
}));

vi.mock('mermaid', () => ({
  default: {
    initialize: vi.fn(),
    render: mermaidRenderMock,
  },
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
  };
}

beforeEach(() => {
  localStorage.removeItem('vcp-smooth-streaming-enabled');
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
        {
          type: 'thought',
          theme: 'default',
          content: 'reasoning fixture',
          is_complete: true,
        },
        { type: 'html-preview', content: '<main>preview fixture</main>' },
        {
          type: 'tool-call-summary',
          items: [{ tool_name: 'Search', status: 'success' }],
          raw_content: 'Search: success',
        },
        { type: 'diary', maid: 'Fixture Agent', date: '', content: 'diary fixture' },
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
    const themeStore = useThemeStore();
    sessionStore.setConversation({
      id: 'agent-a',
      type: 'agent',
      name: 'Agent A',
    } as any, 'topic-a');
    streamStore.addSessionStream('agent-a', 'agent', 'topic-a', 'smooth-append');
    themeStore.setSmoothStreamingEnabled(true);

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

    themeStore.setSmoothStreamingEnabled(false);
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
      ...eventBase,
      type: 'thinking',
      messageId: 'html-tail-message',
      context,
      aurora: null,
    });
    await streamStore.processStreamEvent({
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
    useThemeStore().setSmoothStreamingEnabled(true);
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
      ...eventBase,
      type: 'thinking',
      messageId: 'thought-tail-message',
      context,
      chunk: null,
      aurora: null,
    });
    await streamStore.processStreamEvent({
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
    expect(wrapper.get('.vcp-thought-block').classes()).toContain('vcp-stream-element-fade-in');
    expect(wrapper.get('.vcp-thought-content').classes()).not.toContain('animate-slide-down');
    expect(wrapper.text()).not.toContain('<think>');
    expect(wrapper.find('.custom-spin').exists()).toBe(false);

    await streamStore.processStreamEvent({
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
    expect(wrapper.get('.vcp-thought-block').classes()).not.toContain('vcp-stream-element-fade-in');
    wrapper.unmount();
  });
});

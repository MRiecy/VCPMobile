// Audit reproduction, not a product regression gate. Run from the repository root:
// node --input-type=module -e "import { startVitest } from 'vitest/node'; const ctx = await startVitest('test', ['plan/sse-lifecycle-dimension-2-audit/probes/eos_identity_probe.test.ts'], { watch: false, include: ['plan/sse-lifecycle-dimension-2-audit/probes/eos_identity_probe.test.ts'] }); await ctx?.close();"
// Fixtures model the inspected Rust StreamBlock/ContentBlock wire contracts.
// This does not execute the Rust parsers or measure browser layout/GPU behavior.
import { describe, expect, it, vi } from 'vitest';
import { nextTick } from 'vue';
import { mount } from '@vue/test-utils';
import { createPinia, disposePinia, setActivePinia } from 'pinia';
import MessageRenderer from '@/features/chat/MessageRenderer.vue';
import { useChatSessionStore } from '@/core/stores/chatSessionStore';
import { useChatStreamStore } from '@/core/stores/chatStreamStore';
import type { ContentBlock, StreamEventDto } from '@/core/types/chat';

vi.mock('pele', () => ({ render: vi.fn(() => ({ svg: '<svg></svg>' })) }));

const context = { ownerId: 'audit-agent', ownerType: 'agent' as const, topicId: 'audit-topic' };
const content = '| Header |\n| --- |\n| Body |\n\n';
const nodes = [{
  type: 'table' as const,
  header: [[{ type: 'text' as const, value: 'Header' }]],
  rows: [[[{ type: 'text' as const, value: 'Body' }]]],
  wrapper_class: 'vcp-scrollable no-swipe',
}];

describe('EOS identity audit reproduction', () => {
  for (const mode of ['status-only', 'new-block-same-key', 'terminal-hash-switch'] as const) {
    it(mode, async () => {
      const pinia = createPinia();
      setActivePinia(pinia);
      useChatSessionStore().setConversation({ id: context.ownerId, type: 'agent' }, context.topicId);
      const store = useChatStreamStore();
      const messageId = `eos-audit-${mode}`;
      const event = (update: Partial<StreamEventDto>): StreamEventDto => ({
        consumerRevision: 1, messageId, context, type: 'aurora', chunk: null,
        finishReason: null, error: null, aurora: null, blocks: null,
        timestamp: null, topicUpdatedAt: null, ...update,
      });
      await store.processStreamEvent(event({
        aurora: {
          kind: 'snapshot', streamId: 1, content,
          stableBlocks: [{ type: 'markdown', content, hash: 'abcdef0123456789', nodes }],
          tailBlock: null, tailMode: 'plain',
          tailFrame: { streamId: 1, epoch: 1, revision: 1, frameSeq: 1, reset: true, snapshot: [], mutations: [] },
        },
      }));
      const message = store.getActiveStreamMessage(context.ownerId, 'agent', context.topicId, messageId)!;
      await vi.waitFor(() => expect(message.blocks).toHaveLength(1));
      const wrapper = mount(MessageRenderer, {
        props: { message }, attachTo: document.body,
        global: { plugins: [pinia], directives: { longpress: {} }, stubs: { VcpAvatar: true } },
      });
      try {
        await nextTick();
        const before = wrapper.get<HTMLElement>('.vcp-scrollable.no-swipe > table').element;
        before.scrollLeft = 73;
        expect(before.scrollLeft).toBe(73);
        if (mode === 'status-only') {
          store.removeSessionStream(context.ownerId, 'agent', context.topicId, messageId);
        } else {
          const blocks: ContentBlock[] = [{
            type: 'markdown', nodes: structuredClone(nodes),
            hash: mode === 'terminal-hash-switch' ? 123456789 : 'abcdef0123456789',
          }];
          await store.processStreamEvent(event({ type: 'end', content, finishReason: 'completed', blocks }));
        }
        await nextTick();
        const after = wrapper.get<HTMLElement>('.vcp-scrollable.no-swipe > table').element;
        const observed = { mode, sameElement: before === after, oldConnected: before.isConnected, newScrollLeft: after.scrollLeft };
        console.log(JSON.stringify(observed));
        if (mode === 'terminal-hash-switch') {
          expect(after).not.toBe(before);
          expect(before.isConnected).toBe(false);
          expect(after.scrollLeft).toBe(0);
        } else {
          expect(after).toBe(before);
          expect(after.scrollLeft).toBe(73);
        }
      } finally {
        wrapper.unmount();
        disposePinia(pinia);
      }
    });
  }
});

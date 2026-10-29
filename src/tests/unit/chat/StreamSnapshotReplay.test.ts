import { createHash } from 'node:crypto';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, disposePinia, setActivePinia } from 'pinia';
import { useChatStreamStore } from '@/core/stores/chatStreamStore';
import { applyFrame, cleanupRegistry, rebuildSnapshot } from '@/core/utils/astExecutor';
import type { AuroraRecoverySnapshot, MarkdownNode, StreamEventDto } from '@/core/types/chat';
import { invokeMock, mockInvoke } from '@/tests/mocks/tauri';

const context = { ownerId: 'agent', ownerType: 'agent' as const, topicId: 'topic' };
const messageId = 'recovering-message';
const hash = (text: string) => createHash('sha256').update(text).digest('hex');
const paragraph = (text: string): MarkdownNode => ({
  type: 'paragraph', children: [{ type: 'text', value: text }],
});
const snapshot = (text: string): AuroraRecoverySnapshot => ({
  stableBlocks: [], tailBlock: { type: 'markdown', content: text, hash: hash(text) },
  tailMode: 'ast', tailSnapshot: [paragraph(text)],
});
const event = (value: Partial<StreamEventDto>): StreamEventDto => ({
  consumerRevision: 1, type: 'aurora', messageId, context, chunk: null,
  finishReason: null, error: null, aurora: null, blocks: null, timestamp: null, topicUpdatedAt: null,
  ...value,
});
const baseline = (text: string, frameSeq = 1, consumerRevision = 1): StreamEventDto => event({
  consumerRevision,
  aurora: {
    kind: 'snapshot', streamId: 1, content: text, stableBlocks: [],
    tailBlock: { type: 'markdown', content: text, hash: hash(text) }, tailMode: 'ast',
    tailFrame: { streamId: 1, epoch: 1, revision: frameSeq, frameSeq,
      reset: true, snapshot: [paragraph(text)], mutations: [] },
  },
});
const append = (before: string, suffix: string, frameSeq: number): StreamEventDto => event({
  aurora: {
    kind: 'delta', streamId: 1, chunk: suffix,
    tailOp: { op: 'append', content: suffix, previousHash: hash(before), hash: hash(before + suffix),
      mode: 'ast', blockType: 'markdown' },
    // Rust's ast_diff and the snapshot builder both address paragraphs as tN
    // and their inline children as .iN; hashes use the same full UTF-8 tail.
    tailFrame: { streamId: 1, epoch: 1, revision: frameSeq, frameSeq,
      mutations: [{ op: 'append', id: 't0.i0', chunk: suffix }] },
  },
});

describe('Aurora snapshot cursor replay', () => {
  let pinia: ReturnType<typeof createPinia>;
  let frames: Map<number, FrameRequestCallback>;
  let nextFrame: number;
  let resolveCompile: (value: AuroraRecoverySnapshot) => void;

  beforeEach(() => {
    pinia = createPinia();
    setActivePinia(pinia);
    frames = new Map();
    nextFrame = 1;
    vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => {
      const id = nextFrame++;
      frames.set(id, callback);
      return id;
    });
    vi.stubGlobal('cancelAnimationFrame', (id: number) => frames.delete(id));
    mockInvoke('rebuild_aurora_snapshot', () => new Promise<AuroraRecoverySnapshot>(resolve => {
      resolveCompile = resolve;
    }));
  });
  afterEach(() => {
    disposePinia(pinia);
    cleanupRegistry(messageId);
    vi.unstubAllGlobals();
  });
  const flush = () => {
    const callbacks = [...frames.values()];
    frames.clear();
    callbacks.forEach(callback => callback(100));
  };
  const message = (store: ReturnType<typeof useChatStreamStore>) =>
    store.getActiveStreamMessage('agent', 'agent', 'topic', messageId)!;
  const recover = (store: ReturnType<typeof useChatStreamStore>) =>
    store.requestAuroraSnapshot('agent', 'agent', 'topic', messageId);
  const compileCount = () => invokeMock.mock.calls.filter(([command]) => command === 'rebuild_aurora_snapshot').length;

  it('commits one fixed snapshot plus live deltas without waiting for output to pause', async () => {
    const store = useChatStreamStore();
    await store.processStreamEvent(baseline('你'));
    flush();
    const recovery = recover(store);
    await store.processStreamEvent(append('你', '好', 2));
    flush();
    await store.processStreamEvent(append('你好', '🙂', 3));
    flush();
    await store.processStreamEvent(append('你好🙂', '!', 4));
    resolveCompile(snapshot('你'));
    expect(await recovery).toBe(true);
    expect(compileCount()).toBe(1);
    expect(message(store).content).toBe('你好🙂!');
    expect(message(store).tailContent).toBe('你好🙂!');
    expect(message(store).tailBlock?.hash).toBe(hash('你好🙂!'));
    const frame = message(store).tailFrame!;
    expect(frame).toMatchObject({ frameSeq: 4, reset: true });
    const sandbox = document.createElement('div');
    rebuildSnapshot(frame.snapshot!, messageId, sandbox);
    expect(applyFrame(frame.mutations ?? [], messageId, sandbox).ok).toBe(true);
    expect(sandbox.textContent).toBe('你好🙂!');
    expect(frames.size).toBe(0);
    await store.processStreamEvent(append('你好🙂!', '继续', 5));
    flush();
    expect(message(store).content).toBe('你好🙂!继续');
    expect(compileCount()).toBe(1);
  });

  it('replays stable precipitation and a new tail epoch over the captured prefix', async () => {
    const store = useChatStreamStore();
    await store.processStreamEvent(baseline('first'));
    flush();
    const recovery = recover(store);
    await store.processStreamEvent(event({ aurora: {
      kind: 'delta', streamId: 1, chunk: '\n\nsecond',
      stableAppend: { baseCount: 0, blocks: [{ type: 'markdown', content: 'first', hash: hash('first'), nodes: [paragraph('first')] }] },
      tailOp: { op: 'replace', content: 'second', hash: hash('second'), mode: 'ast', blockType: 'markdown' },
      tailFrame: { streamId: 1, epoch: 2, revision: 1, frameSeq: 2,
        reset: true, snapshot: [paragraph('second')], mutations: [] },
    } }));
    flush();
    resolveCompile(snapshot('first'));
    expect(await recovery).toBe(true);
    expect(message(store).content).toBe('first\n\nsecond');
    expect(message(store).blocks?.map(block => block.content)).toEqual(['first']);
    expect(message(store).tailBlock?.content).toBe('second');
    const frame = message(store).tailFrame!;
    const sandbox = document.createElement('div');
    rebuildSnapshot(frame.snapshot!, messageId, sandbox);
    expect(applyFrame(frame.mutations ?? [], messageId, sandbox).ok).toBe(true);
    expect(sandbox.textContent).toBe('second');
  });

  it('settles immediately when a complete owner snapshot supersedes the local rebuild', async () => {
    const store = useChatStreamStore();
    await store.processStreamEvent(baseline('old'));
    flush();
    const recovery = recover(store);
    await store.processStreamEvent(baseline('authoritative', 2));
    expect(await recovery).toBe(true);
    expect(message(store).content).toBe('authoritative');
    resolveCompile(snapshot('old'));
    await Promise.resolve();
    expect(message(store).content).toBe('authoritative');
  });

  it.each(['consumer', 'terminal'] as const)('invalidates pending replay on %s replacement', async boundary => {
    const store = useChatStreamStore();
    await store.processStreamEvent(baseline('old'));
    flush();
    const recovery = recover(store);
    if (boundary === 'consumer') await store.processStreamEvent(baseline('new', 1, 2));
    else await store.processStreamEvent(event({ type: 'end', content: 'new', finishReason: 'completed',
      blocks: [{ type: 'markdown', content: 'new' }] }));
    expect(await recovery).toBeNull();
    flush();
    resolveCompile(snapshot('old'));
    await Promise.resolve();
    expect(message(store).content).toBe('new');
  });

  it.each(['frames', 'bytes', 'mutations'] as const)('bounds recovery %s and keeps the latest tail readable', async limit => {
    const store = useChatStreamStore();
    await store.processStreamEvent(baseline('A'));
    flush();
    const recovery = recover(store);
    let text = 'A';
    if (limit === 'frames') {
      for (let seq = 2; seq <= 66; seq++) {
        await store.processStreamEvent(append(text, 'x', seq));
        text += 'x';
        flush();
      }
    } else {
      const suffix = 'x'.repeat(limit === 'bytes' ? 600_000 : 513);
      const update = append(text, suffix, 2);
      if (limit === 'mutations') update.aurora!.tailFrame!.mutations = Array.from({ length: 513 }, () => ({
        op: 'append' as const, id: 't0.i0', chunk: 'x',
      }));
      await store.processStreamEvent(update);
      text += suffix;
    }
    expect(await recovery).toBe(false);
    flush();
    expect(message(store).content).toBe(text);
    expect(message(store).tailBlock).toMatchObject({ content: text, render_mode: 'plain' });
    expect(message(store).tailFrame).toBeUndefined();
    expect(await recover(store)).toBe(false);
    expect(compileCount()).toBe(1);
    resolveCompile(snapshot('A'));
    await Promise.resolve();
    expect(message(store).content).toBe(text);
    // A full owner baseline restores the AST path instead of making the local
    // failure a permanent property of this message.
    await store.processStreamEvent(baseline('reset', 100));
    flush();
    expect(message(store).tailBlock?.render_mode).toBe('ast');
    expect(message(store).tailFrame?.snapshot).toEqual([paragraph('reset')]);
  });

  it('does not replay a cursor gap or start a second compilation loop', async () => {
    const store = useChatStreamStore();
    await store.processStreamEvent(baseline('A'));
    flush();
    const recovery = recover(store);
    await store.processStreamEvent(append('A', 'C', 3));
    resolveCompile(snapshot('A'));
    expect(await recovery).toBe(false);
    flush();
    expect(message(store).content).toBe('AC');
    expect(message(store).tailBlock?.render_mode).toBe('plain');
    expect(compileCount()).toBe(1);
  });
});

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, disposePinia, setActivePinia } from "pinia";
import { flushPromises } from "@vue/test-utils";
import { useChatStreamStore } from "@/core/stores/chatStreamStore";
import { mockInvoke } from "@/tests/mocks/tauri";
import type { AuroraRecoverySnapshot, ContentBlock, StreamEventDto } from "@/core/types/chat";

const context = {
  ownerId: "agent-a",
  ownerType: "agent" as const,
  topicId: "topic-a",
  agentId: "agent-a",
};
const messageId = "projection-message";
const event = (value: Partial<StreamEventDto> & Pick<StreamEventDto, "type">): StreamEventDto => ({
  consumerRevision: 1,
  messageId,
  context,
  chunk: null,
  finishReason: null,
  error: null,
  aurora: null,
  blocks: null,
  timestamp: null,
  topicUpdatedAt: null,
  ...value,
});

const baseline = (content = "A", frameSeq = 1): StreamEventDto => event({
  type: "aurora",
  aurora: {
    kind: "snapshot",
    streamId: 1,
    content: `<think>${content}`,
    stableBlocks: [{ type: "markdown", content: "prefix", hash: "prefix" }],
    tailBlock: { type: "thought", content, hash: content, theme: "思维链", is_complete: false },
    tailMode: "plain",
    tailFrame: { streamId: 1, epoch: 1, revision: frameSeq, frameSeq, reset: true, snapshot: [], mutations: [] },
  },
});

const append = (content: string, previousHash: string, hash: string, frameSeq: number): StreamEventDto => event({
  type: "aurora",
  aurora: {
    kind: "delta",
    streamId: 1,
    chunk: content,
    tailOp: { op: "append", content, previousHash, hash, mode: "plain", blockType: "thought", thoughtTheme: "思维链" },
    tailFrame: { streamId: 1, epoch: 1, revision: frameSeq, frameSeq, mutations: [] },
  },
});

describe("stream render projection commits", () => {
  let pinia: ReturnType<typeof createPinia>;
  let frames: Map<number, FrameRequestCallback>;
  let nextFrame: number;

  beforeEach(() => {
    pinia = createPinia();
    setActivePinia(pinia);
    frames = new Map();
    nextFrame = 1;
    vi.stubGlobal("requestAnimationFrame", vi.fn((callback: FrameRequestCallback) => {
      const id = nextFrame++;
      frames.set(id, callback);
      return id;
    }));
    vi.stubGlobal("cancelAnimationFrame", vi.fn((id: number) => { frames.delete(id); }));
  });

  afterEach(() => {
    disposePinia(pinia);
    vi.unstubAllGlobals();
  });

  const flushFrame = () => {
    const callbacks = [...frames.values()];
    frames.clear();
    callbacks.forEach(callback => callback(100));
  };

  const message = (store: ReturnType<typeof useChatStreamStore>) =>
    store.getActiveStreamMessage("agent-a", "agent", "topic-a", messageId)!;

  it("commits all pending tail fields when stopping before the scheduled frame", async () => {
    const store = useChatStreamStore();
    await store.processStreamEvent(baseline());
    expect(frames.size).toBe(1);
    await store.processStreamEvent(event({ type: "stopping" }));

    expect(message(store)).toMatchObject({
      content: "<think>A",
      blocks: [{ type: "markdown", content: "prefix", hash: "prefix" }],
      tailContent: "A",
      tailBlock: { type: "thought", content: "A", render_mode: "plain" },
      tailOp: { op: "replace", content: "A" },
      tailSnapshot: [],
      tailFrame: { streamId: 1, frameSeq: 1, reset: true },
      isStopping: true,
    });
    expect(frames.size).toBe(0);
    expect(store.isMessageActive("agent-a", "agent", "topic-a", messageId)).toBe(true);
  });

  it("replaces an unpainted delta with a delivered snapshot before accepting another append", async () => {
    const store = useChatStreamStore();
    await store.processStreamEvent(baseline());
    flushFrame();
    await store.processStreamEvent(append("B", "A", "AB", 2));
    await store.processStreamEvent(baseline("ABC", 3));
    await store.processStreamEvent(append("D", "ABC", "ABCD", 4));
    flushFrame();

    expect(message(store).content).toBe("<think>ABCD");
    expect(message(store).tailContent).toBe("ABCD");
    expect(message(store).tailOp).toMatchObject({ op: "replace", content: "ABCD", hash: "ABCD" });
  });

  it("consumes pending operations when recovery replaces the projection", async () => {
    const store = useChatStreamStore();
    await store.processStreamEvent(baseline());
    flushFrame();
    await store.processStreamEvent(append("B", "A", "AB", 2));
    mockInvoke("rebuild_aurora_snapshot", () => ({
      stableBlocks: [],
      tailBlock: { type: "thought", content: "AB", hash: "AB", theme: "思维链", is_complete: false },
      tailMode: "plain",
      tailSnapshot: [],
    }));

    expect(await store.requestAuroraSnapshot("agent-a", "agent", "topic-a", messageId)).toBe(true);
    expect(message(store).tailOp).toMatchObject({ op: "replace", content: "AB" });
    expect(message(store).tailFrame).toMatchObject({ frameSeq: 2, reset: true });
    expect(frames.size).toBe(0);
    await store.processStreamEvent(append("C", "AB", "ABC", 3));
    flushFrame();

    expect(message(store).content).toBe("<think>ABC");
    expect(message(store).tailContent).toBe("ABC");
    expect(message(store).tailOp).toMatchObject({ op: "append", content: "C", previousHash: "AB" });
  });

  it("publishes an explicit clear when a recovery snapshot no longer has a tail", async () => {
    const store = useChatStreamStore();
    await store.processStreamEvent(baseline());
    flushFrame();
    mockInvoke("rebuild_aurora_snapshot", () => ({
      stableBlocks: [{ type: "markdown", content: "complete", hash: "complete" }],
      tailSnapshot: [],
    }));
    await store.requestAuroraSnapshot("agent-a", "agent", "topic-a", messageId);

    expect(message(store).tailContent).toBe("");
    expect(message(store).tailBlock).toBeUndefined();
    expect(message(store).tailOp).toEqual({ op: "clear" });
    expect(message(store).blocks).toEqual([{ type: "markdown", content: "complete", hash: "complete" }]);
  });

  it("lets terminal blocks replace the complete projection and rejects late frame writes", async () => {
    const store = useChatStreamStore();
    await store.processStreamEvent(baseline());
    const current = message(store);
    const blocks: ContentBlock[] = [{ type: "markdown", content: "authoritative", hash: 42 }];
    await store.processStreamEvent(event({ type: "end", content: "authoritative", finishReason: "completed", blocks }));
    await store.processStreamEvent(append("late", "A", "late", 2));
    flushFrame();

    expect(current.content).toBe("authoritative");
    expect(current.blocks).toEqual(blocks);
    expect(current.tailContent).toBe("");
    expect(current.tailBlock).toBeUndefined();
    expect(current.tailOp).toBeUndefined();
    expect(current.tailSnapshot).toBeUndefined();
    expect(current.tailFrame).toBeUndefined();
    expect(store.isMessageActive("agent-a", "agent", "topic-a", messageId)).toBe(false);
  });

  it("keeps the flushed projection when terminal compilation fails", async () => {
    const store = useChatStreamStore();
    await store.processStreamEvent(baseline());
    const current = message(store);
    mockInvoke("process_message_content", () => Promise.reject(new Error("compiler unavailable")));
    vi.spyOn(console, "error").mockImplementation(() => {});
    await store.processStreamEvent(event({ type: "end", content: "<think>A", finishReason: "completed" }));
    await flushPromises();
    flushFrame();

    expect(current.tailBlock).toMatchObject({ type: "thought", content: "A" });
    expect(current.tailOp).toMatchObject({ op: "replace", content: "A" });
    expect(current.tailFrame).toMatchObject({ frameSeq: 1 });
    expect(current.blocks).toEqual([{ type: "markdown", content: "prefix", hash: "prefix" }]);
    expect(current.finishReason).toBe("completed");
    expect(store.isMessageActive("agent-a", "agent", "topic-a", messageId)).toBe(false);
  });

  it("discards a recovery reply that arrives after the terminal projection", async () => {
    const store = useChatStreamStore();
    await store.processStreamEvent(baseline());
    flushFrame();
    const current = message(store);
    let finishRecovery!: (snapshot: AuroraRecoverySnapshot) => void;
    mockInvoke("rebuild_aurora_snapshot", () => new Promise<AuroraRecoverySnapshot>(resolve => { finishRecovery = resolve; }));
    const recovery = store.requestAuroraSnapshot("agent-a", "agent", "topic-a", messageId);
    const blocks: ContentBlock[] = [{ type: "markdown", content: "final", hash: 45 }];
    await store.processStreamEvent(event({ type: "end", content: "final", finishReason: "completed", blocks }));
    finishRecovery({ stableBlocks: [], tailBlock: { type: "markdown", content: "obsolete", hash: "obsolete" }, tailSnapshot: [] });

    expect(await recovery).toBe(false);
    expect(current.blocks).toEqual(blocks);
    expect(current.tailBlock).toBeUndefined();
    expect(current.tailOp).toBeUndefined();
  });
});

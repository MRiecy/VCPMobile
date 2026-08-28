import { beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { useChatStreamStore } from "@/core/stores/chatStreamStore";
import { mockInvoke } from "@/tests/mocks/tauri";
import type { MarkdownNode, StreamEventDto } from "@/core/types/chat";

const streamEvent = (
  event: Pick<StreamEventDto, "type" | "messageId"> & Partial<StreamEventDto>,
): StreamEventDto => ({
  chunk: null,
  context: null,
  finishReason: null,
  error: null,
  aurora: null,
  blocks: null,
  timestamp: null,
  ...event,
});

const streamContext = {
  ownerId: "agent-a",
  ownerType: "agent" as const,
  topicId: "topic-a",
  agentId: "agent-a",
};

const tailEvent = (
  streamId: number,
  frameSeq: number,
  text: string,
  options: { epoch?: number; reset?: boolean; chunk?: string } = {},
): StreamEventDto => {
  const snapshot: MarkdownNode[] = [{
    type: "paragraph",
    children: [{ type: "text", value: text }],
  }];
  return streamEvent({
    type: "aurora",
    messageId: "assistant-1",
    context: streamContext,
    aurora: {
      streamId,
      chunk: options.chunk ?? text,
      tailChanged: true,
      tail: text,
      tailBlock: {
        type: "markdown",
        content: text,
        nodes: snapshot,
        hash: `${streamId}:${frameSeq}`,
      },
      tailFrame: {
        streamId,
        epoch: options.epoch ?? 1,
        revision: frameSeq,
        frameSeq,
        reset: options.reset,
        snapshot: options.reset ? snapshot : undefined,
        mutations: options.reset
          ? []
          : [{ op: "append", id: "t0.i0", chunk: text }],
      },
    },
  });
};

const installManualRaf = () => {
  const originalRaf = window.requestAnimationFrame;
  const originalCancelRaf = window.cancelAnimationFrame;
  const callbacks = new Map<number, FrameRequestCallback>();
  let nextId = 1;
  let now = 100;
  const nowSpy = vi.spyOn(performance, "now").mockImplementation(() => now);
  window.requestAnimationFrame = vi.fn((callback: FrameRequestCallback) => {
    const id = nextId++;
    callbacks.set(id, callback);
    return id;
  });
  window.cancelAnimationFrame = vi.fn((id: number) => {
    callbacks.delete(id);
  });

  return {
    flush: () => {
      now += 100;
      const queued = [...callbacks.values()];
      callbacks.clear();
      queued.forEach((callback) => callback(now));
    },
    restore: () => {
      nowSpy.mockRestore();
      window.requestAnimationFrame = originalRaf;
      window.cancelAnimationFrame = originalCancelRaf;
    },
  };
};

describe("stream render backpressure", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    mockInvoke("process_message_content", () => []);
  });

  it("accepts a warm baseline from a newer stream and rebases its lower frame sequence", async () => {
    const manualRaf = installManualRaf();
    try {
      const store = useChatStreamStore();
      await store.processStreamEvent(streamEvent({
        type: "thinking",
        messageId: "assistant-1",
        context: streamContext,
      }));

      await store.processStreamEvent(tailEvent(10, 100, "old", { reset: true }));
      manualRaf.flush();

      await store.processStreamEvent(streamEvent({
        type: "aurora",
        messageId: "assistant-1",
        context: streamContext,
        aurora: {
          streamId: 11,
          content: "authoritative",
          stableChanged: true,
          stableBlocks: [],
          tailChanged: true,
          tail: "authoritative",
          tailBlock: {
            type: "markdown",
            content: "authoritative",
            nodes: [{
              type: "paragraph",
              children: [{ type: "text", value: "authoritative" }],
            }],
            hash: "baseline",
          },
        },
      }));
      await store.processStreamEvent(tailEvent(11, 2, "authoritative!", { chunk: "!" }));
      await store.processStreamEvent(streamEvent({
        type: "end",
        messageId: "assistant-1",
        context: streamContext,
      }));

      const message = store.getActiveStreamMessage(
        "agent-a",
        "agent",
        "topic-a",
        "assistant-1",
      );
      expect(message?.content).toBe("authoritative!");
      expect(message?.tailFrame).toMatchObject({
        streamId: 11,
        frameSeq: 2,
        reset: true,
        mutations: [],
      });
      expect(message?.tailFrame?.snapshot).toEqual([{
        type: "paragraph",
        children: [{ type: "text", value: "authoritative!" }],
      }]);
    } finally {
      manualRaf.restore();
    }
  });

  it("turns a forward frame gap into the latest snapshot after an earlier rAF flush", async () => {
    const manualRaf = installManualRaf();
    try {
      const store = useChatStreamStore();
      await store.processStreamEvent(streamEvent({
        type: "thinking",
        messageId: "assistant-1",
        context: streamContext,
      }));

      await store.processStreamEvent(tailEvent(20, 1, "one", { reset: true }));
      manualRaf.flush();
      await store.processStreamEvent(tailEvent(20, 3, "three"));
      await store.processStreamEvent(streamEvent({
        type: "end",
        messageId: "assistant-1",
        context: streamContext,
      }));

      const frame = store.getActiveStreamMessage(
        "agent-a",
        "agent",
        "topic-a",
        "assistant-1",
      )?.tailFrame;
      expect(frame).toMatchObject({
        streamId: 20,
        frameSeq: 3,
        reset: true,
        mutations: [],
      });
      expect(frame?.snapshot?.[0]).toMatchObject({
        type: "paragraph",
        children: [{ type: "text", value: "three" }],
      });
    } finally {
      manualRaf.restore();
    }
  });

  it("keeps contiguous frames as one ordered mutation batch inside a pending rAF", async () => {
    const manualRaf = installManualRaf();
    try {
      const store = useChatStreamStore();
      await store.processStreamEvent(streamEvent({
        type: "thinking",
        messageId: "assistant-1",
        context: streamContext,
      }));

      await store.processStreamEvent(tailEvent(25, 1, "one"));
      await store.processStreamEvent(tailEvent(25, 2, "two"));
      await store.processStreamEvent(streamEvent({
        type: "end",
        messageId: "assistant-1",
        context: streamContext,
      }));

      const frame = store.getActiveStreamMessage(
        "agent-a",
        "agent",
        "topic-a",
        "assistant-1",
      )?.tailFrame;
      expect(frame?.reset).not.toBe(true);
      expect(frame?.frameSeq).toBe(2);
      expect(frame?.mutations).toEqual([
        { op: "append", id: "t0.i0", chunk: "one" },
        { op: "append", id: "t0.i0", chunk: "two" },
      ]);
    } finally {
      manualRaf.restore();
    }
  });

  it("drops a duplicate frame event before its chunk can be appended twice", async () => {
    const manualRaf = installManualRaf();
    try {
      const store = useChatStreamStore();
      await store.processStreamEvent(streamEvent({
        type: "thinking",
        messageId: "assistant-1",
        context: streamContext,
      }));

      await store.processStreamEvent(tailEvent(30, 1, "once", {
        reset: true,
        chunk: "once",
      }));
      manualRaf.flush();
      await store.processStreamEvent(tailEvent(30, 1, "duplicate", {
        chunk: "duplicate",
      }));

      const message = store.getActiveStreamMessage(
        "agent-a",
        "agent",
        "topic-a",
        "assistant-1",
      );
      expect(message?.content).toBe("once");
      expect(message?.tailFrame?.frameSeq).toBe(1);
    } finally {
      manualRaf.restore();
    }
  });

  it("keeps a commit error recoverable and accepts the later durable end", async () => {
    const store = useChatStreamStore();
    await store.processStreamEvent(streamEvent({
      type: "thinking",
      messageId: "assistant-1",
      context: streamContext,
    }));

    await store.processStreamEvent(streamEvent({
      type: "error",
      messageId: "assistant-1",
      context: streamContext,
      finishReason: "error",
      error: "terminal commit failed",
      content: "candidate partial",
    }));

    let message = store.getActiveStreamMessage(
      "agent-a",
      "agent",
      "topic-a",
      "assistant-1",
    );
    expect(message?.content).toBe("candidate partial");
    expect(message?.isReconnecting).toBe(true);
    expect(message?.finishReason).toBeUndefined();
    expect(store.isMessageActive("agent-a", "agent", "topic-a", "assistant-1")).toBe(true);

    await store.processStreamEvent(streamEvent({
      type: "end",
      messageId: "assistant-1",
      context: streamContext,
      finishReason: "error",
      content: "committed partial\n\n> VCP流式错误: network failed",
      blocks: [],
      timestamp: 123,
    }));

    message = store.getActiveStreamMessage(
      "agent-a",
      "agent",
      "topic-a",
      "assistant-1",
    );
    expect(message?.content).toBe("committed partial\n\n> VCP流式错误: network failed");
    expect(message?.finishReason).toBe("error");
    expect(message?.timestamp).toBe(123);
    expect(message?.isReconnecting).toBe(false);
    expect(store.isMessageActive("agent-a", "agent", "topic-a", "assistant-1")).toBe(false);

    await store.processStreamEvent(streamEvent({
      type: "error",
      messageId: "assistant-1",
      context: streamContext,
      error: "late error",
      content: "must not replace committed content",
    }));
    expect(message?.content).toBe("committed partial\n\n> VCP流式错误: network failed");
  });

  it("collapses a hidden WebView's pending AST diffs into the latest snapshot", async () => {
    const originalRaf = window.requestAnimationFrame;
    const hiddenDescriptor = Object.getOwnPropertyDescriptor(document, "hidden");
    Object.defineProperty(document, "hidden", {
      configurable: true,
      value: true,
    });
    window.requestAnimationFrame = vi.fn(() => 1);

    try {
      const store = useChatStreamStore();
      await store.processStreamEvent(streamEvent({
        type: "thinking",
        messageId: "assistant-1",
        context: {
          ownerId: "agent-a",
          ownerType: "agent",
          topicId: "topic-a",
          agentId: "agent-a",
        },
      }));

      for (let index = 1; index <= 700; index += 1) {
        const snapshot: MarkdownNode[] = [{
          type: "paragraph",
          children: [{ type: "text", value: `node-${index}` }],
        }];
        await store.processStreamEvent(streamEvent({
          type: "aurora",
          messageId: "assistant-1",
          context: {
            ownerId: "agent-a",
            ownerType: "agent",
            topicId: "topic-a",
            agentId: "agent-a",
          },
          aurora: {
            streamId: 1,
            chunk: "x",
            tailChanged: true,
            tail: `tail-${index}`,
            tailBlock: {
              type: "markdown",
              content: `tail-${index}`,
              nodes: snapshot,
              hash: String(index),
            },
            tailFrame: {
              streamId: 1,
              epoch: 1,
              revision: index,
              frameSeq: index,
              mutations: [
                { op: "append", id: "node", chunk: String(index) },
              ],
            },
          },
        }));
      }

      await store.processStreamEvent(streamEvent({
        type: "end",
        messageId: "assistant-1",
        context: {
          ownerId: "agent-a",
          ownerType: "agent",
          topicId: "topic-a",
          agentId: "agent-a",
        },
      }));

      const frame = store.getActiveStreamMessage(
        "agent-a",
        "agent",
        "topic-a",
        "assistant-1",
      )?.tailFrame;
      expect(frame?.reset).toBe(true);
      expect(frame?.mutations).toEqual([]);
      expect(frame?.snapshot).toEqual([{
        type: "paragraph",
        children: [{ type: "text", value: "node-700" }],
      }]);
    } finally {
      window.requestAnimationFrame = originalRaf;
      if (hiddenDescriptor) {
        Object.defineProperty(document, "hidden", hiddenDescriptor);
      }
    }
  });
});

import { beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { useChatStreamStore } from "@/core/stores/chatStreamStore";
import { invokeMock, mockInvoke } from "@/tests/mocks/tauri";
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
  topicUpdatedAt: null,
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
  options: {
    epoch?: number;
    reset?: boolean;
    chunk?: string;
    mutationChunk?: string;
  } = {},
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
      kind: "delta",
      streamId,
      chunk: options.chunk ?? text,
      tailOp: {
        op: "replace",
        content: text,
        hash: `${streamId}:${frameSeq}`,
        mode: "ast",
        blockType: "markdown",
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
          : [{
              op: "append",
              id: "t0.i0",
              chunk: options.mutationChunk ?? text,
            }],
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
    mockInvoke("rebuild_aurora_snapshot", (args) => {
      const content = String(args?.content ?? "");
      const tailSnapshot: MarkdownNode[] = content
        ? [{
            type: "paragraph",
            children: [{ type: "text", value: content }],
          }]
        : [];
      return {
        stableBlocks: [],
        tailBlock: content
          ? { type: "markdown", content, hash: `tail:${content.length}` }
          : undefined,
        tailMode: "ast",
        tailSnapshot,
      };
    });
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
          kind: "snapshot",
          streamId: 11,
          content: "authoritative",
          stableBlocks: [],
          tailMode: "ast",
          tailBlock: {
            type: "markdown",
            content: "authoritative",
            hash: "baseline",
          },
          tailFrame: {
            streamId: 11,
            epoch: 1,
            revision: 1,
            frameSeq: 1,
            reset: true,
            snapshot: [{
              type: "paragraph",
              children: [{ type: "text", value: "authoritative" }],
            }],
            mutations: [],
          },
        },
      }));
      await store.processStreamEvent(tailEvent(11, 2, "authoritative!", {
        chunk: "!",
        mutationChunk: "!",
      }));
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
        mutations: [{ op: "append", id: "t0.i0", chunk: "!" }],
      });
      expect(message?.tailFrame?.snapshot).toEqual([{
        type: "paragraph",
        children: [{ type: "text", value: "authoritative" }],
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
      await vi.waitFor(() => {
        expect(invokeMock).toHaveBeenCalledWith(
          "rebuild_aurora_snapshot",
          { content: "onethree" },
        );
      });
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
        children: [{ type: "text", value: "onethree" }],
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

  it("derives tail text from tailBlock content and clears it without a wire alias", async () => {
    const manualRaf = installManualRaf();
    try {
      const store = useChatStreamStore();
      await store.processStreamEvent(streamEvent({
        type: "thinking",
        messageId: "assistant-1",
        context: streamContext,
      }));

      await store.processStreamEvent(tailEvent(26, 1, "visible tail", { reset: true }));
      manualRaf.flush();

      let message = store.getActiveStreamMessage(
        "agent-a",
        "agent",
        "topic-a",
        "assistant-1",
      );
      expect(message?.tailContent).toBe("visible tail");
      expect(message?.tailBlock?.content).toBe("visible tail");

      await store.processStreamEvent(streamEvent({
        type: "aurora",
        messageId: "assistant-1",
        context: streamContext,
        aurora: {
          kind: "delta",
          streamId: 26,
          tailOp: { op: "clear" },
          tailFrame: {
            streamId: 26,
            epoch: 2,
            revision: 0,
            frameSeq: 2,
            reset: true,
            snapshot: [],
            mutations: [],
          },
        },
      }));
      manualRaf.flush();

      message = store.getActiveStreamMessage(
        "agent-a",
        "agent",
        "topic-a",
        "assistant-1",
      );
      expect(message?.tailContent).toBe("");
      expect(message?.tailBlock).toBeUndefined();
      expect(message?.tailSnapshot).toEqual([]);
    } finally {
      manualRaf.restore();
    }
  });

  it("applies stable and tail appends without receiving cumulative full fields", async () => {
    const manualRaf = installManualRaf();
    try {
      const store = useChatStreamStore();
      await store.processStreamEvent(streamEvent({
        type: "thinking",
        messageId: "assistant-1",
        context: streamContext,
      }));

      await store.processStreamEvent(tailEvent(28, 1, "hello", {
        chunk: "hello",
      }));
      manualRaf.flush();
      await store.processStreamEvent(streamEvent({
        type: "aurora",
        messageId: "assistant-1",
        context: streamContext,
        aurora: {
          kind: "delta",
          streamId: 28,
          chunk: "!",
          stableAppend: {
            baseCount: 0,
            blocks: [{
              type: "markdown",
              content: "stable",
              hash: "stable-1",
            }],
          },
          tailOp: {
            op: "append",
            previousHash: "28:1",
            content: "!",
            hash: "28:2",
            mode: "ast",
            blockType: "markdown",
          },
          tailFrame: {
            streamId: 28,
            epoch: 1,
            revision: 2,
            frameSeq: 2,
            mutations: [{ op: "append", id: "t0.i0", chunk: "!" }],
          },
        },
      }));
      manualRaf.flush();

      const message = store.getActiveStreamMessage(
        "agent-a",
        "agent",
        "topic-a",
        "assistant-1",
      );
      expect(message?.content).toBe("hello!");
      expect(message?.blocks?.map((block) => block.hash)).toEqual(["stable-1"]);
      expect(message?.tailContent).toBe("hello!");
      expect(message?.tailBlock).toMatchObject({
        content: "hello!",
        hash: "28:2",
        render_mode: "ast",
      });
      expect(message?.tailBlock?.nodes).toBeUndefined();
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
            kind: "delta",
            streamId: 1,
            chunk: "x",
            tailOp: {
              op: "replace",
              content: `tail-${index}`,
              hash: String(index),
              mode: "ast",
              blockType: "markdown",
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

      Object.defineProperty(document, "hidden", {
        configurable: true,
        value: false,
      });
      document.dispatchEvent(new Event("visibilitychange"));
      await vi.waitFor(() => {
        expect(invokeMock).toHaveBeenCalledWith(
          "rebuild_aurora_snapshot",
          { content: "x".repeat(700) },
        );
      });

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
        children: [{ type: "text", value: "x".repeat(700) }],
      }]);
    } finally {
      window.requestAnimationFrame = originalRaf;
      if (hiddenDescriptor) {
        Object.defineProperty(document, "hidden", hiddenDescriptor);
      }
    }
  });
});

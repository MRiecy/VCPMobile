import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { useChatStreamStore } from "@/core/stores/chatStreamStore";

const singleContext = {
  topicId: "topic-a",
  agentId: "agent-a",
  agentName: "Agent A",
};

const groupContext = {
  topicId: "topic-group",
  groupId: "group-a",
  agentId: "agent-a",
  agentName: "Agent A",
  isGroupMessage: true,
};

describe("multi-step stream projection", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    vi.mocked(navigator.vibrate).mockClear();
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it("synchronously clears the old projection and cancels its pending rAF", async () => {
    const pendingFrames = new Map<number, FrameRequestCallback>();
    let nextFrameId = 0;
    const requestSpy = vi
      .spyOn(window, "requestAnimationFrame")
      .mockImplementation((callback) => {
        nextFrameId += 1;
        pendingFrames.set(nextFrameId, callback);
        return nextFrameId;
      });
    const cancelSpy = vi
      .spyOn(window, "cancelAnimationFrame")
      .mockImplementation((id) => {
        pendingFrames.delete(id);
      });

    const store = useChatStreamStore();
    const onMessageCreated = vi.fn();
    await store.processStreamEvent(
      {
        type: "thinking",
        messageId: "assistant-step",
        turnAttempt: "attempt-a",
        stepIndex: 0,
        projectionReset: false,
        context: singleContext,
      },
      { onMessageCreated },
    );
    await store.processStreamEvent({
      type: "aurora",
      messageId: "assistant-step",
      turnAttempt: "attempt-a",
      stepIndex: 0,
      projectionReset: false,
      context: singleContext,
      aurora: {
        content: "pending old projection",
        stableChanged: true,
        stableBlocks: [{ type: "tool-use", content: "old" }],
        tailChanged: true,
        tail: "old tail",
        tailBlock: { type: "markdown", content: "old tail" },
      },
    });

    const sameSkeleton = store.activeStreamMessages.get("assistant-step")!;
    sameSkeleton.content = "visible old projection";
    sameSkeleton.blocks = [{ type: "tool-use", content: "old" }];
    sameSkeleton.tailContent = "visible old tail";
    const staleFrame = [...pendingFrames.values()][0];

    await store.processStreamEvent({
      type: "thinking",
      messageId: "assistant-step",
      turnAttempt: "attempt-a",
      stepIndex: 1,
      projectionReset: true,
      context: singleContext,
    });

    expect(store.activeStreamMessages.get("assistant-step")).toBe(sameSkeleton);
    expect(onMessageCreated).toHaveBeenCalledTimes(1);
    expect(cancelSpy).toHaveBeenCalledTimes(1);
    expect(sameSkeleton.content).toBe("");
    expect(sameSkeleton.blocks).toEqual([]);
    expect(sameSkeleton.tailContent).toBe("");
    expect(sameSkeleton.tailBlock).toBeUndefined();

    staleFrame?.(performance.now() + 100);
    expect(sameSkeleton.content).toBe("");
    expect(sameSkeleton.blocks).toEqual([]);
    expect(requestSpy).toHaveBeenCalledTimes(1);

    sameSkeleton.content = "same-step projection";
    await store.processStreamEvent({
      type: "thinking",
      messageId: "assistant-step",
      turnAttempt: "attempt-a",
      stepIndex: 1,
      projectionReset: true,
      context: singleContext,
    });
    expect(sameSkeleton.content).toBe("same-step projection");
    expect(cancelSpy).toHaveBeenCalledTimes(1);
  });

  it("drops prior-step and retired-attempt frames and only finalizes on end", async () => {
    vi.spyOn(window, "requestAnimationFrame").mockImplementation(() => 1);
    const store = useChatStreamStore();
    const onMessageCreated = vi.fn();
    const onStreamFinished = vi.fn();
    const callbacks = { onMessageCreated, onStreamFinished };

    await store.processStreamEvent(
      {
        type: "thinking",
        messageId: "assistant-group-step",
        turnAttempt: "attempt-a",
        stepIndex: 0,
        projectionReset: false,
        context: groupContext,
      },
      callbacks,
    );
    await store.processStreamEvent(
      {
        type: "aurora",
        messageId: "assistant-group-step",
        turnAttempt: "attempt-a",
        stepIndex: 1,
        projectionReset: true,
        finishReason: "tool_calls",
        context: groupContext,
        aurora: { content: "step one projection" },
      },
      callbacks,
    );

    expect(onStreamFinished).not.toHaveBeenCalled();
    expect(store.isMessageActive("assistant-group-step")).toBe(true);
    expect(navigator.vibrate).not.toHaveBeenCalled();

    await store.processStreamEvent(
      {
        type: "end",
        messageId: "assistant-group-step",
        turnAttempt: "attempt-a",
        stepIndex: 0,
        projectionReset: false,
        context: groupContext,
        blocks: [],
      },
      callbacks,
    );
    expect(onStreamFinished).not.toHaveBeenCalled();
    expect(store.isMessageActive("assistant-group-step")).toBe(true);

    await store.processStreamEvent(
      {
        type: "aurora",
        messageId: "assistant-group-step",
        turnAttempt: "attempt-b",
        stepIndex: 0,
        projectionReset: true,
        context: groupContext,
        aurora: { content: "current attempt projection" },
      },
      callbacks,
    );
    await store.processStreamEvent(
      {
        type: "aurora",
        messageId: "assistant-group-step",
        turnAttempt: "attempt-a",
        stepIndex: 99,
        projectionReset: true,
        context: groupContext,
        aurora: { content: "late retired attempt" },
      },
      callbacks,
    );
    await store.processStreamEvent(
      {
        type: "end",
        messageId: "assistant-group-step",
        turnAttempt: "attempt-b",
        stepIndex: 0,
        projectionReset: false,
        context: groupContext,
        blocks: [],
      },
      callbacks,
    );

    expect(
      store.activeStreamMessages.get("assistant-group-step")?.content,
    ).toBe("current attempt projection");
    expect(onMessageCreated).toHaveBeenCalledTimes(1);
    expect(onStreamFinished).toHaveBeenCalledTimes(1);
    expect(store.isMessageActive("assistant-group-step")).toBe(false);
    expect(navigator.vibrate).toHaveBeenCalledTimes(1);
  });

  it.each(["end", "error"] as const)(
    "keeps a bounded tombstone after accepted %s so delayed frames cannot revive the message",
    async (terminalType) => {
      vi.useFakeTimers();
      const store = useChatStreamStore();
      const onMessageCreated = vi.fn();
      const onStreamFinished = vi.fn();
      const callbacks = { onMessageCreated, onStreamFinished };
      const messageId = `assistant-terminal-${terminalType}`;

      await store.processStreamEvent(
        {
          type: "thinking",
          messageId,
          turnAttempt: "attempt-terminal",
          stepIndex: 2,
          projectionReset: true,
          context: singleContext,
        },
        callbacks,
      );
      await store.processStreamEvent(
        {
          type: terminalType,
          messageId,
          turnAttempt: "attempt-terminal",
          stepIndex: 2,
          projectionReset: false,
          context: singleContext,
          error: terminalType === "error" ? "terminal error" : undefined,
          blocks: [],
        },
        callbacks,
      );

      expect(onStreamFinished).toHaveBeenCalledTimes(1);
      await vi.advanceTimersByTimeAsync(1001);
      expect(store.activeStreamMessages.has(messageId)).toBe(false);

      await store.processStreamEvent(
        {
          type: "aurora",
          messageId,
          turnAttempt: "attempt-terminal",
          stepIndex: 2,
          projectionReset: false,
          context: singleContext,
          aurora: { content: "late same-step projection" },
        },
        callbacks,
      );
      await store.processStreamEvent(
        {
          type: "thinking",
          messageId,
          context: singleContext,
        },
        callbacks,
      );

      expect(store.activeStreamMessages.has(messageId)).toBe(false);
      expect(onMessageCreated).toHaveBeenCalledTimes(1);
      expect(onStreamFinished).toHaveBeenCalledTimes(1);
    },
  );
});

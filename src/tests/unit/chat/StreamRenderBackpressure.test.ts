import { beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { useChatStreamStore } from "@/core/stores/chatStreamStore";
import { mockInvoke } from "@/tests/mocks/tauri";

describe("stream render backpressure", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    mockInvoke("process_message_content", () => []);
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
      await store.processStreamEvent({
        type: "thinking",
        messageId: "assistant-1",
        context: { topicId: "topic-a", agentId: "agent-a" },
      });

      for (let index = 1; index <= 700; index += 1) {
        const snapshot = [{ type: "paragraph", id: `node-${index}` }];
        await store.processStreamEvent({
          type: "aurora",
          messageId: "assistant-1",
          context: { topicId: "topic-a", agentId: "agent-a" },
          aurora: {
            chunk: "x",
            tailChanged: true,
            tail: `tail-${index}`,
            tailBlock: { type: "text", nodes: snapshot },
            tailFrame: {
              epoch: 1,
              revision: index,
              frameSeq: index,
              mutations: [
                { op: "append", id: "node", chunk: String(index) },
              ],
            },
          },
        });
      }

      await store.processStreamEvent({
        type: "end",
        messageId: "assistant-1",
        context: { topicId: "topic-a", agentId: "agent-a" },
      });

      const frame = store.activeStreamMessages.get("assistant-1")?.tailFrame;
      expect(frame?.reset).toBe(true);
      expect(frame?.mutations).toEqual([]);
      expect((frame?.snapshot?.[0] as any)?.id).toBe("node-700");
    } finally {
      window.requestAnimationFrame = originalRaf;
      if (hiddenDescriptor) {
        Object.defineProperty(document, "hidden", hiddenDescriptor);
      }
    }
  });
});

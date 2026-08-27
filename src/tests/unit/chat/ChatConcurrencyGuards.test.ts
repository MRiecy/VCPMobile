import { beforeEach, describe, expect, it } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import {
  MAX_HISTORY_MESSAGES,
  useChatHistoryStore,
} from "@/core/stores/chatHistoryStore";
import { useChatSessionStore } from "@/core/stores/chatSessionStore";
import { useChatStreamStore } from "@/core/stores/chatStreamStore";
import { useSettingsStore } from "@/core/stores/settings";
import { useAssistantStore } from "@/core/stores/assistant";
import { useAttachmentStore } from "@/core/stores/attachmentStore";
import {
  channelInstances,
  invokeMock,
  mockInvoke,
} from "@/tests/mocks/tauri";
import { flushPromises } from "@/tests/utils/flush";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

const message = (id: string, topicId: string) => ({
  id,
  role: "user",
  name: "User",
  content: id,
  timestamp: 1,
  topicId,
  blocks: [],
});

describe("chat conversation concurrency guards", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    mockInvoke("get_active_generations", () => []);
  });

  it("invalidates an A snapshot even when selection later returns to A", () => {
    const session = useChatSessionStore();
    session.setConversation({ id: "agent-a", type: "agent" }, "topic-a");
    const firstA = { ...session.currentConversationKey! };

    session.setConversation({ id: "agent-b", type: "agent" }, "topic-b");
    session.setConversation({ id: "agent-a", type: "agent" }, "topic-a");

    expect(session.currentConversationKey?.epoch).toBeGreaterThan(firstA.epoch);
    expect(session.isConversationCurrent(firstA)).toBe(false);
  });

  it("does not let a slow owner selection overwrite a newer selection", async () => {
    const session = useChatSessionStore();
    const assistant = useAssistantStore();
    assistant.agents = [
      { id: "agent-b", name: "B", model: "test", avatarCalculatedColor: null },
      { id: "agent-c", name: "C", model: "test", avatarCalculatedColor: null },
    ];
    const topicsB = deferred<void>();
    const topicsC = deferred<void>();
    mockInvoke("get_topics_streamed", (args) =>
      args?.ownerId === "agent-b" ? topicsB.promise : topicsC.promise,
    );

    const selectingB = session.selectItem({ id: "agent-b", name: "B", type: "agent" });
    const selectingC = session.selectItem({ id: "agent-c", name: "C", type: "agent" });
    channelInstances[1]?.emit([{
      id: "topic-c",
      name: "Topic C",
    }]);
    topicsC.resolve();
    await selectingC;
    channelInstances[0]?.emit([{
      id: "topic-b",
      name: "Topic B",
    }]);
    topicsB.resolve();
    await selectingB;

    expect(session.currentConversationKey).toMatchObject({
      ownerId: "agent-c",
      topicId: "topic-c",
    });
  });

  it("does not let an old load settle the new conversation latch or messages", async () => {
    const session = useChatSessionStore();
    const history = useChatHistoryStore();
    const loadA = deferred<any[]>();
    const loadB = deferred<any[]>();
    mockInvoke("load_chat_history", (args) =>
      args?.topicId === "topic-a" ? loadA.promise : loadB.promise,
    );

    session.setConversation({ id: "agent-a", type: "agent" }, "topic-a");
    const pendingA = history.loadHistoryPaginated(
      "agent-a",
      "agent",
      "topic-a",
    );
    session.setConversation({ id: "agent-b", type: "agent" }, "topic-b");
    const pendingB = history.loadHistoryPaginated(
      "agent-b",
      "agent",
      "topic-b",
    );

    loadA.resolve([message("message-a", "topic-a")]);
    await pendingA;
    expect(history.isLoadingHistory).toBe(true);
    expect(history.currentChatHistory).toEqual([]);

    loadB.resolve([message("message-b", "topic-b")]);
    await pendingB;
    expect(history.isLoadingHistory).toBe(false);
    expect(history.currentChatHistory.map((item) => item.id)).toEqual([
      "message-b",
    ]);
    expect(history.loadedConversationKey?.topicId).toBe("topic-b");
  });

  it("keeps a send bound to its entry conversation across an await", async () => {
    const session = useChatSessionStore();
    const history = useChatHistoryStore();
    const settings = useSettingsStore();
    settings.settings = {
      userName: "User",
      vcpServerUrl: "https://example.invalid",
      vcpApiKey: "key",
    } as any;
    mockInvoke("load_chat_history", () => []);

    session.setConversation({ id: "agent-a", type: "agent" }, "topic-a");
    await history.loadHistoryPaginated("agent-a", "agent", "topic-a");

    const append = deferred<any[]>();
    mockInvoke("append_single_message", () => append.promise);
    const pendingSend = history.sendMessage("hello");

    session.setConversation({ id: "agent-b", type: "agent" }, "topic-b");
    history.resetHistoryForConversation();
    append.resolve([]);
    await pendingSend;

    const appendCall = invokeMock.mock.calls.find(
      ([command]) => command === "append_single_message",
    );
    expect(appendCall?.[1]).toMatchObject({
      ownerId: "agent-a",
      ownerType: "agent",
      topicId: "topic-a",
    });
    const generationCall = invokeMock.mock.calls.find(
      ([command]) => command === "handle_agent_chat_message",
    );
    expect(generationCall?.[1]).toMatchObject({
      payload: { agentId: "agent-a", topicId: "topic-a" },
    });
    expect(history.currentChatHistory).toEqual([]);
  });

  it("loads older history with a stable keyset cursor and bounded window", async () => {
    const session = useChatSessionStore();
    const history = useChatHistoryStore();
    const recent = Array.from({ length: MAX_HISTORY_MESSAGES }, (_, index) => ({
      ...message(`message-${String(index).padStart(4, "0")}`, "topic-a"),
      timestamp: index + 100,
    }));
    let page = 0;
    mockInvoke("load_chat_history", () => {
      page += 1;
      return page === 1
        ? recent
        : [{ ...message("message-older", "topic-a"), timestamp: 1 }];
    });

    session.setConversation({ id: "agent-a", type: "agent" }, "topic-a");
    await history.loadHistoryPaginated("agent-a", "agent", "topic-a");
    await history.loadMoreHistory();

    const historyCalls = invokeMock.mock.calls.filter(
      ([command]) => command === "load_chat_history",
    );
    expect(historyCalls[1]?.[1]).toMatchObject({
      beforeTimestamp: 100,
      beforeMessageId: "message-0000",
      offset: null,
    });
    expect(history.currentChatHistory).toHaveLength(MAX_HISTORY_MESSAGES);
    expect(history.currentChatHistory[0]?.id).toBe("message-older");
    expect(history.currentChatHistory.some(item => item.id === "message-0499")).toBe(false);
    expect(history.hasEvictedNewer).toBe(true);

    mockInvoke("load_chat_history", () => recent.slice(-15));
    await history.returnToLatest();

    const latestCalls = invokeMock.mock.calls.filter(
      ([command]) => command === "load_chat_history",
    );
    const latestCall = latestCalls[latestCalls.length - 1]?.[1];
    expect(latestCall).toMatchObject({
      beforeTimestamp: null,
      beforeMessageId: null,
      offset: 0,
    });
    expect(history.hasEvictedNewer).toBe(false);
    expect(
      history.currentChatHistory[history.currentChatHistory.length - 1]?.id,
    ).toBe("message-0499");
  });

  it("does not clear or send attachments that are still loading", async () => {
    const session = useChatSessionStore();
    const history = useChatHistoryStore();
    const attachments = useAttachmentStore();
    mockInvoke("load_chat_history", () => []);
    session.setConversation({ id: "agent-a", type: "agent" }, "topic-a");
    await history.loadHistoryPaginated("agent-a", "agent", "topic-a");
    attachments.stagedAttachments.push({
      id: "attachment-loading",
      name: "large.pdf",
      type: "application/pdf",
      size: 10,
      src: "file:///large.pdf",
      status: "loading",
    } as any);

    await history.sendMessage("send with attachment");

    expect(attachments.stagedAttachments).toHaveLength(1);
    expect(
      invokeMock.mock.calls.some(([command]) => command === "append_single_message"),
    ).toBe(false);
  });

  it("does not persist a frontend skeleton for a thinking event", async () => {
    const stream = useChatStreamStore();
    await stream.processStreamEvent({
      type: "thinking",
      chunk: null,
      messageId: "assistant-1",
      context: {
        ownerId: "agent-a",
        ownerType: "agent",
        topicId: "topic-a",
        agentId: "agent-a",
      },
      finishReason: null,
      error: null,
      aurora: null,
      blocks: null,
      timestamp: null,
    });

    expect(
      invokeMock.mock.calls.some(
        ([command]) => command === "append_single_message",
      ),
    ).toBe(false);
  });

  it("stops the original message even when the visible conversation changes", async () => {
    const session = useChatSessionStore();
    const stream = useChatStreamStore();
    session.setConversation({ id: "agent-a", type: "agent" }, "topic-a");
    await stream.processStreamEvent({
      type: "thinking",
      chunk: null,
      messageId: "assistant-a",
      context: {
        ownerId: "agent-a",
        ownerType: "agent",
        topicId: "topic-a",
        agentId: "agent-a",
      },
      finishReason: null,
      error: null,
      aurora: null,
      blocks: null,
      timestamp: null,
    });

    const interrupt = deferred<unknown>();
    mockInvoke("interruptRequest", () => interrupt.promise);
    const stoppingA = stream.stopMessage(
      "agent-a",
      "agent",
      "topic-a",
      "assistant-a",
    );

    session.setConversation({ id: "agent-b", type: "agent" }, "topic-b");
    await stream.processStreamEvent({
      type: "thinking",
      chunk: null,
      messageId: "assistant-b",
      context: {
        ownerId: "agent-b",
        ownerType: "agent",
        topicId: "topic-b",
        agentId: "agent-b",
      },
      finishReason: null,
      error: null,
      aurora: null,
      blocks: null,
      timestamp: null,
    });
    interrupt.resolve(undefined);
    await stoppingA;

    expect(
      stream.isMessageActive("agent-a", "agent", "topic-a", "assistant-a"),
    ).toBe(false);
    expect(
      stream.isMessageActiveInSession(
        "agent-b",
        "agent",
        "topic-b",
        "assistant-b",
      ),
    ).toBe(true);
  });

  it("claims a cold recovery once and never starts the removed two-step resume path", async () => {
    const session = useChatSessionStore();
    session.setConversation({ id: "agent-a", type: "agent" }, "topic-a");
    const recovery = deferred<any>();
    mockInvoke("get_active_generations", () => [{
      msgId: "assistant-recovery",
      topicId: "topic-a",
      ownerId: "agent-a",
      ownerType: "agent",
      createdAt: 1,
    }]);
    mockInvoke("recover_active_generation", () => recovery.promise);

    const stream = useChatStreamStore();
    await stream.checkAndRecoverInterruptedStreams();
    await stream.checkAndRecoverInterruptedStreams();

    const recoveryCalls = invokeMock.mock.calls.filter(
      ([command]) => command === "recover_active_generation",
    );
    expect(recoveryCalls).toHaveLength(1);
    expect(recoveryCalls[0]?.[1]).toMatchObject({
      msgId: "assistant-recovery",
      isWarm: false,
    });
    expect(
      invokeMock.mock.calls.some(([command]) => command === "resume_stream"),
    ).toBe(false);

    recovery.resolve({ status: "completed", content: "recovered" });
    await Promise.resolve();
    await Promise.resolve();
  });

  it("lets the backend finalize a durable turn while offline and fences late recovery frames", async () => {
    const onlineDescriptor = Object.getOwnPropertyDescriptor(navigator, "onLine");
    Object.defineProperty(navigator, "onLine", {
      configurable: true,
      value: false,
    });

    try {
      const session = useChatSessionStore();
      session.setConversation({ id: "agent-a", type: "agent" }, "topic-a");
      mockInvoke("get_active_generations", () => [
        {
          msgId: "assistant-offline-finalizing",
          topicId: "topic-a",
          ownerId: "agent-a",
          ownerType: "agent",
          createdAt: 1,
        },
      ]);

      let recoveryChannel: { emit: (event: unknown) => void } | undefined;
      mockInvoke("recover_active_generation", (args) => {
        recoveryChannel = args?.streamChannel as typeof recoveryChannel;
        recoveryChannel?.emit({
          type: "end",
          messageId: "assistant-offline-finalizing",
          turnAttempt: "attempt-offline",
          stepIndex: 2,
          projectionReset: false,
          finishReason: "completed",
          blocks: [],
          context: {
            ownerId: "agent-a",
            ownerType: "agent",
            topicId: "topic-a",
            agentId: "agent-a",
          },
        });
        return { status: "completed", content: "durable offline result" };
      });

      const stream = useChatStreamStore();
      await stream.checkAndRecoverInterruptedStreams();
      await flushPromises();

      expect(
        invokeMock.mock.calls.filter(
          ([command]) => command === "recover_active_generation",
        ),
      ).toHaveLength(1);
      const recovered = stream.getActiveStreamMessage(
        "agent-a",
        "agent",
        "topic-a",
        "assistant-offline-finalizing",
      );
      expect(recovered?.content).toBe("durable offline result");
      expect(recovered?.finishReason).toBe("completed");
      expect(
        stream.isMessageActive(
          "agent-a",
          "agent",
          "topic-a",
          "assistant-offline-finalizing",
        ),
      ).toBe(false);

      recoveryChannel?.emit({
        type: "thinking",
        messageId: "assistant-offline-finalizing",
        turnAttempt: "attempt-offline",
        stepIndex: 2,
        projectionReset: false,
        context: {
          ownerId: "agent-a",
          ownerType: "agent",
          topicId: "topic-a",
          agentId: "agent-a",
        },
      });
      await flushPromises();

      expect(recovered?.isThinking).toBe(false);
      expect(
        stream.isMessageActive(
          "agent-a",
          "agent",
          "topic-a",
          "assistant-offline-finalizing",
        ),
      ).toBe(false);
      expect(recovered?.content).toBe("durable offline result");
    } finally {
      if (onlineDescriptor) {
        Object.defineProperty(navigator, "onLine", onlineDescriptor);
      } else {
        Reflect.deleteProperty(navigator, "onLine");
      }
    }
  });

});

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
import { useNotificationStore } from "@/core/stores/notification";
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
    mockInvoke("reconnect_chat_streams", () => ({ generations: [], activeRequests: [], groupTurns: [] }));
  });

  it("keeps destructive operations blocked until the first inventory is known", async () => {
    const receipt = deferred<any>();
    mockInvoke("reconnect_chat_streams", () => receipt.promise);
    const stream = useChatStreamStore();
    expect(stream.isOwnerBusy("agent-a", "agent")).toBe(true);
    const connecting = stream.checkAndRecoverInterruptedStreams();
    expect(stream.isConversationBusy("agent-a", "agent", "topic-a")).toBe(true);
    receipt.resolve({ generations: [], activeRequests: [], groupTurns: [] });
    await connecting;
    expect(stream.activityInventoryKnown).toBe(true);
    expect(stream.isOwnerBusy("agent-a", "agent")).toBe(false);
  });

  it.each(["response", "rejection"])("keeps a durable cancellation ahead of a late connection %s", async outcome => {
    const receipt = deferred<any>();
    let output: { emit: (event: unknown) => void } | undefined;
    mockInvoke("reconnect_chat_streams", args => {
      output = args?.channel as typeof output;
      return receipt.promise;
    });
    const stream = useChatStreamStore();
    const connecting = stream.checkAndRecoverInterruptedStreams();
    output?.emit({
      type: "end", messageId: "cancelled", consumerRevision: 100,
      context: { ownerId: "agent-a", ownerType: "agent", topicId: "topic-a" },
      content: "partial", finishReason: "cancelled_by_user", blocks: [],
    });
    if (outcome === "response") receipt.resolve({ generations: [{
      msgId: "cancelled", topicId: "topic-a", ownerId: "agent-a", ownerType: "agent", createdAt: 1,
    }], activeRequests: [], groupTurns: [] });
    else receipt.reject(new Error("late command delivery failure"));
    await connecting;
    const recovered = stream.getActiveStreamMessage("agent-a", "agent", "topic-a", "cancelled");
    expect(recovered?.content).toBe("partial");
    expect(recovered?.finishReason).toBe("cancelled_by_user");
    expect(recovered?.isReconnecting).toBe(false);
    expect(stream.isMessageActive("agent-a", "agent", "topic-a", "cancelled")).toBe(false);
  });

  it("receives a group's next speaker after an empty inventory and later selection", async () => {
    let output: { emit: (event: unknown) => void } | undefined;
    mockInvoke("reconnect_chat_streams", args => {
      output = args?.channel as typeof output;
      return { generations: [], activeRequests: [], groupTurns: [] };
    });
    mockInvoke("load_chat_history", () => []);
    const stream = useChatStreamStore();
    await stream.checkAndRecoverInterruptedStreams();
    const session = useChatSessionStore();
    session.setConversation({ id: "group-a", type: "group" }, "topic-a");
    const history = useChatHistoryStore();
    await history.loadHistory("group-a", "group", "topic-a");
    output?.emit({
      type: "thinking", messageId: "other-owner", consumerRevision: 101,
      context: { ownerId: "group-b", ownerType: "group", topicId: "topic-a" },
    });
    output?.emit({
      type: "thinking", messageId: "next-speaker", consumerRevision: 102,
      context: { ownerId: "group-a", ownerType: "group", topicId: "topic-a", agentId: "speaker" },
    });
    await flushPromises();
    expect(history.currentChatHistory.map(item => item.id)).toEqual(["next-speaker"]);
    expect(stream.isMessageActive("group-a", "group", "topic-a", "next-speaker")).toBe(true);
  });

  it("keeps a loaded partial until an ordered snapshot and delta replace it", async () => {
    const session = useChatSessionStore();
    session.setConversation({ id: "agent-a", type: "agent" }, "topic-a");
    mockInvoke("load_chat_history", () => [{
      id: "live", role: "assistant", content: "saved partial", timestamp: 1,
      blocks: [{ type: "markdown", content: "saved partial" }],
    }]);
    const history = useChatHistoryStore();
    await history.loadHistory("agent-a", "agent", "topic-a");
    const visibleMessage = history.currentChatHistory[0];
    const receipt = deferred<any>();
    let output: { emit: (event: unknown) => void } | undefined;
    mockInvoke("reconnect_chat_streams", args => {
      output = args?.channel as typeof output;
      return receipt.promise;
    });
    const stream = useChatStreamStore();
    const connecting = stream.checkAndRecoverInterruptedStreams();
    const event = {
      messageId: "live", consumerRevision: 111,
      context: { ownerId: "agent-a", ownerType: "agent", topicId: "topic-a" },
    };
    output?.emit({ ...event, type: "reconnecting" });
    expect(visibleMessage.content).toBe("saved partial");
    expect(visibleMessage.isReconnecting).toBe(true);
    output?.emit({ ...event, type: "aurora", aurora: {
      kind: "snapshot", streamId: 60, content: "helper snapshot", stableBlocks: [],
      tailBlock: { type: "markdown", content: "helper snapshot", hash: "base" },
    } });
    output?.emit({ ...event, type: "aurora", aurora: {
      kind: "delta", streamId: 60, chunk: " + live",
      tailOp: { op: "append", content: " + live", previousHash: "base", hash: "next", mode: "plain", blockType: "markdown" },
    } });
    receipt.resolve({ generations: [{
      msgId: "live", ownerId: "agent-a", ownerType: "agent", topicId: "topic-a", createdAt: 1,
    }], activeRequests: [], groupTurns: [] });
    await connecting;
    await new Promise<void>(resolve => window.setTimeout(resolve, 0));
    expect(stream.getActiveStreamMessage("agent-a", "agent", "topic-a", "live")).toBe(visibleMessage);
    expect(history.currentChatHistory).toHaveLength(1);
    expect(visibleMessage.content).toBe("helper snapshot + live");
    expect(visibleMessage.isReconnecting).toBe(false);
  });

  it("keeps a failed attachment pending without a spinner or duplicate toast", async () => {
    let output: { emit: (event: unknown) => void } | undefined;
    mockInvoke("reconnect_chat_streams", args => {
      output = args?.channel as typeof output;
      return { generations: [], activeRequests: [], groupTurns: [] };
    });
    const stream = useChatStreamStore();
    await stream.checkAndRecoverInterruptedStreams();
    const identity = {
      messageId: "partial", consumerRevision: 140,
      context: { ownerId: "agent-a", ownerType: "agent", topicId: "topic-a" },
    };
    output?.emit({ ...identity, type: "reconnecting" });
    output?.emit({ ...identity, type: "aurora", aurora: {
      kind: "snapshot", streamId: 70, content: "received partial", stableBlocks: [],
    } });
    output?.emit({ ...identity, type: "error", error: "connection unavailable" });
    output?.emit({ ...identity, type: "error", error: "connection unavailable" });
    await flushPromises();
    const partial = stream.getActiveStreamMessage("agent-a", "agent", "topic-a", "partial");
    expect(partial?.content).toBe("received partial");
    expect(partial?.isReconnecting).toBe(false);
    expect(partial?.streamRecoveryState).toBe("waiting_retry");
    expect(partial?.finishReason).toBeUndefined();
    expect(stream.isConversationBusy("agent-a", "agent", "topic-a")).toBe(true);
    expect(useNotificationStore().activeToasts.filter(item => item.title === "流式接续失败")).toHaveLength(1);
    output?.emit({ ...identity, type: "reconnecting", consumerRevision: 141 });
    output?.emit({ ...identity, type: "connected", consumerRevision: 141 });
    expect(partial?.streamRecoveryState).toBeUndefined();
    expect(partial?.isReconnecting).toBe(false);
    output?.emit({
      ...identity, type: "end", consumerRevision: 141,
      content: "durable partial", blocks: [], finishReason: "completed",
    });
    expect(stream.isConversationBusy("agent-a", "agent", "topic-a")).toBe(false);
  });

  it("fills a live group's speaker identity from the receipt without resetting connected state", async () => {
    mockInvoke("reconnect_chat_streams", args => {
      const output = args?.channel as { emit: (event: unknown) => void };
      output.emit({
        type: "connected", messageId: "speaker-message", consumerRevision: 1,
        context: { ownerId: "group-a", ownerType: "group", topicId: "topic-a" },
      });
      return { generations: [{
        msgId: "speaker-message", ownerId: "group-a", ownerType: "group", topicId: "topic-a",
        agentId: "speaker-a", agentName: "Speaker A", createdAt: 10, stopRequested: false,
      }], activeRequests: [], groupTurns: [] };
    });
    const stream = useChatStreamStore();
    await stream.checkAndRecoverInterruptedStreams();
    const message = stream.getActiveStreamMessage("group-a", "group", "topic-a", "speaker-message");
    expect(message).toMatchObject({ name: "Speaker A", agentId: "speaker-a", timestamp: 10, isReconnecting: false });
    expect(message?.shell?.displayName).toBe("Speaker A");
  });

  it("replaces a stale pending UI with persisted history when the inventory no longer contains it", async () => {
    const session = useChatSessionStore();
    session.setConversation({ id: "agent-a", type: "agent" }, "topic-a");
    mockInvoke("load_chat_history", () => []);
    const history = useChatHistoryStore();
    await history.loadHistory("agent-a", "agent", "topic-a");
    const stream = useChatStreamStore();
    await stream.checkAndRecoverInterruptedStreams();
    const output = channelInstances[0];
    const identity = {
      messageId: "already-saved", consumerRevision: 1,
      context: { ownerId: "agent-a", ownerType: "agent", topicId: "topic-a" },
    };
    output?.emit({ ...identity, type: "thinking" });
    mockInvoke("load_chat_history", () => [{
      id: "already-saved", role: "assistant", content: "persisted result", timestamp: 1,
      finishReason: "completed", blocks: [],
    }]);
    await stream.checkAndRecoverInterruptedStreams();
    output?.emit({ ...identity, type: "aurora", aurora: {
      kind: "snapshot", streamId: 1, content: "stale partial", stableBlocks: [],
    } });
    expect(history.currentChatHistory[0]).toMatchObject({ content: "persisted result", finishReason: "completed" });
    expect(stream.isConversationBusy("agent-a", "agent", "topic-a")).toBe(false);
    expect(stream.getActiveStreamMessage("agent-a", "agent", "topic-a", "already-saved")).toBeUndefined();
  });

  it("fences late frames after an external committed deletion", async () => {
    let output: { emit: (event: unknown) => void } | undefined;
    const receipt = deferred<any>();
    mockInvoke("reconnect_chat_streams", args => {
      output = args?.channel as typeof output;
      return receipt.promise;
    });
    const stream = useChatStreamStore();
    const connecting = stream.checkAndRecoverInterruptedStreams();
    const identity = {
      messageId: "deleted", consumerRevision: 150,
      context: { ownerId: "agent-a", ownerType: "agent", topicId: "topic-a" },
    };
    output?.emit({ ...identity, type: "reconnecting" });
    stream.forgetDeletedMessage("agent-a", "agent", "topic-a", "deleted");
    output?.emit({
      ...identity, type: "end", content: "late terminal", finishReason: "completed", blocks: [],
    });
    receipt.resolve({ generations: [{
      msgId: "deleted", topicId: "topic-a", ownerId: "agent-a", ownerType: "agent", createdAt: 1,
    }], activeRequests: [], groupTurns: [] });
    await connecting;
    expect(stream.getActiveStreamMessage("agent-a", "agent", "topic-a", "deleted")).toBeUndefined();
    expect(stream.isMessageActive("agent-a", "agent", "topic-a", "deleted")).toBe(false);
    expect(useNotificationStore().activeToasts).toEqual([]);
  });

  it("inserts an offline durable end during connection without a second recovery command", async () => {
    const session = useChatSessionStore();
    session.setConversation({ id: "agent-a", type: "agent" }, "topic-a");
    mockInvoke("load_chat_history", () => []);
    const history = useChatHistoryStore();
    await history.loadHistory("agent-a", "agent", "topic-a");
    mockInvoke("reconnect_chat_streams", args => {
      const output = args?.channel as { emit: (event: unknown) => void };
      output.emit({
        type: "end", messageId: "offline", consumerRevision: 160,
        context: { ownerId: "agent-a", ownerType: "agent", topicId: "topic-a" },
        content: "offline result", finishReason: "completed",
        blocks: [{ type: "markdown", content: "offline result" }],
      });
      return { generations: [], activeRequests: [], groupTurns: [] };
    });
    await useChatStreamStore().checkAndRecoverInterruptedStreams();
    expect(history.currentChatHistory).toEqual([
      expect.objectContaining({ id: "offline", content: "offline result", isReconnecting: false }),
    ]);
    expect(invokeMock.mock.calls.some(([command]) =>
      command === "recover_active_generation" || command === "get_active_generations")).toBe(false);
  });

  it("keeps group handoff activity and ignores a prior turn's delayed finish", async () => {
    const receipt = deferred<any>();
    let output: { emit: (event: unknown) => void } | undefined;
    mockInvoke("reconnect_chat_streams", args => {
      output = args?.channel as typeof output;
      return receipt.promise;
    });
    const stream = useChatStreamStore();
    const connecting = stream.checkAndRecoverInterruptedStreams();
    const event = (turnId: string, active: boolean) => ({
      type: "group_turn", messageId: "", consumerRevision: 170, chunk: { active },
      context: { ownerId: "group-a", ownerType: "group", topicId: "topic-a", turnId },
    });
    output?.emit(event("new-turn", true));
    receipt.resolve({ generations: [], activeRequests: [], groupTurns: [] });
    await connecting;
    output?.emit(event("old-turn", false));
    expect(stream.isConversationBusy("group-a", "group", "topic-a")).toBe(true);
    output?.emit(event("new-turn", false));
    expect(stream.isConversationBusy("group-a", "group", "topic-a")).toBe(false);
  });

  it("keeps a remote request preparing across WebView recreation without making an empty message", async () => {
    const receipt = deferred<any>();
    let output: { emit: (event: unknown) => void } | undefined;
    mockInvoke("reconnect_chat_streams", args => {
      output = args?.channel as typeof output;
      return receipt.promise;
    });
    const stream = useChatStreamStore();
    const connecting = stream.checkAndRecoverInterruptedStreams();
    const activity = (requestId: string, active: boolean) => ({
      type: "request_activity", messageId: "", consumerRevision: 170, chunk: { active },
      context: { ownerId: "agent-a", ownerType: "agent", topicId: "topic-a", requestId },
    });
    output?.emit(activity("new-attempt", true));
    receipt.resolve({ generations: [], groupTurns: [], activeRequests: [] });
    await connecting;
    expect(stream.isConversationPreparing("agent-a", "agent", "topic-a")).toBe(true);
    expect(stream.isOwnerBusy("agent-a", "agent")).toBe(true);
    expect(stream.activeStreamMessages.size).toBe(0);
    output?.emit(activity("old-attempt", false));
    expect(stream.isConversationBusy("agent-a", "agent", "topic-a")).toBe(true);
    output?.emit(activity("new-attempt", false));
    expect(stream.isConversationBusy("agent-a", "agent", "topic-a")).toBe(false);
  });

  it("settles overlapping request attempts independently of their consumer revisions", async () => {
    const stream = useChatStreamStore();
    await stream.checkAndRecoverInterruptedStreams();
    const output = channelInstances[0];
    const activity = (requestId: string, active: boolean, consumerRevision: number) => ({
      type: "request_activity", messageId: "", consumerRevision, chunk: { active },
      context: { ownerId: "agent-a", ownerType: "agent", topicId: "topic-a", requestId },
    });
    output?.emit(activity("attempt-a", true, 10));
    output?.emit(activity("attempt-b", true, 20));
    output?.emit(activity("attempt-a", false, 11));
    expect(stream.isConversationBusy("agent-a", "agent", "topic-a")).toBe(true);
    output?.emit(activity("attempt-b", false, 21));
    expect(stream.isConversationBusy("agent-a", "agent", "topic-a")).toBe(false);
    output?.emit(activity("attempt-a", true, 10));
    expect(stream.isConversationBusy("agent-a", "agent", "topic-a")).toBe(false);
  });

  it("merges an untouched request from the inventory while another attempt finishes during reconnect", async () => {
    const receipt = deferred<any>();
    let output: { emit: (event: unknown) => void } | undefined;
    mockInvoke("reconnect_chat_streams", args => {
      output = args?.channel as typeof output;
      return receipt.promise;
    });
    const stream = useChatStreamStore();
    const connecting = stream.checkAndRecoverInterruptedStreams();
    const context = { ownerId: "agent-a", ownerType: "agent", topicId: "topic-a" };
    output?.emit({
      type: "request_activity", messageId: "", consumerRevision: 20,
      context: { ...context, requestId: "attempt-b" }, chunk: { active: false },
    });
    receipt.resolve({ generations: [], groupTurns: [], activeRequests: [
      { ...context, requestId: "attempt-a" }, { ...context, requestId: "attempt-b" },
    ] });
    await connecting;
    expect(stream.isConversationPreparing("agent-a", "agent", "topic-a")).toBe(true);
    output?.emit({
      type: "request_activity", messageId: "", consumerRevision: 10,
      context: { ...context, requestId: "attempt-a" }, chunk: { active: false },
    });
    expect(stream.isConversationBusy("agent-a", "agent", "topic-a")).toBe(false);
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

  it("refreshes owner presentation without changing conversation identity", () => {
    const session = useChatSessionStore();
    session.setConversation(
      {
        id: "agent-a",
        name: "Old name",
        type: "agent",
        avatarCalculatedColor: "#111111",
      },
      "topic-a",
    );
    const originalKey = session.currentConversationKey;

    session.setConversation(
      {
        id: "agent-a",
        name: "New name",
        type: "agent",
        avatarCalculatedColor: "#222222",
      },
      "topic-a",
    );

    expect(session.currentConversationKey).toBe(originalKey);
    expect(session.currentSelectedItem).toMatchObject({
      name: "New name",
      avatarCalculatedColor: "#222222",
    });
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

    const append = deferred<{ blocks: any[]; topicUpdatedAt: number }>();
    mockInvoke("append_single_message", () => append.promise);
    const pendingSend = history.sendMessage("hello");

    session.setConversation({ id: "agent-b", type: "agent" }, "topic-b");
    history.resetHistoryForConversation();
    append.resolve({ blocks: [], topicUpdatedAt: 100 });
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
      consumerRevision: 1,
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
      topicUpdatedAt: null,
    });

    expect(
      invokeMock.mock.calls.some(
        ([command]) => command === "append_single_message",
      ),
    ).toBe(false);
  });

  it("submits stop to the original message and waits for its durable end", async () => {
    const session = useChatSessionStore();
    const stream = useChatStreamStore();
    session.setConversation({ id: "agent-a", type: "agent" }, "topic-a");
    await stream.processStreamEvent({
      consumerRevision: 1,
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
      topicUpdatedAt: null,
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
      consumerRevision: 1,
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
      topicUpdatedAt: null,
    });
    interrupt.resolve(undefined);
    await stoppingA;

    expect(
      stream.isMessageActive("agent-a", "agent", "topic-a", "assistant-a"),
    ).toBe(true);
    expect(
      stream.isMessageActiveInSession(
        "agent-b",
        "agent",
        "topic-b",
        "assistant-b",
      ),
    ).toBe(true);

    await stream.processStreamEvent({
      consumerRevision: 1,
      type: "end",
      chunk: null,
      messageId: "assistant-a",
      context: {
        ownerId: "agent-a",
        ownerType: "agent",
        topicId: "topic-a",
        agentId: "agent-a",
      },
      finishReason: "cancelled_by_user",
      error: null,
      content: "partial",
      aurora: null,
      blocks: [],
      timestamp: 123,
      topicUpdatedAt: 124,
    });
    expect(
      stream.isMessageActive("agent-a", "agent", "topic-a", "assistant-a"),
    ).toBe(false);
  });

  it("coalesces connection calls and serializes a Channel only at registration", async () => {
    const receipt = deferred<any>();
    mockInvoke("reconnect_chat_streams", () => receipt.promise);
    const stream = useChatStreamStore();
    const first = stream.checkAndRecoverInterruptedStreams();
    const concurrent = stream.checkAndRecoverInterruptedStreams();
    const calls = invokeMock.mock.calls.filter(([command]) => command === "reconnect_chat_streams");
    expect(calls).toHaveLength(1);
    expect(calls[0]?.[1]?.channel).toBeDefined();
    const connectionId = calls[0]?.[1]?.connectionId;
    receipt.resolve({ generations: [], activeRequests: [], groupTurns: [] });
    await Promise.all([first, concurrent]);
    await stream.checkAndRecoverInterruptedStreams();
    const retry = invokeMock.mock.calls.filter(([command]) => command === "reconnect_chat_streams")[1];
    expect(retry?.[1]).toEqual({ connectionId });
    expect(invokeMock.mock.calls.some(([command]) =>
      command === "get_active_generations" || command === "recover_active_generation")).toBe(false);
  });

  it("replaces the JS channel only after Rust explicitly lost its registration", async () => {
    const stream = useChatStreamStore();
    await stream.checkAndRecoverInterruptedStreams();
    const previousOutput = channelInstances[0];
    let attempts = 0;
    mockInvoke("reconnect_chat_streams", args => {
      attempts += 1;
      if (!args?.channel) return Promise.reject("connection_not_registered");
      return { generations: [], activeRequests: [], groupTurns: [] };
    });
    await stream.checkAndRecoverInterruptedStreams();
    expect(attempts).toBe(2);
    expect(channelInstances).toHaveLength(2);
    const oldEvent = {
      type: "thinking", messageId: "old-channel", consumerRevision: 999,
      context: { ownerId: "agent-a", ownerType: "agent", topicId: "topic-a" },
    };
    previousOutput?.emit(oldEvent);
    expect(stream.getActiveStreamMessage("agent-a", "agent", "topic-a", "old-channel")).toBeUndefined();
    channelInstances[1]?.emit({ ...oldEvent, messageId: "new-channel" });
    expect(stream.isMessageActive("agent-a", "agent", "topic-a", "new-channel")).toBe(true);
  });

  it("does not reregister a channel after a metadata query failed", async () => {
    mockInvoke("reconnect_chat_streams", () => Promise.reject("database temporarily busy"));
    const stream = useChatStreamStore();
    await stream.checkAndRecoverInterruptedStreams();
    expect(stream.activityInventoryKnown).toBe(false);
    mockInvoke("reconnect_chat_streams", () => ({ generations: [], activeRequests: [], groupTurns: [] }));
    await stream.checkAndRecoverInterruptedStreams();
    const calls = invokeMock.mock.calls.filter(([command]) => command === "reconnect_chat_streams");
    expect(calls[0]?.[1]?.channel).toBeDefined();
    expect(calls[1]?.[1]).not.toHaveProperty("channel");
    expect(stream.activityInventoryKnown).toBe(true);
  });

  it("keeps stopping receipts separate from the durable partial", async () => {
    const stream = useChatStreamStore();
    await stream.checkAndRecoverInterruptedStreams();
    const context = { ownerId: "agent-a", ownerType: "agent" as const, topicId: "topic-a" };
    const base = {
      messageId: "stopping", consumerRevision: 1, context,
      chunk: null, finishReason: null, error: null, aurora: null,
      blocks: null, timestamp: null, topicUpdatedAt: null,
    };
    await stream.processStreamEvent({ ...base, type: "thinking" });
    mockInvoke("interruptRequest", () => ({ success: true, status: "cancelling" }));
    await stream.stopMessage("agent-a", "agent", "topic-a", "stopping");
    const partial = stream.getActiveStreamMessage("agent-a", "agent", "topic-a", "stopping");
    expect(partial?.isStopping).toBe(true);
    expect(stream.isMessageActive("agent-a", "agent", "topic-a", "stopping")).toBe(true);
    await stream.processStreamEvent({
      ...base, type: "end", content: "partial", finishReason: "cancelled_by_user", blocks: [],
    });
    expect(partial?.content).toBe("partial");
    expect(partial?.isStopping).toBe(false);
    expect(stream.isMessageActive("agent-a", "agent", "topic-a", "stopping")).toBe(false);
  });

  it("renders a persisted stopping partial in a new WebView without declaring it complete", async () => {
    mockInvoke("process_message_content", args => [{ type: "markdown", content: args?.content }]);
    const stream = useChatStreamStore();
    await stream.checkAndRecoverInterruptedStreams();
    channelInstances[0]?.emit({
      type: "stopping", messageId: "stopped-offline", consumerRevision: 1,
      context: { ownerId: "agent-a", ownerType: "agent", topicId: "topic-a" },
      content: "confirmed partial", error: "helper temporarily unavailable",
    });
    await flushPromises();
    const message = stream.getActiveStreamMessage("agent-a", "agent", "topic-a", "stopped-offline");
    expect(message?.content).toBe("confirmed partial");
    expect(message?.blocks).toEqual([{ type: "markdown", content: "confirmed partial" }]);
    expect(message?.isStopping).toBe(true);
    expect(message?.finishReason).toBeUndefined();
    expect(stream.isConversationBusy("agent-a", "agent", "topic-a")).toBe(true);
  });

  it("lets Rust reconcile offline results and fences late frames after durable completion", async () => {
    const onlineDescriptor = Object.getOwnPropertyDescriptor(navigator, "onLine");
    Object.defineProperty(navigator, "onLine", { configurable: true, value: false });
    try {
      let output: { emit: (event: unknown) => void } | undefined;
      const event = {
        messageId: "offline", consumerRevision: 1,
        context: { ownerId: "agent-a", ownerType: "agent", topicId: "topic-a" },
      };
      mockInvoke("reconnect_chat_streams", args => {
        output = args?.channel as typeof output;
        output?.emit({ ...event, type: "end", content: "durable offline result", finishReason: "completed", blocks: [] });
        return { generations: [], activeRequests: [], groupTurns: [] };
      });
      const stream = useChatStreamStore();
      await stream.checkAndRecoverInterruptedStreams();
      output?.emit({ ...event, type: "thinking" });
      const recovered = stream.getActiveStreamMessage("agent-a", "agent", "topic-a", "offline");
      expect(recovered?.content).toBe("durable offline result");
      expect(recovered?.isThinking).toBe(false);
      expect(stream.isMessageActive("agent-a", "agent", "topic-a", "offline")).toBe(false);
    } finally {
      if (onlineDescriptor) Object.defineProperty(navigator, "onLine", onlineDescriptor);
      else Reflect.deleteProperty(navigator, "onLine");
    }
  });

});

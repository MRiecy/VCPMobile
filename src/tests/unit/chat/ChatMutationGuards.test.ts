import { beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { mount } from "@vue/test-utils";
import { useChatHistoryStore } from "@/core/stores/chatHistoryStore";
import { useChatSessionStore } from "@/core/stores/chatSessionStore";
import { useChatStreamStore } from "@/core/stores/chatStreamStore";
import { useAssistantStore } from "@/core/stores/assistant";
import { useTopicStore } from "@/core/stores/topicListManager";
import { useSettingsStore } from "@/core/stores/settings";
import { useNotificationStore } from "@/core/stores/notification";
import { useAttachmentStore } from "@/core/stores/attachmentStore";
import { chatMutationBlockReason } from "@/features/chat/chatMutationPolicy";
import GroupStopAllButton from "@/features/chat/components/GroupStopAllButton.vue";
import { invokeMock, mockInvoke } from "@/tests/mocks/tauri";
import { makeConfigSnapshot, makeSettings } from "@/tests/helpers/config";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}

const conversation = { ownerId: "agent-a", ownerType: "agent" as const, topicId: "topic-a" };

function loadedHistory() {
  const session = useChatSessionStore();
  session.setConversation({ id: conversation.ownerId, type: conversation.ownerType }, conversation.topicId);
  const history = useChatHistoryStore();
  history.loadedConversationKey = session.currentConversationKey;
  history.currentChatHistory = [{ id: "old", role: "user", content: "original", timestamp: 1, blocks: [] }];
  return history;
}

describe("chat mutations during generation", () => {
  let reconnectOutput: { emit: (event: unknown) => void };
  beforeEach(async () => {
    setActivePinia(createPinia());
    mockInvoke("reconnect_chat_streams", args => {
      if (args?.channel) reconnectOutput = args.channel as typeof reconnectOutput;
      return { generations: [], groupTurns: [], activeRequests: [] };
    });
    await useChatStreamStore().checkAndRecoverInterruptedStreams();
    invokeMock.mockClear();
  });

  it('keeps ordinary attachments and the composer receipt untouched when persistence fails', async () => {
    const history = loadedHistory();
    const attachments = useAttachmentStore();
    attachments.stagedAttachments = [{ id: 'draft-file', name: 'draft.txt', type: 'text/plain', src: '', size: 4, status: 'done' }];
    const acknowledge = vi.fn();
    mockInvoke('append_single_message', () => Promise.reject(new Error('CAS unavailable')));
    await history.sendMessage('retryable draft', acknowledge);
    expect(acknowledge).not.toHaveBeenCalled();
    expect(attachments.stagedAttachments.map(item => item.id)).toEqual(['draft-file']);
    expect(history.currentChatHistory.map(item => item.id)).toEqual(['old']);
    expect(history.isSubmitting).toBe(false);
  });

  it.each(['', null])('uses the saved username %s or the generated default only when settings are absent', async userName => {
    const history = loadedHistory();
    useSettingsStore().snapshot = userName === null ? null : makeConfigSnapshot(makeSettings({ 'identity.userName': userName }));
    mockInvoke('append_single_message', () => ({ blocks: [], topicUpdatedAt: 2 }));
    mockInvoke('handle_agent_chat_message', () => undefined);
    await history.sendMessage('name boundary');
    expect(invokeMock).toHaveBeenCalledWith('append_single_message', expect.objectContaining({
      message: expect.objectContaining({ name: userName ?? makeSettings().identity.userName }),
    }));
  });

  it('consumes only the committed draft before generation finishes and rejects double submission', async () => {
    const history = loadedHistory();
    useSettingsStore().snapshot = makeConfigSnapshot(makeSettings());
    const attachments = useAttachmentStore();
    attachments.stagedAttachments = [{ id: 'sent-file', name: 'sent.txt', type: 'text/plain', src: '', size: 4, status: 'done' }];
    const append = deferred<{ blocks: []; topicUpdatedAt: number }>();
    const generation = deferred<never>();
    mockInvoke('append_single_message', () => append.promise);
    mockInvoke('handle_agent_chat_message', () => generation.promise);
    const acknowledge = vi.fn();
    const sending = history.sendMessage('submitted text', acknowledge);
    await history.sendMessage('duplicate click', acknowledge);
    expect(invokeMock.mock.calls.filter(([name]) => name === 'append_single_message')).toHaveLength(1);
    expect(history.isSubmitting).toBe(true);
    attachments.stagedAttachments.push({ id: 'later-file', name: 'later.txt', type: 'text/plain', src: '', size: 5, status: 'done' });
    append.resolve({ blocks: [], topicUpdatedAt: 2 });
    await vi.waitFor(() => expect(acknowledge).toHaveBeenCalledTimes(1));
    expect(history.isSubmitting).toBe(false);
    expect(attachments.stagedAttachments.map(item => item.id)).toEqual(['later-file']);
    generation.reject(new Error('model unavailable'));
    await sending;
    expect(acknowledge).toHaveBeenCalledTimes(1);
    expect(history.currentChatHistory[history.currentChatHistory.length - 1]?.content).toBe('submitted text');
    expect(attachments.stagedAttachments.map(item => item.id)).toEqual(['later-file']);
  });

  it('keeps the edit intent and history until the atomic edit receipt succeeds', async () => {
    const history = loadedHistory();
    history.currentChatHistory.push({ id: 'later', role: 'assistant', content: 'keep on failure', timestamp: 2, blocks: [] });
    await history.beginEditResend(history.captureMessageActionKey('old')!);
    const intent = history.editingMessage;
    const acknowledge = vi.fn();
    mockInvoke('edit_resend_message', () => Promise.reject(new Error('write failed')));
    await history.sendMessage('edited body', acknowledge);
    expect(history.editingMessage).toBe(intent);
    expect(history.currentChatHistory.map(item => item.content)).toEqual(['original', 'keep on failure']);
    expect(acknowledge).not.toHaveBeenCalled();
    expect(invokeMock.mock.calls.some(([name]) => name === 'truncate_history_after_message')).toBe(false);
    expect(invokeMock.mock.calls.some(([name]) => name === 'append_single_message')).toBe(false);

    useSettingsStore().snapshot = makeConfigSnapshot(makeSettings());
    mockInvoke('edit_resend_message', () => ({ blocks: [{ type: 'markdown', content: 'edited body' }], topicUpdatedAt: 3, msgCount: 1 }));
    await history.sendMessage('edited body', acknowledge);
    expect(acknowledge).toHaveBeenCalledTimes(1);
    expect(history.editingMessage).toBeNull();
    expect(history.currentChatHistory.map(item => item.content)).toEqual(['edited body']);
    expect(invokeMock.mock.calls.some(([name]) => name === 'append_single_message')).toBe(false);
  });

  it("rejects message, topic and owner removal while a send is still preparing, then unlocks after failure", async () => {
    const append = deferred<never>();
    mockInvoke("append_single_message", () => append.promise);
    const history = loadedHistory();
    const key = history.captureMessageActionKey("old")!;
    const sending = history.sendMessage("new message");

    // There is no thinking/message ID yet: the real append call is still pending.
    expect(useChatStreamStore().activeStreamingIds.size).toBe(0);
    expect(chatMutationBlockReason(conversation)).not.toBeNull();
    expect(chatMutationBlockReason({ ...conversation, topicId: "other-topic" })).toBeNull();
    expect(chatMutationBlockReason({ ownerId: "other-agent", ownerType: "agent" })).toBeNull();

    await history.deleteMessage(key);
    await history.beginEditResend(key);
    await history.regenerateResponse(key);
    await useTopicStore().deleteTopic(conversation.ownerId, conversation.ownerType, conversation.topicId);
    expect(await useAssistantStore().deleteAgent(conversation.ownerId)).toBe(false);
    expect(history.currentChatHistory.some(message => message.id === "old")).toBe(true);
    expect(history.editingMessage).toBeNull();

    append.reject(new Error("storage unavailable"));
    await sending;
    expect(chatMutationBlockReason(conversation)).toBeNull();
    mockInvoke("delete_messages", () => ({ msgCount: 0, updatedAt: 2 }));
    await history.deleteMessage(key);
    expect(invokeMock).toHaveBeenCalledWith("delete_messages", expect.objectContaining({ msgIds: ["old"] }));
  });

  it("rechecks an already opened edit-resend intent before truncating history", async () => {
    const history = loadedHistory();
    await history.beginEditResend(history.captureMessageActionKey("old")!);
    expect(history.editingMessage).not.toBeNull();
    const release = useChatStreamStore().beginConversationOperation("agent-a", "agent", "topic-a");
    await history.sendMessage("edited");
    expect(invokeMock).not.toHaveBeenCalledWith("truncate_history_after_message", expect.anything());
    expect(history.editingMessage?.initialContent).toBe("original");
    expect(history.currentChatHistory[0].content).toBe("original");
    release();
  });

  it("protects an Agent used by an active group even when another conversation is selected", async () => {
    const assistant = useAssistantStore();
    assistant.groups = [{ id: "group-a", name: "Group", members: ["agent-a"], mode: "sequential" }] as any;
    loadedHistory();
    const release = useChatStreamStore().beginConversationOperation("group-a", "group", "group-topic");
    expect(chatMutationBlockReason(conversation)).toBeNull();
    expect(await assistant.deleteAgent("agent-a")).toBe(false);
    expect(await assistant.deleteGroup("group-a")).toBe(false);
    expect(invokeMock).not.toHaveBeenCalled();
    release();
    expect(chatMutationBlockReason({ ownerId: "agent-a", ownerType: "agent" })).toBeNull();
  });

  it("keeps a group turn stoppable between speakers and blocks removing its topic", async () => {
    useChatSessionStore().setConversation({ id: "group-a", type: "group" }, "group-topic");
    mockInvoke("reconnect_chat_streams", () => ({
      generations: [],
      groupTurns: [{ ownerId: "group-a", ownerType: "group", topicId: "group-topic", turnId: "turn-a" }],
      activeRequests: [],
    }));
    await useChatStreamStore().checkAndRecoverInterruptedStreams();
    expect(useChatStreamStore().activeStreamingIds.size).toBe(0);
    const wrapper = mount(GroupStopAllButton);
    expect(wrapper.find("button").exists()).toBe(true);
    await useTopicStore().deleteTopic("group-a", "group", "group-topic");
    expect(invokeMock).not.toHaveBeenCalledWith("delete_topic", expect.anything());
    await wrapper.find("button").trigger("click");
    expect(invokeMock).toHaveBeenCalledWith("interruptGroupTurn", {
      ownerId: "group-a", ownerType: "group", topicId: "group-topic",
    });
    expect(invokeMock).not.toHaveBeenCalledWith("interruptRequest", expect.anything());
    wrapper.unmount();
  });

  it.each([true, false])("reports one failure from the real global channel or an unreported preflight rejection (pending: %s)", async recoveryPending => {
    const history = loadedHistory();
    useSettingsStore().snapshot = makeConfigSnapshot(makeSettings({
      "server.url": "http://vcp.test", "server.apiKey": "key",
    }));
    mockInvoke("append_single_message", () => ({ blocks: [], topicUpdatedAt: 1 }));
    mockInvoke("handle_agent_chat_message", () => {
      if (!recoveryPending) return Promise.reject(new Error("invalid configuration"));
      // Production routes output to the registered reconnect consumer, not the
      // command's original Channel. The command receipt does not own its state.
      reconnectOutput.emit({
        type: "error", messageId: "pending", consumerRevision: 1,
        context: conversation, content: "connection lost",
      });
      return { status: "recovery_pending", messageId: "pending" };
    });
    await history.sendMessage("hello");
    const errors = useNotificationStore().activeToasts.filter(notification => notification.type === "error");
    expect(errors).toHaveLength(1);
    expect(errors[0].title).toBe(recoveryPending ? "流式接续失败" : "消息生成失败");
    expect(useChatStreamStore().isConversationPreparing("agent-a", "agent", "topic-a")).toBe(false);
    expect(useChatStreamStore().isConversationBusy("agent-a", "agent", "topic-a")).toBe(recoveryPending);
  });

});

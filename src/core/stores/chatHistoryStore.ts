import { defineStore } from "pinia";
import { nextTick, ref } from "vue";
import { invoke, Channel } from "@tauri-apps/api/core";
import { useChatSessionStore } from "./chatSessionStore";
import { useChatStreamStore } from "./chatStreamStore";
import { useAttachmentStore } from "./attachmentStore";
import { useAssistantStore } from "./assistant";
import { useSettingsStore } from "./settings";
import { useTopicStore } from "./topicListManager";
import { useNotificationStore } from "./notification";
import { clearMessageCache } from "../utils/astRenderer";
import { extractMentionedMemberIds } from "../utils/mention";

import type { ChatMessage, ContentBlock } from "../types/chat";
import type {
  ConversationKey,
  ConversationOwnerType,
} from "./chatSessionStore";

export interface HistoryPageResult {
  addedCount: number;
  error?: unknown;
  aborted?: boolean;
}

type AnchorLoadResult = "loaded" | "missing" | "aborted";

export interface MessageActionKey {
  conversation: ConversationKey;
  messageId: string;
}

interface EditingMessageIntent {
  key: MessageActionKey;
  initialContent: string;
}

export const MAX_HISTORY_MESSAGES = 500;

const compareMessages = (a: ChatMessage, b: ChatMessage) => {
  if (a.timestamp !== b.timestamp) return a.timestamp - b.timestamp;
  // 与 SQLite 默认 BINARY collation 对齐，避免同毫秒消息在 keyset 游标处错位。
  return a.id < b.id ? -1 : a.id > b.id ? 1 : 0;
};

const mergeHistoryWindow = (
  existing: ChatMessage[],
  incoming: ChatMessage[],
  loadingOlder: boolean,
) => {
  const byId = new Map(existing.map(message => [message.id, message]));
  incoming.forEach(message => byId.set(message.id, message));
  const merged = Array.from(byId.values()).sort(compareMessages);
  if (merged.length <= MAX_HISTORY_MESSAGES) return merged;
  return loadingOlder
    ? merged.slice(0, MAX_HISTORY_MESSAGES)
    : merged.slice(-MAX_HISTORY_MESSAGES);
};

const sameConversation = (a: ConversationKey | null, b: ConversationKey | null) =>
  Boolean(
    a &&
      b &&
      a.ownerId === b.ownerId &&
      a.ownerType === b.ownerType &&
      a.topicId === b.topicId &&
      a.epoch === b.epoch,
  );

export const useChatHistoryStore = defineStore("chatHistory", () => {
  const currentChatHistory = ref<ChatMessage[]>([]);
  const loading = ref(false);

  // 分页加载状态
  const hasMoreHistory = ref(true);    // 是否还有更多旧消息
  const isLoadingHistory = ref(false); // 防止并发重复触发
  const hasEvictedNewer = ref(false);  // 深翻历史触发窗口裁剪后，最新端需要显式重载
  const loadedConversationKey = ref<ConversationKey | null>(null);

  // 启动预加载缓存：PRELOADING 阶段提前拉取首屏历史，ChatView mount 后直接消费
  const preloadedHistory = ref<{ key: ConversationKey; messages: ChatMessage[] } | null>(null);
  let preloadConsumed = false;

  // “编辑重发”意图必须绑定完整会话身份，避免异步读取后误落到其他话题。
  const editingMessage = ref<EditingMessageIntent | null>(null);

  // 用于防止并发加载与话题切换导致竞态的消息拉取中止控制器 (AbortController)
  let currentLoadAbortController: AbortController | null = null;
  let currentLoadId = 0;
  let currentAnchorLoadId = 0;

  const sessionStore = useChatSessionStore();
  const streamStore = useChatStreamStore();
  const attachmentStore = useAttachmentStore();
  const assistantStore = useAssistantStore();
  const settingsStore = useSettingsStore();
  const topicStore = useTopicStore();
  const notificationStore = useNotificationStore();

  const errorMessage = (error: unknown) =>
    error instanceof Error ? error.message : String(error);
  const isMissingTopicError = (error: unknown) =>
    errorMessage(error) === "topic does not belong to the selected owner";
  const notifyHistoryLoadFailure = () => {
    notificationStore.addNotification({
      type: "error",
      title: "历史加载失败",
      message: "暂时无法读取聊天记录，请稍后重试",
      toastOnly: true,
    });
  };

  const captureLoadedConversation = (): ConversationKey | null => {
    const key = sessionStore.currentConversationKey;
    return sameConversation(loadedConversationKey.value, key) && key ? { ...key } : null;
  };

  const canCommitConversation = (key: ConversationKey) =>
    sessionStore.isConversationCurrent(key) && sameConversation(loadedConversationKey.value, key);

  const captureMessageActionKey = (messageId: string): MessageActionKey | null => {
    const conversation = captureLoadedConversation();
    return conversation ? { conversation, messageId } : null;
  };

  const isMessageActionCurrent = (key: MessageActionKey) =>
    canCommitConversation(key.conversation);

  /**
   * 启动预加载：在 PRELOADING 阶段提前拉取首屏聊天历史
   * 让 DB + IPC 开销与 Vue 组件挂载并行，ChatView mount 后直接命中缓存
   */
  const preloadHistory = async (
    ownerId: string,
    ownerType: ConversationOwnerType,
    topicId: string,
    limit: number = 5,
  ) => {
    const key = sessionStore.currentConversationKey;
    if (
      !key ||
      key.ownerId !== ownerId ||
      key.ownerType !== ownerType ||
      key.topicId !== topicId
    ) {
      return;
    }
    try {
      const messages = await invoke<ChatMessage[]>('load_chat_history', {
        ownerId, ownerType, topicId, limit, offset: 0,
        beforeTimestamp: null, beforeMessageId: null,
      });
      if (!sessionStore.isConversationCurrent(key)) return;
      preloadedHistory.value = { key, messages };
      console.log(`[ChatHistoryStore] Preloaded ${messages.length} messages for topic ${topicId}`);
    } catch (e) {
      console.error('[ChatHistoryStore] Preload failed:', e);
      preloadedHistory.value = null;
    }
  };

  /**
   * 尝试为话题生成 AI 总结标题
   * 触发条件：消息数 >= 4 且标题仍为初始的 "新话题 HH:MM:SS" 格式
   */
  const summarizeTopic = async () => {
    if (!sessionStore.currentTopicId || !sessionStore.currentSelectedItem?.id) return;

    const topicId = sessionStore.currentTopicId;
    const ownerId = sessionStore.currentSelectedItem.id;
    const ownerType = sessionStore.currentSelectedItem.type;

    const topic = topicStore.topics.find(
      (candidate) =>
        candidate.id === topicId &&
        candidate.ownerId === ownerId &&
        candidate.ownerType === ownerType,
    );
    const defaultTitle = topic?.name;
    const isDefaultName =
      defaultTitle && /^(新话题|新会话) \d{2}:\d{2}:\d{2}$/.test(defaultTitle);
    const messageCount = currentChatHistory.value.filter(
      (m) => m.role !== "system",
    ).length;

    if (isDefaultName && messageCount >= 4) {
      console.log(`[ChatHistoryStore] Triggering AI summary for topic: ${topicId}`);
      try {
        const agentName = ownerType === "agent"
          ? assistantStore.agents.find((a: any) => a.id === ownerId)?.name || "AI"
          : assistantStore.groups.find((g) => g.id === ownerId)?.name || "AI";
        const newTitle = await invoke<string>("summarize_topic", {
          ownerId,
          ownerType,
          topicId,
          agentName,
        });

        if (newTitle) {
          console.log(`[ChatHistoryStore] AI Summarized Title: ${newTitle}`);
          await topicStore.updateTopicTitle(
            ownerId,
            ownerType,
            topicId,
            newTitle,
            defaultTitle,
          );
        }
      } catch (e) {
        console.error("[ChatHistoryStore] AI Summary failed:", e);
      }
    }
  };

  /**
   * 加载聊天历史
   */
  const loadHistory = async (
    ownerId: string,
    ownerType: ConversationOwnerType,
    topicId: string,
    limit: number = 15,
    loadingOlder = false,
  ): Promise<HistoryPageResult> => {
    const key = sessionStore.currentConversationKey;
    if (
      !key ||
      key.ownerId !== ownerId ||
      key.ownerType !== ownerType ||
      key.topicId !== topicId
    ) {
      return { addedCount: 0, aborted: true };
    }
    const loadId = ++currentLoadId;
    const oldest = loadingOlder ? currentChatHistory.value[0] : undefined;
    if (loadingOlder && !oldest) return { addedCount: 0, aborted: true };
    console.log(
      `[ChatHistoryStore] Loading ${loadingOlder ? "older" : "latest"} history for ${ownerId}, topic: ${topicId}, limit: ${limit}`,
    );
    if (currentLoadAbortController) {
      currentLoadAbortController.abort();
    }
    const controller = new AbortController();
    currentLoadAbortController = controller;
    const { signal } = controller;
    loading.value = true;
    isLoadingHistory.value = true;

    try {
      let messages: ChatMessage[];
      if (
        !loadingOlder &&
        !preloadConsumed &&
        preloadedHistory.value &&
        sameConversation(preloadedHistory.value.key, key)
      ) {
        messages = preloadedHistory.value.messages;
        preloadedHistory.value = null;
        preloadConsumed = true;
      } else {
        preloadConsumed = true;
        messages = await invoke<ChatMessage[]>('load_chat_history', {
          ownerId, ownerType, topicId, limit,
          // 后端分页使用稳定游标；offset=0 只标识最新窗口读取。
          offset: loadingOlder ? null : 0,
          beforeTimestamp: oldest?.timestamp ?? null,
          beforeMessageId: oldest?.id ?? null,
        });
      }

      if (
        signal.aborted ||
        loadId !== currentLoadId ||
        !sessionStore.isConversationCurrent(key)
      ) {
        return { addedCount: 0, aborted: true };
      }

      const hydrated = messages.map(
        msg => streamStore.getActiveStreamMessage(ownerId, ownerType, topicId, msg.id) || msg,
      );
      let addedCount = hydrated.length;
      if (!loadingOlder) {
        currentChatHistory.value = mergeHistoryWindow([], hydrated, false);
        hasEvictedNewer.value = false;
      } else {
        const existingIds = new Set(currentChatHistory.value.map(message => message.id));
        const unique = hydrated.filter(message => !existingIds.has(message.id));
        if (currentChatHistory.value.length + unique.length > MAX_HISTORY_MESSAGES) {
          hasEvictedNewer.value = true;
        }
        currentChatHistory.value = mergeHistoryWindow(
          currentChatHistory.value,
          unique,
          true,
        );
        addedCount = unique.length;
      }
      loadedConversationKey.value = key;
      hasMoreHistory.value = messages.length >= limit;
      hydrated.forEach(msg => attachmentStore.resolveMessageAssets(msg));
      if (!loadingOlder) {
        streamStore.checkAndRecoverInterruptedStreams().catch((error) => {
          console.error("[ChatHistoryStore] Failed to recover streams after loading history:", error);
        });
      }
      return { addedCount };
    } catch (e) {
      console.error("[ChatHistoryStore] Failed to load history:", e);
      if (
        signal.aborted ||
        loadId !== currentLoadId ||
        !sessionStore.isConversationCurrent(key)
      ) {
        return { addedCount: 0, aborted: true };
      }
      if (isMissingTopicError(e)) {
        sessionStore.clearCurrentTopic();
        return { addedCount: 0, aborted: true };
      }
      notifyHistoryLoadFailure();
      return { addedCount: 0, error: e };
    } finally {
      if (
        currentLoadAbortController === controller &&
        loadId === currentLoadId &&
        sessionStore.isConversationCurrent(key)
      ) {
        currentLoadAbortController = null;
        loading.value = false;
        isLoadingHistory.value = false;
      }
    }
  };

  const resetHistoryForConversation = () => {
    currentLoadId += 1;
    currentLoadAbortController?.abort();
    currentLoadAbortController = null;
    currentChatHistory.value = [];
    loadedConversationKey.value = null;
    hasMoreHistory.value = true;
    hasEvictedNewer.value = false;
    loading.value = false;
    isLoadingHistory.value = false;
    editingMessage.value = null;
  };

  const loadHistoryPaginated = async (
    ownerId: string,
    ownerType: ConversationOwnerType,
    topicId: string,
  ) => {
    resetHistoryForConversation();
    return loadHistory(ownerId, ownerType, topicId, 5);
  };

  const loadMoreHistory = async (): Promise<HistoryPageResult> => {
    const key = sessionStore.currentConversationKey;
    if (
      !key ||
      !sameConversation(loadedConversationKey.value, key) ||
      !hasMoreHistory.value ||
      isLoadingHistory.value
    ) {
      return { addedCount: 0, aborted: true };
    }
    return loadHistory(
      key.ownerId,
      key.ownerType,
      key.topicId,
      10,
      true,
    );
  };

  const returnToLatest = async (): Promise<HistoryPageResult> => {
    if (!hasEvictedNewer.value) return { addedCount: 0 };
    const key = sessionStore.currentConversationKey;
    if (!key || !sameConversation(loadedConversationKey.value, key)) {
      return { addedCount: 0, aborted: true };
    }
    return loadHistory(key.ownerId, key.ownerType, key.topicId, 15);
  };

  /**
   * 锚点窗口加载（全局搜索跳转定位）：以 anchorMessageId 为中心，
   * 用"前 beforeN + 锚点 + 后 afterN"的消息窗口整体替换当前历史。
   * 读取成功前不切换会话；独立序号保证连续点击时只有最后一次可以提交。
   */
  const loadHistoryAround = async (
    ownerId: string,
    ownerType: ConversationOwnerType,
    topicId: string,
    anchorMessageId: string,
    beforeN = 12,
    afterN = 8,
  ): Promise<AnchorLoadResult> => {
    const sourceEpoch = sessionStore.sessionEpoch;
    const anchorLoadId = ++currentAnchorLoadId;
    let messages: ChatMessage[];
    try {
      messages = await invoke<ChatMessage[]>('load_chat_history_around', {
        ownerId,
        ownerType,
        topicId,
        anchorMsgId: anchorMessageId,
        beforeN,
        afterN,
      });
    } catch (e) {
      console.error("[ChatHistoryStore] Failed to load history around anchor:", e);
      if (
        anchorLoadId !== currentAnchorLoadId ||
        sessionStore.sessionEpoch !== sourceEpoch
      ) {
        return "aborted";
      }
      if (isMissingTopicError(e)) {
        return "missing";
      }
      throw e;
    }

    if (
      anchorLoadId !== currentAnchorLoadId ||
      sessionStore.sessionEpoch !== sourceEpoch
    ) {
      return "aborted";
    }

    const anchorIndex = messages.findIndex((message) => message.id === anchorMessageId);
    if (anchorIndex === -1) return "missing";

    const hydrated = messages.map(
      message => streamStore.getActiveStreamMessage(ownerId, ownerType, topicId, message.id) || message,
    );
    const current = sessionStore.currentConversationKey;
    if (
      !current ||
      current.ownerId !== ownerId ||
      current.ownerType !== ownerType ||
      current.topicId !== topicId
    ) {
      await sessionStore.selectTopicById(ownerId, ownerType, topicId);
      await nextTick();
    }
    if (anchorLoadId !== currentAnchorLoadId) return "aborted";

    const targetKey = sessionStore.currentConversationKey;
    if (
      !targetKey ||
      targetKey.ownerId !== ownerId ||
      targetKey.ownerType !== ownerType ||
      targetKey.topicId !== topicId
    ) {
      return "aborted";
    }

    resetHistoryForConversation();
    currentChatHistory.value = mergeHistoryWindow([], hydrated, false);
    loadedConversationKey.value = targetKey;
    // 锚点上方未取满 beforeN 说明已触顶；下方未取满 afterN 说明已在最新端
    hasMoreHistory.value = anchorIndex >= beforeN;
    hasEvictedNewer.value = messages.length - anchorIndex - 1 >= afterN;
    hydrated.forEach(message => attachmentStore.resolveMessageAssets(message));
    streamStore.checkAndRecoverInterruptedStreams().catch((error) => {
      console.error("[ChatHistoryStore] Failed to recover streams after anchor load:", error);
    });
    return "loaded";
  };

  /**
   * 构造群聊/单聊共用的流式事件 Channel 接线
   */
  const makeStreamChannel = (key: ConversationKey) => {
    const streamChannel = new Channel<any>();
    streamChannel.onmessage = (event) => streamStore.processStreamEvent(event, {
      onMessageCreated: (msg, tid) => {
        if (tid === key.topicId && canCommitConversation(key) && !currentChatHistory.value.some(m => m.id === msg.id)) {
          currentChatHistory.value.push(msg);
          currentChatHistory.value = mergeHistoryWindow(
            currentChatHistory.value,
            [],
            false,
          );
        }
      },
      onStreamFinished: (_mid, tid) => {
        if (tid === key.topicId && canCommitConversation(key)) {
          summarizeTopic();
        }
      }
    });
    return streamChannel;
  };

  /**
   * 邀请群成员单人发言（invite_only 模式入口）
   */
  const inviteGroupMember = async (agentId: string, frozenKey?: ConversationKey) => {
    const key = frozenKey || captureLoadedConversation();
    if (!key || key.ownerType !== "group") return;
    try {
      const settings = settingsStore.settings;
      if (!settings) throw new Error("应用尚未完成初始化");

      const streamChannel = makeStreamChannel(key);
      await invoke("invite_group_member_to_speak", {
        payload: {
          groupId: key.ownerId,
          topicId: key.topicId,
          agentId,
          vcpUrl: settings.vcpServerUrl || "",
          vcpApiKey: settings.vcpApiKey || "",
        },
        streamChannel,
      });
    } catch (e) {
      console.error("[ChatHistoryStore] Invite failed:", e);
      // 邀请链路失败绝不能静默：必须给用户可见反馈
      notificationStore.addNotification({
        type: "error",
        title: "邀请发言失败",
        message: e instanceof Error ? e.message : String(e),
        toastOnly: true,
        duration: 6000,
      });
    }
  };

  /**
   * 触发 AI 生成逻辑
   */
  const triggerGeneration = async (userMsg: ChatMessage, frozenKey?: ConversationKey) => {
    const key = frozenKey || captureLoadedConversation();
    if (!key) return;
    try {
      const compiledBlocks = await invoke<ContentBlock[]>("append_single_message", {
        ownerId: key.ownerId,
        ownerType: key.ownerType,
        topicId: key.topicId,
        message: {
          ...userMsg,
          blocks: undefined, // 强行设为 undefined，迫使后端执行真正的编译，生成 markdown AST 节点与表情包匹配
        },
      });

      const targetIndex = currentChatHistory.value.findIndex(m => m.id === userMsg.id);
      if (canCommitConversation(key) && targetIndex !== -1) {
        currentChatHistory.value[targetIndex] = {
          ...currentChatHistory.value[targetIndex],
          blocks: compiledBlocks as any,
        };
      }

      const settings = settingsStore.settings;
      if (!settings) throw new Error("应用尚未完成初始化");

      const streamChannel = makeStreamChannel(key);

      if (key.ownerType === "group") {
        const result = await invoke<{ status?: string; reason?: string }>("handle_group_chat_message", {
          payload: {
            groupId: key.ownerId,
            topicId: key.topicId,
            userMessage: userMsg,
            vcpUrl: settings.vcpServerUrl || "",
            vcpApiKey: settings.vcpApiKey || "",
          },
          streamChannel
        });
        // 群组回合可能合法地"无人发言"（如未实现的发言模式），必须显式可见，
        // 否则用户看到的是"发送后没有任何反应"
        if (result?.status === "no_ai_response") {
          if (result.reason === "invite_only") {
            // 提及即邀约：消息中 @ 到的成员按出现顺序依次单人发言（群聊串行约束）；
            // 未提及任何成员时引导用户使用邀约横条
            const group = assistantStore.groups.find((g) => g.id === key.ownerId);
            const mentionedIds = group
              ? extractMentionedMemberIds(
                  userMsg.content ?? "",
                  group.members.map((id) => ({
                    id,
                    name: assistantStore.agents.find((a: any) => a.id === id)?.name ?? "",
                  })),
                )
              : [];
            if (mentionedIds.length > 0) {
              for (const agentId of mentionedIds) {
                await inviteGroupMember(agentId, key);
              }
            } else {
              notificationStore.addNotification({
                type: "info",
                title: "群组未产生回复",
                message: "邀请发言模式下成员不会自动回复，可点击上方成员头像邀约，或在消息中 @ 成员后发送。",
                toastOnly: true,
                duration: 6000,
              });
            }
          } else {
            const message = result.reason === "mode_not_implemented"
              ? "当前发言模式尚未实现，请在群组设置中改为「顺序发言」或「自然随机」。"
              : "没有成员满足发言条件，可尝试 @提及 成员或调整发言模式。";
            notificationStore.addNotification({
              type: "info",
              title: "群组未产生回复",
              message,
              toastOnly: true,
              duration: 6000,
            });
          }
        }
      } else {
        await invoke("handle_agent_chat_message", { 
          payload: {
            agentId: key.ownerId,
            topicId: key.topicId,
            userMessage: userMsg,
            vcpUrl: settings.vcpServerUrl || "",
            vcpApiKey: settings.vcpApiKey || "",
          }, 
          streamChannel 
        });
      }
    } catch (e) {
      console.error("[ChatHistoryStore] Generation failed:", e);
      // 生成链路失败绝不能静默：release 下控制台不可见，必须给用户可见反馈
      notificationStore.addNotification({
        type: "error",
        title: "消息生成失败",
        message: e instanceof Error ? e.message : String(e),
        toastOnly: true,
        duration: 6000,
      });
    }
  };

  /**
   * 发送消息
   */
  const sendMessage = async (content: string) => {
    if (hasEvictedNewer.value && !editingMessage.value) {
      const latest = await returnToLatest();
      if (latest.error || latest.aborted || hasEvictedNewer.value) return;
    }
    const key = captureLoadedConversation();
    if (!key || (!content.trim() && attachmentStore.stagedAttachments.length === 0)) return;
    if (attachmentStore.stagedAttachments.some(attachment => attachment.status !== "done")) return;

    if (typeof navigator !== "undefined" && navigator.vibrate) {
      navigator.vibrate(25);
    }

    if (editingMessage.value) {
      const intent = editingMessage.value;
      editingMessage.value = null;
      if (!sameConversation(intent.key.conversation, key)) return;
      const originalId = intent.key.messageId;
      const targetIndex = currentChatHistory.value.findIndex(m => m.id === originalId);
      if (targetIndex === -1) return;
      const targetMsg = {
        ...currentChatHistory.value[targetIndex],
        content,
        blocks: [{ type: "markdown" as const, content }],
      };
      try {
        const msgCount = await invoke<number>("truncate_history_after_message", {
          ownerId: key.ownerId,
          ownerType: key.ownerType,
          topicId: key.topicId,
          anchorMessageId: originalId,
        });
        topicStore.setTopicMsgCount(key.ownerId, key.ownerType, key.topicId, msgCount);
        if (canCommitConversation(key)) {
          const currentIndex = currentChatHistory.value.findIndex(message => message.id === originalId);
          if (currentIndex !== -1) {
            currentChatHistory.value[currentIndex] = targetMsg;
            currentChatHistory.value = currentChatHistory.value.slice(0, currentIndex + 1);
          }
        }
        await triggerGeneration(targetMsg, key);
      } catch (e) {
        if (canCommitConversation(key)) editingMessage.value = intent;
        notificationStore.addNotification({
          type: "error",
          title: "编辑重发失败",
          message: errorMessage(e),
          toastOnly: true,
          duration: 6000,
        });
      }
      return;
    }

    const currentStaged = attachmentStore.stagedAttachments.map((attachment, attachmentOrder) => ({
      ...attachment,
      attachmentOrder,
    }));
    attachmentStore.clearStaged();
    if (currentStaged.length > 0) {
      await attachmentStore.preProcessDocuments(currentStaged);
    }

    const now = Date.now();
    const userName = settingsStore.settings?.userName || "User";
    const userMsg: ChatMessage = {
      id: `msg_${now}_user_${Math.random().toString(36).substring(2, 9)}`,
      role: "user",
      name: userName,
      content,
      timestamp: now,
      attachments: currentStaged.length > 0 ? currentStaged : undefined,
      shell: streamStore.computeShell({ role: "user", name: userName }),
      blocks: [{ type: "markdown" as const, content }],
    };

    if (canCommitConversation(key)) {
      currentChatHistory.value = mergeHistoryWindow(
        currentChatHistory.value,
        [userMsg],
        false,
      );
    }
    topicStore.incrementTopicMsgCount(key.ownerId, key.ownerType, key.topicId);
    await triggerGeneration(userMsg, key);
  };

  /**
   * 删除消息
   */
  const deleteMessage = async (messageKey: MessageActionKey) => {
    const key = messageKey.conversation;
    const messageId = messageKey.messageId;
    if (!isMessageActionCurrent(messageKey)) return;

    const targetIndex = currentChatHistory.value.findIndex(m => m.id === messageId);
    if (targetIndex === -1) return;

    try {
      const msgCount = await invoke<number>("delete_messages", {
        ownerId: key.ownerId,
        ownerType: key.ownerType,
        topicId: key.topicId,
        msgIds: [messageId],
      });
      if (canCommitConversation(key)) {
        const currentIndex = currentChatHistory.value.findIndex(message => message.id === messageId);
        if (currentIndex !== -1) currentChatHistory.value.splice(currentIndex, 1);
      }
      topicStore.setTopicMsgCount(key.ownerId, key.ownerType, key.topicId, msgCount);
    } catch (e) {
      notificationStore.addNotification({
        type: "error",
        title: "删除消息失败",
        message: errorMessage(e),
        toastOnly: true,
        duration: 6000,
      });
    }
  };

  const deleteAttachment = async (
    messageKey: MessageActionKey,
    attachmentOrder: number,
    hash: string,
  ) => {
    const key = messageKey.conversation;
    const messageId = messageKey.messageId;
    if (!isMessageActionCurrent(messageKey)) return;
    // 1. 调用后端逻辑删除命令
    await invoke("delete_message_attachment", {
      ownerId: key.ownerId,
      ownerType: key.ownerType,
      topicId: key.topicId,
      messageId,
      attachmentOrder,
      hash,
    });

    // 2. 更新本地状态，以便在界面上实时隐藏该附件
    if (!canCommitConversation(key)) return;
    const targetIndex = currentChatHistory.value.findIndex(m => m.id === messageId);
    if (targetIndex !== -1) {
      const msg = currentChatHistory.value[targetIndex];
      if (msg.attachments) {
        msg.attachments = msg.attachments.filter((att, index) =>
          (att.attachmentOrder ?? index) !== attachmentOrder || att.hash !== hash
        );
      }
    }
  };

  const updateMessageContent = async (messageKey: MessageActionKey, newContent: string) => {
    const key = messageKey.conversation;
    const messageId = messageKey.messageId;
    if (!isMessageActionCurrent(messageKey)) return;
    const targetIndex = currentChatHistory.value.findIndex(m => m.id === messageId);
    if (targetIndex === -1) return;

    const msg = { ...currentChatHistory.value[targetIndex] };

    try {
      const compiledBlocks = await invoke("patch_single_message", {
        ownerId: key.ownerId,
        ownerType: key.ownerType,
        topicId: key.topicId,
        message: {
          ...msg,
          content: newContent,
          blocks: undefined,
        },
      });
      if (canCommitConversation(key)) {
        clearMessageCache(messageId);
        const currentIndex = currentChatHistory.value.findIndex(message => message.id === messageId);
        if (currentIndex !== -1) {
          currentChatHistory.value[currentIndex] = {
            ...currentChatHistory.value[currentIndex],
            content: newContent,
            blocks: compiledBlocks as any,
          };
        }
      }
    } catch (e) {
      console.error("[updateMessageContent] patch_single_message failed:", e);
      notificationStore.addNotification({
        type: "error",
        title: "保存消息失败",
        message: errorMessage(e),
        toastOnly: true,
        duration: 6000,
      });
      throw e;
    }
  };

  const regenerateResponse = async (messageKey: MessageActionKey) => {
    const key = messageKey.conversation;
    const targetMessageId = messageKey.messageId;
    if (!isMessageActionCurrent(messageKey)) return;
    const targetIndex = currentChatHistory.value.findIndex(m => m.id === targetMessageId);
    if (targetIndex === -1) return;

    // 当前窗口仅负责乐观更新；权威用户锚点由后端从完整历史中寻找。
    let lastUserMsgIndex = targetIndex - 1;
    while (lastUserMsgIndex >= 0 && currentChatHistory.value[lastUserMsgIndex].role !== "user") {
      lastUserMsgIndex--;
    }
    const didOptimisticTruncate = lastUserMsgIndex >= 0;
    if (didOptimisticTruncate) {
      const countToDelete = currentChatHistory.value.length - (lastUserMsgIndex + 1);
      currentChatHistory.value = currentChatHistory.value.slice(0, lastUserMsgIndex + 1);
      topicStore.decrementTopicMsgCount(key.ownerId, key.ownerType, key.topicId, countToDelete);
    }

    try {
      const streamChannel = new Channel<any>();
      streamChannel.onmessage = (event) => streamStore.processStreamEvent(event, {
        onMessageCreated: (msg, tid) => {
          if (tid === key.topicId && canCommitConversation(key) && !currentChatHistory.value.some(m => m.id === msg.id)) {
            currentChatHistory.value = mergeHistoryWindow(
              currentChatHistory.value,
              [msg],
              false,
            );
          }
        },
        onStreamFinished: (_mid, tid) => {
          if (tid === key.topicId && canCommitConversation(key)) {
            summarizeTopic();
          }
        }
      });

      const result = await invoke<{ msgCount: number }>("regenerate_topic_response", {
        ownerId: key.ownerId,
        ownerType: key.ownerType,
        topicId: key.topicId,
        targetResponseMsgId: targetMessageId,
        streamChannel
      });
      if (!didOptimisticTruncate && canCommitConversation(key)) {
        await loadHistory(key.ownerId, key.ownerType, key.topicId, 15);
      }
      topicStore.setTopicMsgCount(key.ownerId, key.ownerType, key.topicId, result.msgCount);
    } catch (e) {
      console.error("[ChatHistoryStore] Regeneration failed:", e);
      // 与 triggerGeneration 一致：重新生成失败同样需要用户可见反馈
      notificationStore.addNotification({
        type: "error",
        title: "重新生成失败",
        message: e instanceof Error ? e.message : String(e),
        toastOnly: true,
        duration: 6000,
      });
      if (canCommitConversation(key)) {
        await loadHistory(key.ownerId, key.ownerType, key.topicId, 15);
        if (canCommitConversation(key)) {
          await topicStore.loadTopicList(key.ownerId, key.ownerType).catch((reloadError) => {
            console.error("[ChatHistoryStore] Failed to reconcile topic count:", reloadError);
          });
        }
      }
    }
  };


  const fetchRawContent = async (messageKey: MessageActionKey): Promise<string> => {
    const key = messageKey.conversation;
    const messageId = messageKey.messageId;
    if (!isMessageActionCurrent(messageKey)) return "";
    const existingMsg = currentChatHistory.value.find(m => m.id === messageId);
    if (existingMsg && existingMsg.content) return existingMsg.content;
    try {
      const content = await invoke<string>('fetch_raw_message_content', {
        ownerId: key.ownerId,
        ownerType: key.ownerType,
        topicId: key.topicId,
        messageId,
      });
      if (canCommitConversation(key)) {
        const current = currentChatHistory.value.find(message => message.id === messageId);
        if (current) current.content = content;
      }
      return content;
    } catch (e) {
      console.error(`[ChatHistoryStore] Failed to fetch raw content for message ${messageId}:`, e);
      return "";
    }
  };

  const beginEditResend = async (messageKey: MessageActionKey) => {
    const initialContent = await fetchRawContent(messageKey);
    if (!isMessageActionCurrent(messageKey)) return;
    editingMessage.value = {
      key: messageKey,
      initialContent,
    };
  };

  const reRenderMessage = async (messageKey: MessageActionKey) => {
    const key = messageKey.conversation;
    const messageId = messageKey.messageId;
    if (!isMessageActionCurrent(messageKey)) throw new Error("消息不属于当前会话");
    const targetIndex = currentChatHistory.value.findIndex(m => m.id === messageId);
    if (targetIndex === -1) {
      throw new Error("消息未在当前历史记录中找到");
    }

    clearMessageCache(messageId);

    try {
      const compiledBlocks = await invoke<ContentBlock[]>("re_render_message", {
        ownerId: key.ownerId,
        ownerType: key.ownerType,
        messageId,
        topicId: key.topicId,
      });
      if (!canCommitConversation(key)) return;
      clearMessageCache(messageId);
      const currentIndex = currentChatHistory.value.findIndex(message => message.id === messageId);
      if (currentIndex !== -1) {
        currentChatHistory.value[currentIndex] = {
          ...currentChatHistory.value[currentIndex],
          blocks: compiledBlocks,
        };
      }
    } catch (e) {
      console.error("[reRenderMessage] re_render_message failed:", e);
      throw e;
    }
  };

  return {
    currentChatHistory,
    loading,
    hasMoreHistory,
    isLoadingHistory,
    hasEvictedNewer,
    loadedConversationKey,
    editingMessage,
    preloadedHistory,
    preloadHistory,
    loadHistory,
    loadHistoryPaginated,
    loadMoreHistory,
    loadHistoryAround,
    returnToLatest,
    resetHistoryForConversation,
    sendMessage,
    deleteMessage,
    deleteAttachment,
    triggerGeneration,
    inviteGroupMember,
    summarizeTopic,
    captureMessageActionKey,
    isMessageActionCurrent,
    updateMessageContent,
    beginEditResend,
    regenerateResponse,
    fetchRawContent,
    reRenderMessage,
  };
});

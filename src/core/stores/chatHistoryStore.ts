import { defineStore } from "pinia";
import { ref } from "vue";
import { invoke, Channel } from "@tauri-apps/api/core";
import { useChatSessionStore } from "./chatSessionStore";
import { useChatStreamStore } from "./chatStreamStore";
import { useAttachmentStore } from "./attachmentStore";
import { useAssistantStore } from "./assistant";
import { useSettingsStore } from "./settings";
import { useTopicStore } from "./topicListManager";
import { clearMessageCache } from "../utils/astRenderer";

import type { ChatMessage, ContentBlock } from "../types/chat";
import type { ConversationKey } from "./chatSessionStore";

export interface HistoryPageResult {
  addedCount: number;
  error?: unknown;
  aborted?: boolean;
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
  const historyOffset = ref(0);        // 当前已加载的消息总数（= 下次请求的 offset 起点）
  const hasMoreHistory = ref(true);    // 是否还有更多旧消息
  const isLoadingHistory = ref(false); // 防止并发重复触发
  const hasEvictedNewer = ref(false);  // 深翻历史触发窗口裁剪后，最新端需要显式重载
  const loadedConversationKey = ref<ConversationKey | null>(null);

  // 启动预加载缓存：PRELOADING 阶段提前拉取首屏历史，ChatView mount 后直接消费
  const preloadedHistory = ref<{ key: ConversationKey; messages: ChatMessage[] } | null>(null);
  let preloadConsumed = false;

  // 用于拦截重新生成时的输入框补全
  const editMessageContent = ref("");
  // 用于标记当前是否正在“编辑重发”某条历史消息
  const editingOriginalMessageId = ref<string | null>(null);

  // 用于防止并发加载与话题切换导致竞态的消息拉取中止控制器 (AbortController)
  let currentLoadAbortController: AbortController | null = null;
  let currentLoadId = 0;

  const sessionStore = useChatSessionStore();
  const streamStore = useChatStreamStore();
  const attachmentStore = useAttachmentStore();
  const assistantStore = useAssistantStore();
  const settingsStore = useSettingsStore();
  const topicStore = useTopicStore();

  const captureLoadedConversation = (): ConversationKey | null => {
    const key = sessionStore.currentConversationKey;
    return sameConversation(loadedConversationKey.value, key) && key ? { ...key } : null;
  };

  const canCommitConversation = (key: ConversationKey) =>
    sessionStore.isConversationCurrent(key) && sameConversation(loadedConversationKey.value, key);

  /**
   * 启动预加载：在 PRELOADING 阶段提前拉取首屏聊天历史
   * 让 DB + IPC 开销与 Vue 组件挂载并行，ChatView mount 后直接命中缓存
   */
  const preloadHistory = async (
    ownerId: string,
    ownerType: string,
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

    const topic = topicStore.topics.find((t) => t.id === topicId);
    const isDefaultName = topic && /^(新话题|新会话) \d{2}:\d{2}:\d{2}$/.test(topic.name);
    const messageCount = currentChatHistory.value.filter(
      (m) => m.role !== "system",
    ).length;

    if (isDefaultName && messageCount >= 4) {
      console.log(`[ChatHistoryStore] Triggering AI summary for topic: ${topicId}`);
      try {
        const agentName =
          assistantStore.agents.find((a: any) => a.id === ownerId)?.name ||
          "AI";
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
    ownerType: string,
    topicId: string,
    limit: number = 15,
    offset: number = 0
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
    const oldest = offset > 0 ? currentChatHistory.value[0] : undefined;
    if (offset > 0 && !oldest) return { addedCount: 0, aborted: true };
    console.log(
      `[ChatHistoryStore] Loading history for ${ownerId}, topic: ${topicId}, limit: ${limit}, offset: ${offset}`,
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
        offset === 0 &&
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
          // `offset` remains local bookkeeping only; the backend page boundary is stable.
          offset: offset === 0 ? 0 : null,
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

      const hydrated = messages.map(msg => streamStore.activeStreamMessages.get(msg.id) || msg);
      let addedCount = hydrated.length;
      if (offset === 0) {
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
      historyOffset.value = currentChatHistory.value.length;
      hasMoreHistory.value = messages.length >= limit;
      hydrated.forEach(msg => attachmentStore.resolveMessageAssets(msg));
      return { addedCount };
    } catch (e) {
      console.error("[ChatHistoryStore] Failed to load history:", e);
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

        if (offset === 0 && !signal.aborted) {
          streamStore.checkAndRecoverInterruptedStreams().catch((err) => {
            console.error("[ChatHistoryStore] Failed to trigger stream recovery after loading history:", err);
          });
        }
      }
    }
  };

  const resetHistoryForConversation = () => {
    currentLoadId += 1;
    currentLoadAbortController?.abort();
    currentLoadAbortController = null;
    currentChatHistory.value = [];
    loadedConversationKey.value = null;
    historyOffset.value = 0;
    hasMoreHistory.value = true;
    hasEvictedNewer.value = false;
    loading.value = false;
    isLoadingHistory.value = false;
  };

  const loadHistoryPaginated = async (
    ownerId: string,
    ownerType: string,
    topicId: string,
  ) => {
    resetHistoryForConversation();
    return loadHistory(ownerId, ownerType, topicId, 5, 0);
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
      historyOffset.value,
    );
  };

  const returnToLatest = async (): Promise<HistoryPageResult> => {
    if (!hasEvictedNewer.value) return { addedCount: 0 };
    const key = sessionStore.currentConversationKey;
    if (!key || !sameConversation(loadedConversationKey.value, key)) {
      return { addedCount: 0, aborted: true };
    }
    return loadHistory(key.ownerId, key.ownerType, key.topicId, 15, 0);
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

      if (key.ownerType === "group") {
        await invoke("handle_group_chat_message", { 
          payload: {
            groupId: key.ownerId,
            topicId: key.topicId,
            userMessage: userMsg,
            vcpUrl: settings.vcpServerUrl || "",
            vcpApiKey: settings.vcpApiKey || "",
          }, 
          streamChannel 
        });
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
    }
  };

  /**
   * 发送消息
   */
  const sendMessage = async (content: string) => {
    if (hasEvictedNewer.value && !editingOriginalMessageId.value) {
      const latest = await returnToLatest();
      if (latest.error || latest.aborted || hasEvictedNewer.value) return;
    }
    const key = captureLoadedConversation();
    if (!key || (!content.trim() && attachmentStore.stagedAttachments.length === 0)) return;
    if (attachmentStore.stagedAttachments.some(attachment => attachment.status !== "done")) return;

    if (typeof navigator !== "undefined" && navigator.vibrate) {
      navigator.vibrate(25);
    }

    if (editingOriginalMessageId.value) {
      const originalId = editingOriginalMessageId.value;
      editingOriginalMessageId.value = null;
      const targetIndex = currentChatHistory.value.findIndex(m => m.id === originalId);
      if (targetIndex !== -1) {
        const targetMsg = {
          ...currentChatHistory.value[targetIndex],
          content,
          blocks: [{ type: "markdown" as const, content }],
        };
        await invoke("truncate_history_after_timestamp", {
          ownerId: key.ownerId,
          ownerType: key.ownerType,
          topicId: key.topicId,
          timestamp: targetMsg.timestamp,
        });
        if (canCommitConversation(key)) {
          const currentIndex = currentChatHistory.value.findIndex(message => message.id === originalId);
          if (currentIndex !== -1) {
            currentChatHistory.value[currentIndex] = targetMsg;
            currentChatHistory.value = currentChatHistory.value.slice(0, currentIndex + 1);
          }
        }
        await triggerGeneration(targetMsg, key);
        return;
      }
    }

    const currentStaged = [...attachmentStore.stagedAttachments];
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
    topicStore.incrementTopicMsgCount(key.topicId);
    await triggerGeneration(userMsg, key);
  };

  /**
   * 删除消息
   */
  const deleteMessage = async (messageId: string, deleteAfter: boolean = false) => {
    const key = captureLoadedConversation();
    if (!key) return;

    const targetIndex = currentChatHistory.value.findIndex(m => m.id === messageId);
    if (targetIndex === -1) return;

    const targetMsg = { ...currentChatHistory.value[targetIndex] };
    if (deleteAfter) {
      const countToDelete = currentChatHistory.value.length - targetIndex;
      await invoke("truncate_history_after_timestamp", {
        ownerId: key.ownerId,
        ownerType: key.ownerType,
        topicId: key.topicId,
        timestamp: targetMsg.timestamp - 1,
      });
      if (canCommitConversation(key)) {
        const currentIndex = currentChatHistory.value.findIndex(message => message.id === messageId);
        if (currentIndex !== -1) currentChatHistory.value.splice(currentIndex);
      }
      topicStore.decrementTopicMsgCount(key.topicId, countToDelete);
    } else {
      await invoke("delete_messages", {
        ownerId: key.ownerId,
        ownerType: key.ownerType,
        topicId: key.topicId,
        msgIds: [messageId],
      });
      if (canCommitConversation(key)) {
        const currentIndex = currentChatHistory.value.findIndex(message => message.id === messageId);
        if (currentIndex !== -1) currentChatHistory.value.splice(currentIndex, 1);
      }
      topicStore.decrementTopicMsgCount(key.topicId, 1);
    }
  };

  const deleteAttachment = async (topicId: string, messageId: string, hash: string) => {
    const key = captureLoadedConversation();
    if (!key || key.topicId !== topicId) return;
    // 1. 调用后端逻辑删除命令
    await invoke("delete_message_attachment", {
      topicId,
      messageId,
      hash,
    });

    // 2. 更新本地状态，以便在界面上实时隐藏该附件
    if (!canCommitConversation(key)) return;
    const targetIndex = currentChatHistory.value.findIndex(m => m.id === messageId);
    if (targetIndex !== -1) {
      const msg = currentChatHistory.value[targetIndex];
      if (msg.attachments) {
        msg.attachments = msg.attachments.filter(att => att.hash !== hash);
      }
    }
  };

  const updateMessageContent = async (messageId: string, newContent: string) => {
    const key = captureLoadedConversation();
    if (!key) return;
    clearMessageCache(messageId);
    const targetIndex = currentChatHistory.value.findIndex(m => m.id === messageId);
    if (targetIndex === -1) return;

    const msg = { ...currentChatHistory.value[targetIndex] };
    currentChatHistory.value[targetIndex] = {
      ...msg,
      content: newContent,
      blocks: [{ type: "markdown" as const, content: newContent }],
    };

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
      if (canCommitConversation(key)) {
        const currentIndex = currentChatHistory.value.findIndex(message => message.id === messageId);
        if (currentIndex !== -1) {
          currentChatHistory.value[currentIndex] = {
            ...currentChatHistory.value[currentIndex],
            blocks: [{ type: "markdown" as const, content: newContent }],
          };
        }
      }
    }
  };

  const regenerateResponse = async (targetMessageId: string) => {
    const key = captureLoadedConversation();
    if (!key) return;
    const targetIndex = currentChatHistory.value.findIndex(m => m.id === targetMessageId);
    if (targetIndex === -1) return;

    // 1. 寻找该 AI 消息之前的最后一条用户消息
    let lastUserMsgIndex = targetIndex - 1;
    while (lastUserMsgIndex >= 0 && currentChatHistory.value[lastUserMsgIndex].role !== "user") {
      lastUserMsgIndex--;
    }
    
    if (lastUserMsgIndex === -1) {
      console.warn("[ChatHistoryStore] No user message found to regenerate from.");
      return;
    }

    const lastUserMsg = { ...currentChatHistory.value[lastUserMsgIndex] };

    // 2. 乐观更新 UI：截断历史
    const countToDelete = currentChatHistory.value.length - (lastUserMsgIndex + 1);
    currentChatHistory.value = currentChatHistory.value.slice(0, lastUserMsgIndex + 1);
    topicStore.decrementTopicMsgCount(key.topicId, countToDelete);



    // 3. 调用后端重构后的重生接口
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

      await invoke("regenerate_topic_response", {
        ownerId: key.ownerId,
        ownerType: key.ownerType,
        topicId: key.topicId,
        targetUserMsgId: lastUserMsg.id,
        streamChannel
      });
    } catch (e) {
      console.error("[ChatHistoryStore] Regeneration failed:", e);
    }
  };


  const fetchRawContent = async (messageId: string): Promise<string> => {
    const key = captureLoadedConversation();
    if (!key) return "";
    const existingMsg = currentChatHistory.value.find(m => m.id === messageId);
    if (existingMsg && existingMsg.content) return existingMsg.content;
    try {
      const content = await invoke<string>('fetch_raw_message_content', { messageId });
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

  const persistMessageBlocks = async (messageId: string, blocks: ContentBlock[]) => {
    const key = captureLoadedConversation();
    if (!key) return;
    const msg = currentChatHistory.value.find(m => m.id === messageId);
    if (!msg) return;
    msg.blocks = blocks;
    try {
      await invoke("patch_single_message", {
        ownerId: key.ownerId,
        ownerType: key.ownerType,
        topicId: key.topicId,
        message: { ...msg },
      });
    } catch (e) {
      console.error(`[ChatHistoryStore] Failed to persist message blocks for message ${messageId}:`, e);
    }
  };

  const reRenderMessage = async (messageId: string, topicId: string) => {
    const key = captureLoadedConversation();
    if (!key || key.topicId !== topicId) {
      throw new Error("消息不属于当前会话");
    }
    const targetIndex = currentChatHistory.value.findIndex(m => m.id === messageId);
    if (targetIndex === -1) {
      throw new Error("消息未在当前历史记录中找到");
    }

    clearMessageCache(messageId);

    try {
      const compiledBlocks = await invoke<ContentBlock[]>("re_render_message", {
        messageId,
        topicId,
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
    historyOffset,
    hasMoreHistory,
    isLoadingHistory,
    hasEvictedNewer,
    loadedConversationKey,
    editMessageContent,
    editingOriginalMessageId,
    preloadedHistory,
    preloadHistory,
    loadHistory,
    loadHistoryPaginated,
    loadMoreHistory,
    returnToLatest,
    resetHistoryForConversation,
    sendMessage,
    deleteMessage,
    deleteAttachment,
    triggerGeneration,
    summarizeTopic,
    updateMessageContent,
    regenerateResponse,
    fetchRawContent,
    persistMessageBlocks,
    reRenderMessage,
  };
});

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

import type { ChatMessage, HistoryChunk, ContentBlock } from "../types/chat";

export const useChatHistoryStore = defineStore("chatHistory", () => {
  const currentChatHistory = ref<ChatMessage[]>([]);
  const loading = ref(false);

  // 分页加载状态
  const historyOffset = ref(0);        // 当前已加载的消息总数（= 下次请求的 offset 起点）
  const hasMoreHistory = ref(true);    // 是否还有更多旧消息
  const isLoadingHistory = ref(false); // 防止并发重复触发

  // 启动预加载缓存：PRELOADING 阶段提前拉取首屏历史，ChatView mount 后直接消费
  const preloadedHistory = ref<{ topicId: string; messages: ChatMessage[] } | null>(null);
  let preloadConsumed = false;

  // 用于拦截重新生成时的输入框补全
  const editMessageContent = ref("");
  // 用于标记当前是否正在“编辑重发”某条历史消息
  const editingOriginalMessageId = ref<string | null>(null);

  // 用于防止并发加载与话题切换导致竞态的消息拉取中止控制器 (AbortController)
  let currentLoadAbortController: AbortController | null = null;

  const sessionStore = useChatSessionStore();
  const streamStore = useChatStreamStore();
  const attachmentStore = useAttachmentStore();
  const assistantStore = useAssistantStore();
  const settingsStore = useSettingsStore();
  const topicStore = useTopicStore();

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
    try {
      const messages = await invoke<ChatMessage[]>('load_chat_history', {
        ownerId, ownerType, topicId, limit, offset: 0,
      });
      preloadedHistory.value = { topicId, messages };
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
  ) => {
    console.log(
      `[ChatHistoryStore] Loading history for ${ownerId}, topic: ${topicId}, limit: ${limit}, offset: ${offset}`,
    );
    loading.value = true;
    isLoadingHistory.value = true;

    if (currentLoadAbortController) {
      currentLoadAbortController.abort();
    }
    const controller = new AbortController();
    currentLoadAbortController = controller;
    const { signal } = controller;

    try {
      // Fast Path: offset=0 initial load uses batch invoke (skip Channel + RAF)
      if (offset === 0) {
        let messages: ChatMessage[];

        // Check preloaded cache from PRELOADING phase — zero-latency if hit
        if (!preloadConsumed && preloadedHistory.value?.topicId === topicId) {
          messages = preloadedHistory.value.messages;
          preloadedHistory.value = null;
          preloadConsumed = true;
          console.log(`[ChatHistoryStore] Using preloaded cache: ${messages.length} messages`);
        } else {
          // Normal invoke path
          preloadConsumed = true;
          messages = await invoke<ChatMessage[]>('load_chat_history', {
            ownerId, ownerType, topicId, limit, offset,
          });
        }

        if (signal.aborted || sessionStore.currentTopicId !== topicId) {
          console.warn(`[ChatHistoryStore] Topic changed/aborted during batch load, discarding.`);
          return;
        }

        // Object hydration: prefer reactive proxy from active streams
        const hydrated = messages.map(msg =>
          streamStore.activeStreamMessages.get(msg.id) || msg,
        );

        currentChatHistory.value = hydrated;
        historyOffset.value = hydrated.length;
        hasMoreHistory.value = hydrated.length >= limit;

        // Resolve attachment paths (sync, no IPC)
        hydrated.forEach(msg => attachmentStore.resolveMessageAssets(msg));

        console.log(`[ChatHistoryStore] Loaded ${hydrated.length} messages [initial]`);
        return;
      }

      // Channel Path: pagination (offset > 0) streaming logic unchanged
      const channel = new Channel<HistoryChunk>();
      const buffer: ChatMessage[] = [];
      let resolveComplete: (() => void) | null = null;
      const completePromise = new Promise<void>((resolve) => { resolveComplete = resolve; });

      channel.onmessage = (chunk) => {
        // 唯一性与话题一致性防御性校验
        if (signal.aborted || sessionStore.currentTopicId !== topicId) {
          return;
        }

        // 对象劫持 (Object Hydration)：活跃流中的响应式对象优先
        const activeMsg = streamStore.activeStreamMessages.get(chunk.message.id);
        const msgToUse = activeMsg || chunk.message;

        buffer.push(msgToUse);

        if (chunk.is_last) {
          currentChatHistory.value = [...buffer, ...currentChatHistory.value];
          historyOffset.value += buffer.length;
          if (buffer.length < limit) {
            hasMoreHistory.value = false;
          }
          resolveComplete?.();
        }
      };

      const total = await invoke<number>('load_chat_history_streamed', {
        ownerId,
        ownerType,
        topicId,
        limit,
        offset,
        onMessage: channel,
      });

      if (total === 0) {
        hasMoreHistory.value = false;
        (resolveComplete as (() => void) | null)?.();
      }

      await completePromise;

      console.log(
        `[ChatHistoryStore] Loaded ${buffer.length} messages [pagination] for ${ownerId}, topic: ${topicId}`,
      );

      if (signal.aborted || sessionStore.currentTopicId !== topicId) {
        console.warn(`[ChatHistoryStore] Topic changed or request aborted during pagination, discarding.`);
        return;
      }

      buffer.forEach(msg => attachmentStore.resolveMessageAssets(msg));
    } catch (e) {
      console.error("[ChatHistoryStore] Failed to stream history:", e);
    } finally {
      if (currentLoadAbortController === controller) {
        currentLoadAbortController = null;
      }
      loading.value = false;
      isLoadingHistory.value = false;

      // 🆕 串行化安全保障：首次加载历史记录成功后，安全且串行地触发活跃流的检查与接续，杜绝对齐竞态
      if (offset === 0 && !signal.aborted && sessionStore.currentTopicId === topicId) {
        const streamStore = useChatStreamStore();
        streamStore.checkAndRecoverInterruptedStreams().catch((err) => {
          console.error("[ChatHistoryStore] Failed to trigger stream recovery after loading history:", err);
        });
      }
    }
  };

  const loadHistoryPaginated = async (
    ownerId: string,
    ownerType: string,
    topicId: string,
  ) => {
    // 切换话题时强制重置分页状态，避免旧话题状态污染
    historyOffset.value = 0;
    hasMoreHistory.value = true;
    await loadHistory(ownerId, ownerType, topicId, 5, 0);
  };

  const loadMoreHistory = async () => {
    if (!hasMoreHistory.value || isLoadingHistory.value) return;
    if (!sessionStore.currentSelectedItem?.id || !sessionStore.currentTopicId) return;
    await loadHistory(
      sessionStore.currentSelectedItem.id,
      sessionStore.currentSelectedItem.type,
      sessionStore.currentTopicId,
      10,
      historyOffset.value,
    );
  };

  /**
   * 触发 AI 生成逻辑
   */
  const triggerGeneration = async (userMsg: ChatMessage) => {
    if (!sessionStore.currentSelectedItem || !sessionStore.currentTopicId) return;

    const agentId = sessionStore.currentSelectedItem.id;
    const topicId = sessionStore.currentTopicId;


    try {
      const compiledBlocks = await invoke<ContentBlock[]>("append_single_message", {
        ownerId: sessionStore.currentSelectedItem.id,
        ownerType: sessionStore.currentSelectedItem.type,
        topicId,
        message: {
          ...userMsg,
          blocks: undefined, // 强行设为 undefined，迫使后端执行真正的编译，生成 markdown AST 节点与表情包匹配
        },
      });

      const targetIndex = currentChatHistory.value.findIndex(m => m.id === userMsg.id);
      if (targetIndex !== -1) {
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
          if (tid === sessionStore.currentTopicId && !currentChatHistory.value.some(m => m.id === msg.id)) {
            currentChatHistory.value.push(msg);
            currentChatHistory.value.sort((a, b) => a.timestamp - b.timestamp);
          }
        },
        onStreamFinished: (_mid, tid) => {
          if (tid === sessionStore.currentTopicId) {
            summarizeTopic();
          }
        }
      });

      if (sessionStore.currentSelectedItem.type === "group") {
        await invoke("handle_group_chat_message", { 
          payload: {
            groupId: sessionStore.currentSelectedItem.id,
            topicId,
            userMessage: userMsg,
            vcpUrl: settings.vcpServerUrl || "",
            vcpApiKey: settings.vcpApiKey || "",
          }, 
          streamChannel 
        });
      } else {
        await invoke("handle_agent_chat_message", { 
          payload: {
            agentId,
            topicId,
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
    if (!sessionStore.currentSelectedItem || !sessionStore.currentTopicId || (!content.trim() && attachmentStore.stagedAttachments.length === 0)) return;

    if (typeof navigator !== "undefined" && navigator.vibrate) {
      navigator.vibrate(25);
    }

    if (editingOriginalMessageId.value) {
      const originalId = editingOriginalMessageId.value;
      editingOriginalMessageId.value = null;
      const targetIndex = currentChatHistory.value.findIndex(m => m.id === originalId);
      if (targetIndex !== -1) {
        const targetMsg = currentChatHistory.value[targetIndex];
        targetMsg.content = content;
        targetMsg.blocks = [{ type: "markdown" as const, content }];
        await invoke("truncate_history_after_timestamp", {
          ownerId: sessionStore.currentSelectedItem.id,
          ownerType: sessionStore.currentSelectedItem.type,
          topicId: sessionStore.currentTopicId,
          timestamp: targetMsg.timestamp,
        });
        currentChatHistory.value = currentChatHistory.value.slice(0, targetIndex + 1);
        await triggerGeneration(targetMsg);
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

    currentChatHistory.value.push(userMsg);
    if (sessionStore.currentTopicId) {
      topicStore.incrementTopicMsgCount(sessionStore.currentTopicId);
    }
    await triggerGeneration(userMsg);
  };

  /**
   * 删除消息
   */
  const deleteMessage = async (messageId: string, deleteAfter: boolean = false) => {
    if (!sessionStore.currentSelectedItem || !sessionStore.currentTopicId) return;

    const targetIndex = currentChatHistory.value.findIndex(m => m.id === messageId);
    if (targetIndex === -1) return;

    const targetMsg = currentChatHistory.value[targetIndex];
    if (deleteAfter) {
      const countToDelete = currentChatHistory.value.length - targetIndex;
      await invoke("truncate_history_after_timestamp", {
        ownerId: sessionStore.currentSelectedItem.id,
        ownerType: sessionStore.currentSelectedItem.type,
        topicId: sessionStore.currentTopicId,
        timestamp: targetMsg.timestamp - 1,
      });
      currentChatHistory.value.splice(targetIndex);
      if (sessionStore.currentTopicId) {
        topicStore.decrementTopicMsgCount(sessionStore.currentTopicId, countToDelete);
      }
    } else {
      await invoke("delete_messages", {
        ownerId: sessionStore.currentSelectedItem.id,
        ownerType: sessionStore.currentSelectedItem.type,
        topicId: sessionStore.currentTopicId,
        msgIds: [messageId],
      });
      currentChatHistory.value.splice(targetIndex, 1);
      if (sessionStore.currentTopicId) {
        topicStore.decrementTopicMsgCount(sessionStore.currentTopicId, 1);
      }
    }
  };

  const deleteAttachment = async (topicId: string, messageId: string, hash: string) => {
    // 1. 调用后端逻辑删除命令
    await invoke("delete_message_attachment", {
      topicId,
      messageId,
      hash,
    });

    // 2. 更新本地状态，以便在界面上实时隐藏该附件
    const targetIndex = currentChatHistory.value.findIndex(m => m.id === messageId);
    if (targetIndex !== -1) {
      const msg = currentChatHistory.value[targetIndex];
      if (msg.attachments) {
        msg.attachments = msg.attachments.filter(att => att.hash !== hash);
      }
    }
  };

  const updateMessageContent = async (messageId: string, newContent: string) => {
    clearMessageCache(messageId);
    const targetIndex = currentChatHistory.value.findIndex(m => m.id === messageId);
    if (targetIndex === -1) return;

    const msg = currentChatHistory.value[targetIndex];
    currentChatHistory.value[targetIndex] = {
      ...msg,
      content: newContent,
      blocks: [{ type: "markdown" as const, content: newContent }],
    };

    if (sessionStore.currentSelectedItem?.id && sessionStore.currentTopicId) {
      try {
        const compiledBlocks = await invoke("patch_single_message", {
          ownerId: sessionStore.currentSelectedItem.id,
          ownerType: sessionStore.currentSelectedItem.type,
          topicId: sessionStore.currentTopicId,
          message: {
            ...currentChatHistory.value[targetIndex],
            blocks: undefined,
          },
        });
        clearMessageCache(messageId);
        currentChatHistory.value[targetIndex] = {
          ...currentChatHistory.value[targetIndex],
          blocks: compiledBlocks as any,
        };
      } catch (e) {
        console.error("[updateMessageContent] patch_single_message failed:", e);
        currentChatHistory.value[targetIndex] = {
          ...currentChatHistory.value[targetIndex],
          blocks: [{ type: "markdown" as const, content: newContent }],
        };
      }
    }
  };

  const regenerateResponse = async (targetMessageId: string) => {
    const targetIndex = currentChatHistory.value.findIndex(m => m.id === targetMessageId);
    if (targetIndex === -1) return;

    if (!sessionStore.currentSelectedItem?.id || !sessionStore.currentTopicId) return;

    const topicId = sessionStore.currentTopicId;
    const ownerId = sessionStore.currentSelectedItem.id;
    const ownerType = sessionStore.currentSelectedItem.type;

    // 1. 寻找该 AI 消息之前的最后一条用户消息
    let lastUserMsgIndex = targetIndex - 1;
    while (lastUserMsgIndex >= 0 && currentChatHistory.value[lastUserMsgIndex].role !== "user") {
      lastUserMsgIndex--;
    }
    
    if (lastUserMsgIndex === -1) {
      console.warn("[ChatHistoryStore] No user message found to regenerate from.");
      return;
    }

    const lastUserMsg = currentChatHistory.value[lastUserMsgIndex];

    // 2. 乐观更新 UI：截断历史
    const countToDelete = currentChatHistory.value.length - (lastUserMsgIndex + 1);
    currentChatHistory.value = currentChatHistory.value.slice(0, lastUserMsgIndex + 1);
    topicStore.decrementTopicMsgCount(topicId, countToDelete);



    // 3. 调用后端重构后的重生接口
    try {
      const streamChannel = new Channel<any>();
      streamChannel.onmessage = (event) => streamStore.processStreamEvent(event, {
        onMessageCreated: (msg, tid) => {
          if (tid === sessionStore.currentTopicId && !currentChatHistory.value.some(m => m.id === msg.id)) {
            currentChatHistory.value.push(msg);
            currentChatHistory.value.sort((a, b) => a.timestamp - b.timestamp);
          }
        },
        onStreamFinished: (_mid, tid) => {
          if (tid === sessionStore.currentTopicId) {
            summarizeTopic();
          }
        }
      });

      await invoke("regenerate_topic_response", {
        ownerId,
        ownerType,
        topicId,
        targetUserMsgId: lastUserMsg.id,
        streamChannel
      });
    } catch (e) {
      console.error("[ChatHistoryStore] Regeneration failed:", e);
    }
  };


  const fetchRawContent = async (messageId: string): Promise<string> => {
    const existingMsg = currentChatHistory.value.find(m => m.id === messageId);
    if (existingMsg && existingMsg.content) return existingMsg.content;
    try {
      const content = await invoke<string>('fetch_raw_message_content', { messageId });
      if (existingMsg) existingMsg.content = content;
      return content;
    } catch (e) {
      console.error(`[ChatHistoryStore] Failed to fetch raw content for message ${messageId}:`, e);
      return "";
    }
  };

  const persistMessageBlocks = async (messageId: string, blocks: ContentBlock[]) => {
    const msg = currentChatHistory.value.find(m => m.id === messageId);
    if (!msg || !sessionStore.currentSelectedItem?.id || !sessionStore.currentTopicId) return;
    msg.blocks = blocks;
    try {
      await invoke("patch_single_message", {
        ownerId: sessionStore.currentSelectedItem.id,
        ownerType: sessionStore.currentSelectedItem.type,
        topicId: sessionStore.currentTopicId,
        message: msg,
      });
    } catch (e) {
      console.error(`[ChatHistoryStore] Failed to persist message blocks for message ${messageId}:`, e);
    }
  };

  const reRenderMessage = async (messageId: string, topicId: string) => {
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
      clearMessageCache(messageId);
      currentChatHistory.value[targetIndex] = {
        ...currentChatHistory.value[targetIndex],
        blocks: compiledBlocks,
      };
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
    editMessageContent,
    editingOriginalMessageId,
    preloadedHistory,
    preloadHistory,
    loadHistory,
    loadHistoryPaginated,
    loadMoreHistory,
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

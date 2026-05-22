import { defineStore } from "pinia";
import { ref, reactive } from "vue";
import { invoke, Channel } from "@tauri-apps/api/core";
import { useChatSessionStore } from "./chatSessionStore";
import { useChatStreamStore } from "./chatStreamStore";
import { useAttachmentStore } from "./attachmentStore";
import { useAssistantStore } from "./assistant";
import { useSettingsStore } from "./settings";
import { useTopicStore } from "./topicListManager";
import { clearMessageCache } from "../utils/astRenderer";
import { acquireScreenKeep } from "../composables/useScreenKeeper";
import type { ChatMessage, HistoryChunk, ContentBlock } from "../types/chat";

export const useChatHistoryStore = defineStore("chatHistory", () => {
  const currentChatHistory = ref<ChatMessage[]>([]);
  const loading = ref(false);

  // 分页加载状态
  const historyOffset = ref(0);        // 当前已加载的消息总数（= 下次请求的 offset 起点）
  const hasMoreHistory = ref(true);    // 是否还有更多旧消息
  const isLoadingHistory = ref(false); // 防止并发重复触发

  // 用于拦截重新生成时的输入框补全
  const editMessageContent = ref("");
  // 用于标记当前是否正在“编辑重发”某条历史消息
  const editingOriginalMessageId = ref<string | null>(null);

  const sessionStore = useChatSessionStore();
  const streamStore = useChatStreamStore();
  const attachmentStore = useAttachmentStore();
  const assistantStore = useAssistantStore();
  const settingsStore = useSettingsStore();
  const topicStore = useTopicStore();

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
    const isDefaultName = topic && /^新话题 \d{2}:\d{2}:\d{2}$/.test(topic.name);
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
    const loadType = offset === 0 ? "initial" : "pagination";
    console.log(
      `[ChatHistoryStore] Loading history [${loadType}] for ${ownerId}, topic: ${topicId}, limit: ${limit}, offset: ${offset}`,
    );
    loading.value = true;
    isLoadingHistory.value = true;
    try {
      const requestedTopicId = sessionStore.currentTopicId;
      const channel = new Channel<HistoryChunk>();
      const buffer: ChatMessage[] = [];
      let receivedCount = 0;
      let resolveComplete: (() => void) | null = null;
      const completePromise = new Promise<void>((resolve) => { resolveComplete = resolve; });

      channel.onmessage = (chunk) => {
        // 1. 会话一致性校验：如果用户在加载中途切换了话题，丢弃后续消息
        if (sessionStore.currentTopicId !== requestedTopicId && requestedTopicId !== null) {
          return;
        }

        // 2. [关键修复] 消息对象劫持 (Object Hydration)
        // 如果该消息正在活跃生成中，则从全局流池中取出“活的”响应式对象
        // 这确保了即使是刚从 DB 拉回来的骨架，也能瞬间恢复流式动画与渲染状态
        const activeMsg = streamStore.activeStreamMessages.get(chunk.message.id);
        const msgToUse = activeMsg || chunk.message;

        if (offset === 0) {
          if (chunk.index === 0) {
            currentChatHistory.value = [];
            hasMoreHistory.value = true;
          }
          currentChatHistory.value.push(msgToUse);
          receivedCount++;
        } else {
          buffer.push(msgToUse);
          receivedCount++;
        }

        if (chunk.is_last) {
          if (offset > 0) {
            currentChatHistory.value = [...buffer, ...currentChatHistory.value];
            historyOffset.value += buffer.length;
            if (buffer.length < limit) {
              hasMoreHistory.value = false;
            }
          } else {
            historyOffset.value = receivedCount;
            if (receivedCount < limit) {
              hasMoreHistory.value = false;
            }
          }
          resolveComplete?.();
        }
      };

      await invoke('load_chat_history_streamed', {
        ownerId,
        ownerType,
        topicId,
        limit,
        offset,
        onMessage: channel,
      });

      if (receivedCount === 0) {
        if (offset === 0) {
          currentChatHistory.value = [];
          historyOffset.value = 0;
        }
        hasMoreHistory.value = false;
        (resolveComplete as (() => void) | null)?.();
      }

      await completePromise;

      const loadedCount = offset === 0 ? receivedCount : buffer.length;
      console.log(
        `[ChatHistoryStore] Loaded ${loadedCount} messages [${loadType}] for ${ownerId}, topic: ${topicId}`,
      );

      if (sessionStore.currentTopicId !== requestedTopicId && requestedTopicId !== null) {
        console.warn(`[ChatHistoryStore] Topic changed during load, discarding results.`);
        return;
      }

      const messagesToResolve = offset === 0 ? currentChatHistory.value : buffer;
      await Promise.all(
        messagesToResolve.map(async (msg) => {
          attachmentStore.resolveMessageAssets(msg);
        }),
      );
    } catch (e) {
      console.error("[ChatHistoryStore] Failed to stream history:", e);
    } finally {
      loading.value = false;
      isLoadingHistory.value = false;
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
    const now = Date.now();
    const thinkingId = `msg_${now}_assistant_${Math.random().toString(36).substring(2, 9)}`;
    const assistantName = sessionStore.currentSelectedItem.type === "agent"
      ? (assistantStore.agents.find((a) => a.id === agentId)?.name || "Assistant")
      : undefined;

    const thinkingMsg = reactive<ChatMessage>({
      id: thinkingId,
      role: "assistant",
      name: assistantName,
      content: "",
      timestamp: now + 1,
      isThinking: true,
      isGroupMessage: sessionStore.currentSelectedItem.type === "group",
      groupId: sessionStore.currentSelectedItem.type === "group" ? sessionStore.currentSelectedItem.id : undefined,
      agentId: sessionStore.currentSelectedItem.type === "agent" ? sessionStore.currentSelectedItem.id : undefined,
      shell: streamStore.computeShell({ role: "assistant", agentId, name: assistantName }),
    });

    // 注册到全局流池和当前视图
    streamStore.activeStreamMessages.set(thinkingId, thinkingMsg);
    currentChatHistory.value.push(thinkingMsg);
    topicStore.incrementTopicMsgCount(topicId);

    streamStore.streamingMessageId = thinkingId;
    streamStore.addSessionStream(agentId, topicId, thinkingId);
    acquireScreenKeep();

    try {
      await invoke("append_single_message", {
        ownerId: sessionStore.currentSelectedItem.id,
        ownerType: sessionStore.currentSelectedItem.type,
        topicId: sessionStore.currentTopicId,
        message: userMsg,
      });
      await invoke("append_single_message", {
        ownerId: sessionStore.currentSelectedItem.id,
        ownerType: sessionStore.currentSelectedItem.type,
        topicId: sessionStore.currentTopicId,
        message: thinkingMsg,
      });

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
            topicId: sessionStore.currentTopicId,
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
            topicId: sessionStore.currentTopicId,
            userMessage: userMsg,
            vcpUrl: settings.vcpServerUrl || "",
            vcpApiKey: settings.vcpApiKey || "",
            thinkingMessageId: thinkingId,
          }, 
          streamChannel 
        });
      }
    } catch (e) {
      console.error("[ChatHistoryStore] Generation failed:", e);
      const errorText = `\n\n> VCP错误: ${e instanceof Error ? e.message : String(e)}`;
      thinkingMsg.isThinking = false;
      thinkingMsg.content += errorText;
      streamStore.removeSessionStream(agentId, topicId, thinkingId);
    }
  };

  /**
   * 发送消息
   */
  const sendMessage = async (content: string) => {
    if (!sessionStore.currentSelectedItem || !sessionStore.currentTopicId || (!content.trim() && attachmentStore.stagedAttachments.length === 0)) return;

    if (editingOriginalMessageId.value) {
      const originalId = editingOriginalMessageId.value;
      editingOriginalMessageId.value = null;
      const targetIndex = currentChatHistory.value.findIndex(m => m.id === originalId);
      if (targetIndex !== -1) {
        const targetMsg = currentChatHistory.value[targetIndex];
        targetMsg.content = content;
        targetMsg.blocks = undefined;
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

  const updateMessageContent = async (messageId: string, newContent: string) => {
    clearMessageCache(messageId);
    const targetIndex = currentChatHistory.value.findIndex(m => m.id === messageId);
    if (targetIndex === -1) return;

    const msg = currentChatHistory.value[targetIndex];
    currentChatHistory.value[targetIndex] = {
      ...msg,
      content: newContent,
      blocks: undefined,
    };

    if (sessionStore.currentSelectedItem?.id && sessionStore.currentTopicId) {
      try {
        const compiledBlocks = await invoke("patch_single_message", {
          ownerId: sessionStore.currentSelectedItem.id,
          ownerType: sessionStore.currentSelectedItem.type,
          topicId: sessionStore.currentTopicId,
          message: currentChatHistory.value[targetIndex],
        });
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

    // 3. 构造思考占位消息 (并注册到全局池)
    const thinkingId = `msg_${Date.now()}_assistant_${Math.random().toString(36).substring(2, 9)}`;
    const regenName = ownerType === "agent"
        ? (assistantStore.agents.find((a) => a.id === ownerId)?.name || "Assistant")
        : undefined;
    const thinkingMsg = reactive<ChatMessage>({
      id: thinkingId,
      role: "assistant",
      name: regenName,
      content: "",
      timestamp: Date.now(),
      isThinking: true,
      isGroupMessage: ownerType === "group",
      groupId: ownerType === "group" ? ownerId : undefined,
      agentId: ownerType === "agent" ? ownerId : undefined,
      shell: streamStore.computeShell({ role: "assistant", agentId: ownerType === "agent" ? ownerId : undefined, name: regenName }),
    });
    
    streamStore.activeStreamMessages.set(thinkingId, thinkingMsg);
    currentChatHistory.value.push(thinkingMsg);
    topicStore.incrementTopicMsgCount(topicId);

    streamStore.streamingMessageId = thinkingId;
    streamStore.addSessionStream(ownerId, topicId, thinkingId);
    acquireScreenKeep();

    // 4. 调用后端重构后的重生接口
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
        thinkingId: thinkingId,
        streamChannel
      });
    } catch (e) {
      console.error("[ChatHistoryStore] Regeneration failed:", e);
      thinkingMsg.isThinking = false;
      thinkingMsg.content += `\n\n> VCP错误: ${e}`;
      streamStore.removeSessionStream(ownerId, topicId, thinkingId);
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
    } catch (e) {}
  };

  /**
   * 为指定消息向后端请求预渲染 blocks（用于无 blocks 的消息回退）
   */
  const compileMessageBlocks = async (messageId: string) => {
    const targetIndex = currentChatHistory.value.findIndex(m => m.id === messageId);
    if (targetIndex === -1) return;
    const msg = currentChatHistory.value[targetIndex];
    if (msg.blocks || !msg.content) return;

    try {
      const blocks = await invoke("process_message_content", { content: msg.content });
      currentChatHistory.value[targetIndex] = {
        ...msg,
        blocks: blocks as any,
      };
    } catch (e) {
      console.error("[ChatHistoryStore] compileMessageBlocks failed:", e);
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
    loadHistory,
    loadHistoryPaginated,
    loadMoreHistory,
    sendMessage,
    deleteMessage,
    triggerGeneration,
    summarizeTopic,
    updateMessageContent,
    regenerateResponse,
    fetchRawContent,
    persistMessageBlocks,
    compileMessageBlocks,
  };
});

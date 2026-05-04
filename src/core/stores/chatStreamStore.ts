import { defineStore } from "pinia";
import { ref, computed, reactive } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { useStreamManagerStore } from "./streamManager";
import { useChatSessionStore } from "./chatSessionStore";
import type { ChatMessage } from "../types/chat";

export const useChatStreamStore = defineStore("chatStream", () => {
  const streamingMessageId = ref<string | null>(null);

  // 核心：记录每个会话（itemId + topicId）是否处于活动流状态
  // 格式: "itemId:topicId" -> [messageId1, messageId2, ...]
  const sessionActiveStreams = ref<Record<string, string[]>>({});

  // 全局活跃流消息池：存储所有正在生成的响应对象 (messageId -> Reactive<ChatMessage>)
  // 无论是在前台还是后台，流式消息都从此池中获取，保证响应式链路不断裂
  const activeStreamMessages = reactive<Map<string, ChatMessage>>(new Map());

  const streamManager = useStreamManagerStore();
  const sessionStore = useChatSessionStore();

  // 兼容旧逻辑的计算属性
  const activeStreamingIds = computed(() => {
    if (!sessionStore.currentSelectedItem?.id || !sessionStore.currentTopicId)
      return new Set<string>();
    const key = `${sessionStore.currentSelectedItem.id}:${sessionStore.currentTopicId}`;
    return new Set(sessionActiveStreams.value[key] || []);
  });

  const isGroupGenerating = computed(() => {
    if (
      !sessionStore.currentSelectedItem?.id ||
      !sessionStore.currentTopicId ||
      sessionStore.currentSelectedItem.type !== "group"
    )
      return false;
    const key = `${sessionStore.currentSelectedItem.id}:${sessionStore.currentTopicId}`;
    const streams = sessionActiveStreams.value[key];
    return streams ? streams.length > 0 : false;
  });

  // 辅助方法：管理会话流状态
  const addSessionStream = (
    ownerId: string,
    topicId: string,
    messageId: string,
  ) => {
    const key = `${ownerId}:${topicId}`;
    if (!sessionActiveStreams.value[key]) {
      sessionActiveStreams.value[key] = [];
    }
    if (!sessionActiveStreams.value[key].includes(messageId)) {
      sessionActiveStreams.value[key].push(messageId);
    }
  };

  const removeSessionStream = (
    ownerId: string,
    topicId: string,
    messageId: string,
  ) => {
    const key = `${ownerId}:${topicId}`;
    const streams = sessionActiveStreams.value[key];
    if (streams) {
      const index = streams.indexOf(messageId);
      if (index !== -1) {
        streams.splice(index, 1);
      }
      if (streams.length === 0) {
        delete sessionActiveStreams.value[key];
      }
    }
    // 同时从全局池中移除 (延迟移除，确保 finalizeStream 能拿到对象)
    setTimeout(() => {
        if (!activeStreamingIds.value.has(messageId)) {
            activeStreamMessages.delete(messageId);
        }
    }, 1000);
  };

  /**
   * 中止指定消息的生成
   */
  const stopMessage = async (messageId: string, onUpdateMessage?: (msg: any) => Promise<void>) => {
    console.log(
      `[ChatStreamStore] Sending interrupt signal for message: ${messageId}`,
    );
    try {
      await invoke("interruptRequest", { messageId: messageId });
      // 本地伪造一个 end 事件，防止假死
      streamManager.finalizeStream(messageId);

      if (streamingMessageId.value === messageId) {
        streamingMessageId.value = null;
      }

      const ownerId = sessionStore.currentSelectedItem?.id;
      const topicId = sessionStore.currentTopicId;

      if (ownerId && topicId) {
        removeSessionStream(ownerId, topicId, messageId);
      }

      // 触发回调进行持久化更新
      if (onUpdateMessage) {
        await onUpdateMessage(messageId);
      }
    } catch (e) {
      console.error(
        `[ChatStreamStore] Failed to interrupt stream for ${messageId}:`,
        e,
      );
    }
  };

  /**
   * 强行中止整个群组的接力赛回合
   */
  const stopGroupTurn = async (topicId: string) => {
    console.log(`[ChatStreamStore] Global Group Interruption for topic: ${topicId}`);
    try {
      // 1. 发射后端熔断信号，打断 for 循环接力
      await invoke("interruptGroupTurn", { topicId: topicId });
      
      // 2. 同时中止当前活跃的所有流消息
      const activeIds = Array.from(activeStreamingIds.value);
      if (activeIds.length > 0) {
        await Promise.all(activeIds.map(id => stopMessage(id)));
      }
    } catch (e) {
      console.error("[ChatStreamStore] Failed to stop group turn:", e);
    }
  };

  return {
    streamingMessageId,
    sessionActiveStreams,
    activeStreamMessages,
    activeStreamingIds,
    isGroupGenerating,
    addSessionStream,
    removeSessionStream,
    stopMessage,
    stopGroupTurn,
  };
});


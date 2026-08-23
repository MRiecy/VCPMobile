import { defineStore } from "pinia";
import { ref, computed } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { useAssistantStore } from "./assistant";
import { useTopicStore } from "./topicListManager";

export interface PickedFileInfo {
  path: string;
  name: string;
  mime: string;
  size: number;
  hash: string;
  thumbnailPath?: string;
}

export type ConversationOwnerType = "agent" | "group";

export interface ConversationKey {
  ownerId: string;
  ownerType: ConversationOwnerType;
  topicId: string;
  epoch: number;
}

export const useChatSessionStore = defineStore("chatSession", () => {
  const currentSelectedItem = ref<any>(null);
  const currentTopicId = ref<string | null>(null);
  const sessionEpoch = ref(0);
  const lastActiveTopicMap = ref<Record<string, string>>({});
  const ownerMapKey = (ownerType: ConversationOwnerType, ownerId: string) =>
    `${ownerType}:${ownerId}`;

  const currentConversationKey = computed<ConversationKey | null>(() => {
    const ownerId = currentSelectedItem.value?.id;
    const ownerType = currentSelectedItem.value?.type;
    const topicId = currentTopicId.value;
    if (!ownerId || !topicId || (ownerType !== "agent" && ownerType !== "group")) {
      return null;
    }
    return { ownerId, ownerType, topicId, epoch: sessionEpoch.value };
  });

  const setConversation = (item: any, topicId: string | null) => {
    const ownerType: ConversationOwnerType | null = item
      ? item.type === "agent" || item.type === "group"
        ? item.type
        : null
      : null;
    const nextItem = item && ownerType ? { ...item, type: ownerType } : null;
    const changed =
      currentSelectedItem.value?.id !== nextItem?.id ||
      currentSelectedItem.value?.type !== nextItem?.type ||
      currentTopicId.value !== topicId;
    if (changed) sessionEpoch.value += 1;
    currentSelectedItem.value = nextItem;
    currentTopicId.value = topicId;
  };

  const clearConversation = () => setConversation(null, null);

  const isConversationCurrent = (key: ConversationKey | null | undefined) => {
    const current = currentConversationKey.value;
    return Boolean(
      key &&
        current &&
        key.ownerId === current.ownerId &&
        key.ownerType === current.ownerType &&
        key.topicId === current.topicId &&
        key.epoch === current.epoch,
    );
  };

  // 动态检索当前活跃的话题对象
  const currentTopic = computed(() => {
    if (!currentTopicId.value) return null;
    const topicStore = useTopicStore();
    return topicStore.topics.find((t) => t.id === currentTopicId.value) || null;
  });

  // 顶部显示标题（话题名，无话题则回退到智能体/群组名字）
  const headerTitle = computed(() => {
    return currentTopic.value?.name || currentSelectedItem.value?.name || "VCP Mobile";
  });

  // Share intent prefill state
  const sharePrefillText = ref("");
  const sharePrefillFiles = ref<PickedFileInfo[]>([]);

  const assistantStore = useAssistantStore();

  /**
   * 从外部分享意图启动会话
   * 1. 选择 Agent → 创建话题 → 切换到聊天 → 预填输入
   */
  const startShareSession = async (
    agentId: string,
    sharedText: string,
    sharedFiles: PickedFileInfo[],
  ) => {
    // 1. 查找并选中 agent
    const agent = assistantStore.agents.find((a) => a.id === agentId);
    if (!agent) {
      throw new Error(`Agent ${agentId} not found`);
    }

    // 2. 创建新话题（复用 TopicCreator 默认命名逻辑）
    const newTopicName = `新话题 ${new Date().toLocaleTimeString([], {
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
    })}`;

    const newTopic = await invoke<any>("create_topic", {
      ownerId: agentId,
      ownerType: "agent",
      name: newTopicName,
    });

    if (!newTopic?.id) {
      throw new Error("Failed to create topic");
    }

    // 3. 选择 topic（设置 currentSelectedItem 和 currentTopicId）
    await selectTopicById(agentId, "agent", newTopic.id);

    // 4. 存储预填数据（由 ChatView/InputEnhancer 消费后清空）
    sharePrefillText.value = sharedText;
    sharePrefillFiles.value = sharedFiles;

    return { topicId: newTopic.id, agentId };
  };

  /**
   * 消费分享预填数据（调用后清空）
   */
  const consumeSharePrefill = () => {
    const text = sharePrefillText.value;
    const files = sharePrefillFiles.value;
    sharePrefillText.value = "";
    sharePrefillFiles.value = [];
    return { text, files };
  };

  /**
   * 选择一个助手或群组，并自动跳转到最近的话题
   * @param loadHistoryCallback 回调函数，用于触发历史加载 (解耦 HistoryStore)
   */
  const selectTopicById = async (
    itemId: string,
    ownerType: ConversationOwnerType,
    topicId: string,
    loadHistoryCallback?: (itemId: string, ownerType: string, topicId: string) => Promise<void>
  ) => {
    lastActiveTopicMap.value[ownerMapKey(ownerType, itemId)] = topicId;

    // 设置当前选中的项目详情 (确保头像和色调同步)
    const agent = ownerType === "agent"
      ? assistantStore.agents.find((a: any) => a.id === itemId)
      : undefined;
    const group = ownerType === "group"
      ? assistantStore.groups.find((g) => g.id === itemId)
      : undefined;
    if (ownerType === "agent" && agent) {
      setConversation({ ...agent, type: "agent" }, topicId);
    } else if (ownerType === "group" && group) {
      setConversation({ ...group, type: "group" }, topicId);
    } else {
      throw new Error(`Conversation owner ${itemId} not found`);
    }

    if (loadHistoryCallback) {
      await loadHistoryCallback(itemId, ownerType, topicId);
    }
  };

  /**
   * 选择一个项目 (Agent/Group)，自动加载其记录的或最新的话题
   */
  const selectItem = async (
    item: any,
    loadHistoryCallback?: (itemId: string, ownerType: string, topicId: string) => Promise<void>
  ) => {
    if (!item) return;
    
    const ownerId = item.id;
    if (item.type !== "agent" && item.type !== "group") {
      throw new Error(`Conversation owner ${ownerId} has no valid type`);
    }
    const ownerType: ConversationOwnerType = item.type;
    
    // 如果已经选中了该项，且当前已有话题，则不重复加载
    if (
      currentSelectedItem.value?.id === ownerId &&
      currentSelectedItem.value?.type === ownerType &&
      currentTopicId.value
    ) {
      return;
    }

    // 1. 优先从 Pinia 持久化的 lastActiveTopicMap 中获取最后一次打开的话题 ID
    let targetTopicId = lastActiveTopicMap.value[ownerMapKey(ownerType, ownerId)];

    // 2. 如果 Pinia 中没有记录，则尝试获取该 Owner 下最新的话题
    if (!targetTopicId) {
      // 选择意图已发生：先关闭旧会话，避免等待 IPC 时旧列表仍可操作。
      setConversation({ ...item, type: ownerType }, null);
      const selectionEpoch = sessionEpoch.value;
      try {
        const topics = await invoke<any[]>("get_topics", {
          ownerId,
          ownerType,
        });
        if (topics && topics.length > 0) {
          // 列表通常按 updated_at 倒序，取第一个
          targetTopicId = topics[0].id || topics[0].topic_id;
        }
      } catch (e) {
        console.error("[ChatSessionStore] Failed to fetch fallback topics:", e);
      }
      if (
        sessionEpoch.value !== selectionEpoch ||
        currentSelectedItem.value?.id !== ownerId ||
        currentSelectedItem.value?.type !== ownerType ||
        currentTopicId.value !== null
      ) {
        console.log(`[ChatSessionStore] Discarding stale owner selection for ${ownerId}.`);
        return;
      }
    }

    if (targetTopicId) {
      await selectTopicById(ownerId, ownerType, targetTopicId, loadHistoryCallback);
    } else {
      // 没有任何话题的极端情况
      console.warn(`[ChatSessionStore] No topics found for ${ownerId}`);
      setConversation({ ...item, type: ownerType }, null);
    }
  };

  return {
    currentSelectedItem,
    currentTopicId,
    sessionEpoch,
    currentConversationKey,
    currentTopic,
    headerTitle,
    lastActiveTopicMap,
    sharePrefillText,
    sharePrefillFiles,
    startShareSession,
    consumeSharePrefill,
    setConversation,
    clearConversation,
    isConversationCurrent,
    selectTopicById,
    selectItem,
  };
}, {
  persist: {
    pick: ['currentSelectedItem', 'currentTopicId', 'lastActiveTopicMap'],
  },
});

import { defineStore } from "pinia";
import { ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { useAssistantStore } from "./assistant";

export interface PickedFileInfo {
  path: string;
  name: string;
  mime: string;
  size: number;
  hash: string;
  thumbnailPath?: string;
}

export const useChatSessionStore = defineStore("chatSession", () => {
  const currentSelectedItem = ref<any>(null);
  const currentTopicId = ref<string | null>(null);
  const lastActiveTopicMap = ref<Record<string, string>>({});

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
    await selectTopicById(agentId, newTopic.id);

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
    topicId: string, 
    loadHistoryCallback?: (itemId: string, ownerType: string, topicId: string) => Promise<void>
  ) => {
    // 立即更新 currentTopicId，确保话题列表高亮实时响应
    currentTopicId.value = topicId;
    
    // 记录在该 itemId 下最后一次选中的活跃话题 ID
    lastActiveTopicMap.value[itemId] = topicId;

    const ownerType = assistantStore.agents.some((a) => a.id === itemId)
      ? "agent"
      : "group";

    // 设置当前选中的项目详情 (确保头像和色调同步)
    const agent = assistantStore.agents.find((a: any) => a.id === itemId);
    const group = assistantStore.groups.find((g) => g.id === itemId);
    if (agent) {
      currentSelectedItem.value = { ...agent, type: "agent" };
    } else if (group) {
      currentSelectedItem.value = { ...group, type: "group" };
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
    const ownerType = item.members ? 'group' : 'agent';
    
    // 如果已经选中了该项，且当前已有话题，则不重复加载
    if (currentSelectedItem.value?.id === ownerId && currentTopicId.value) {
      return;
    }

    // 1. 优先从 Pinia 持久化的 lastActiveTopicMap 中获取最后一次打开的话题 ID
    let targetTopicId = lastActiveTopicMap.value[ownerId];

    // 2. 如果 Pinia 中没有记录，则尝试获取该 Owner 下最新的话题
    if (!targetTopicId) {
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
    }

    if (targetTopicId) {
      await selectTopicById(ownerId, targetTopicId, loadHistoryCallback);
    } else {
      // 没有任何话题的极端情况
      console.warn(`[ChatSessionStore] No topics found for ${ownerId}`);
      currentSelectedItem.value = { ...item, type: ownerType };
      currentTopicId.value = null;
    }
  };

  return {
    currentSelectedItem,
    currentTopicId,
    lastActiveTopicMap,
    sharePrefillText,
    sharePrefillFiles,
    startShareSession,
    consumeSharePrefill,
    selectTopicById,
    selectItem,
  };
}, {
  persist: {
    pick: ['currentSelectedItem', 'currentTopicId', 'lastActiveTopicMap'],
  },
});

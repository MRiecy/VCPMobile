import { defineStore } from "pinia";
import { ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { useAssistantStore } from "./assistant";

export const useChatSessionStore = defineStore("chatSession", () => {
  const currentSelectedItem = ref<any>(null);
  const currentTopicId = ref<string | null>(null);

  const assistantStore = useAssistantStore();

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

    // 1. 获取目标项的最新配置（含 currentTopicId）
    let targetTopicId = item.currentTopicId;

    // 2. 如果没有记录的话题，或者记录的话题已失效，则尝试获取该 Owner 下最新的话题
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
    selectTopicById,
    selectItem,
  };
});

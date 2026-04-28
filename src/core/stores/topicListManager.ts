import { defineStore } from "pinia";
import { ref, computed } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { useChatManagerStore } from "./chatManager";
import { useNotificationStore } from "./notification";

/**
 * 话题接口定义
 */
export interface Topic {
  id: string;
  ownerId?: string; // 所属实体的 ID
  ownerType?: string; // 实体类型: agent | group
  name: string;
  createdAt: number;
  locked?: boolean;
  unread?: boolean;
  unreadCount?: number;
  msgCount?: number;
}

/**
 * 话题列表管理 Store
 */
export const useTopicStore = defineStore("topic", () => {
  const chatManager = useChatManagerStore();
  const notificationStore = useNotificationStore();

  // --- 状态 (State) ---
  const topics = ref<Topic[]>([]);
  const loading = ref(false);
  const searchTerm = ref("");
  const currentAgentId = ref<string | null>(null);

  // --- 事件监听 (Event Listeners) ---
  // 注意：topic-index-updated 事件当前在 Rust 侧未被 emit，已移除死代码

  /**
   * 使所有话题列表缓存失效
   * 同步完成后调用，确保下次切到任意 Agent/Group 时重新加载最新话题
   */
  const invalidateAllTopicCaches = () => {
    topics.value = [];
    // currentAgentId 保持不动，这样当前选中的话题列表会在 watch 中重新加载
    console.log("[TopicStore] All topic caches invalidated");
  };

  // --- 计算属性 (Getters) ---

  /**
   * 过滤后的搜索列表 (支持标题和日期搜索)
   */
  const filteredTopics = computed(() => {
    const term = searchTerm.value.toLowerCase().trim();
    if (!term) return topics.value;

    return topics.value.filter((topic) => {
      // 标题匹配
      const nameMatch = topic.name.toLowerCase().includes(term);

      // 日期匹配 (格式化后搜索)
      let dateMatch = false;
      const createdAt = (topic as any).createdAt || (topic as any).created_at;
      if (createdAt) {
        // Rust 返回的是毫秒级时间戳 (i64) 或秒级
        const date = new Date(createdAt > 1e11 ? createdAt : createdAt * 1000);
        const fullDateStr = `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")} ${String(date.getHours()).padStart(2, "0")}:${String(date.getMinutes()).padStart(2, "0")}`;
        const shortDateStr = `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
        dateMatch =
          fullDateStr.toLowerCase().includes(term) ||
          shortDateStr.toLowerCase().includes(term);
      }

      return nameMatch || dateMatch;
    });
  });

  // --- 核心 Action (Actions) ---

  /**
   * 加载话题列表
   * @param ownerId Agent ID or Group ID
   * @param ownerType "agent" or "group"
   */
  const loadTopicList = async (ownerId: string, ownerType: string) => {
    if (!ownerId) return;

    currentAgentId.value = ownerId;
    console.log(`[TopicStore] Loading topics for ${ownerType}: ${ownerId}`);
    loading.value = true;

    try {
      // 1. 从 Rust 获取基础话题列表
      // 命令对应 Rust 中的 get_topics
      const result = await invoke<any[]>("get_topics", { ownerId, ownerType });

      // 竞态检查：如果请求返回时，当前选中的 Agent 已经改变，则丢弃该结果
      if (currentAgentId.value !== ownerId) {
        console.warn(`[TopicStore] Discarding stale topics for ${ownerId} (Current: ${currentAgentId.value})`);
        return;
      }

      // 映射 Rust 字段到前端状态
      topics.value = result.map((t) => ({
        ...t,
        ownerId: ownerId,
        ownerType: ownerType,
        name: t.name || t.title || t.id,
        unreadCount: t.unreadCount || 0,
        msgCount: t.msgCount || 0,
      }));

      console.log(
        `[TopicStore] Topic list loaded (Backend computed): ${result.length} topics`,
      );
    } catch (e) {
      console.error("[TopicStore] Failed to load topics:", e);
    } finally {
      loading.value = false;
    }
  };

  /**
   * 创建新话题
   */
  const createTopic = async (
    ownerId: string,
    ownerType: string,
    name: string,
  ) => {
    try {
      console.log(
        `[TopicStore] Creating new topic "${name}" for ${ownerType} ${ownerId}`,
      );
      const newTopic = await invoke<Topic>("create_topic", {
        ownerId,
        ownerType,
        name,
      });

      // 初始化默认状态
      const topicWithState: Topic = {
        ...newTopic,
        ownerId,
        ownerType,
        unreadCount: 0,
        msgCount: 0,
        unread: false,
        locked: true,
      };

      topics.value.unshift(topicWithState);
      notificationStore.addNotification({
        type: "success",
        title: "话题创建成功",
        message: `已开启新话题: ${name}`,
        toastOnly: true,
      });
      return topicWithState;
    } catch (e: any) {
      console.error("[TopicStore] Failed to create topic:", e);

      // 统一错误通知
      notificationStore.addNotification({
        type: "error",
        title: "创建话题失败",
        message:
          typeof e === "string" ? e : e.message || "系统或网络异常，请稍后重试",
        duration: 5000,
      });

      throw e;
    }
  };

  /**
   * 删除话题
   */
  const deleteTopic = async (
    ownerId: string,
    ownerType: string,
    topicId: string,
  ) => {
    try {
      console.log(`[TopicStore] Deleting topic ${topicId}`);
      // 注意：确保 Rust 端已实现 delete_topic 命令
      await invoke("delete_topic", { ownerId, ownerType, topicId });

      topics.value = topics.value.filter((t) => t.id !== topicId);

      notificationStore.addNotification({
        type: "success",
        title: "话题删除成功",
        message: "话题及其记录已被移除",
        toastOnly: true,
      });

      // 如果删除的是当前选中的话题，通知 chatManager
      if (chatManager.currentTopicId === topicId) {
        // 修复响应式误用：在 setup store 中，跨 store 访问 ref 需要通过 .value
        (chatManager as any).currentTopicId = null;
        (chatManager as any).currentChatHistory = [];
      }
    } catch (e) {
      console.error("[TopicStore] Failed to delete topic:", e);
      throw e;
    }
  };

  /**
   * 更新话题标题
   */
  const updateTopicTitle = async (
    ownerId: string,
    ownerType: string,
    topicId: string,
    newTitle: string,
  ) => {
    try {
      console.log(
        `[TopicStore] Updating title for topic ${topicId} to "${newTitle}"`,
      );
      // 注意：确保 Rust 端已实现 update_topic_title 命令
      await invoke("update_topic_title", {
        ownerId,
        ownerType,
        topicId,
        title: newTitle,
      });

      const index = topics.value.findIndex((t) => t.id === topicId);
      if (index !== -1) {
        topics.value[index] = { ...topics.value[index], name: newTitle };
      }
    } catch (e) {
      console.error("[TopicStore] Failed to update topic title:", e);
      throw e;
    }
  };

  /**
   * 切换话题锁定状态
   */
  const toggleTopicLock = async (
    ownerId: string,
    ownerType: string,
    topicId: string,
  ) => {
    try {
      const index = topics.value.findIndex((t) => t.id === topicId);
      if (index === -1) return;

      const targetLockState = !topics.value[index].locked;
      console.log(
        `[TopicStore] Toggling lock for ${topicId} to ${targetLockState}`,
      );

      // 调用 Rust 命令切换锁定
      await invoke("toggle_topic_lock", {
        ownerId,
        ownerType,
        topicId,
        locked: targetLockState,
      });
      topics.value[index] = { ...topics.value[index], locked: targetLockState };
    } catch (e) {
      console.error("[TopicStore] Failed to toggle topic lock:", e);
      throw e;
    }
  };

  /**
   * 设置未读状态 (手动标记)
   */
  const setTopicUnread = async (
    ownerId: string,
    ownerType: string,
    topicId: string,
    unread: boolean,
  ) => {
    try {
      console.log(
        `[TopicStore] Setting unread state for ${topicId} to ${unread}`,
      );
      // 调用 Rust 命令更新状态
      await invoke("set_topic_unread", { ownerId, ownerType, topicId, unread });

      const index = topics.value.findIndex((t) => t.id === topicId);
      if (index !== -1) {
        topics.value[index] = { ...topics.value[index], unread: unread };
      }
    } catch (e) {
      console.error("[TopicStore] Failed to set topic unread:", e);
      throw e;
    }
  };

  return {
    topics,
    loading,
    searchTerm,
    filteredTopics,
    loadTopicList,
    createTopic,
    deleteTopic,
    updateTopicTitle,
    currentAgentId,
    toggleTopicLock,
    setTopicUnread,
    invalidateAllTopicCaches,
  };
});

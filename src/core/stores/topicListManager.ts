import { defineStore } from "pinia";
import { ref, computed } from "vue";
import { invoke, Channel } from "@tauri-apps/api/core";
import { useChatSessionStore } from "./chatSessionStore";

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
  const sessionStore = useChatSessionStore();
  const notificationStore = useNotificationStore();

  // --- 状态 (State) ---
  const topics = ref<Topic[]>([]);
  const loading = ref(false);
  const searchTerm = ref("");
  const currentAgentId = ref<string | null>(null);
  const currentOwnerType = ref<string | null>(null);
  let loadGeneration = 0;
  let activeLoadKey: string | null = null;
  let activeLoadPromise: Promise<void> | null = null;

  const ownerKey = (ownerId: string, ownerType: string) =>
    `${ownerType}:${ownerId}`;
  const isCurrentOwner = (ownerId: string, ownerType: string) =>
    currentAgentId.value === ownerId && currentOwnerType.value === ownerType;

  // --- 事件监听 (Event Listeners) ---
  // 注意：topic-index-updated 事件当前在 Rust 侧未被 emit，已移除死代码

  /**
   * 使所有话题列表缓存失效
   * 同步完成后调用，确保下次切到任意 Agent/Group 时重新加载最新话题
   */
  const invalidateAllTopicCaches = () => {
    // 使所有在途 Channel 失去提交资格；它们可以自然结束，但不能再污染新列表。
    loadGeneration += 1;
    activeLoadKey = null;
    activeLoadPromise = null;
    loading.value = false;
    topics.value = [];
    // 当前 owner 身份保留，由调用方显式 reload，避免依赖不会再次触发的 selection watch。
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
  const loadTopicList = (ownerId: string, owner_type: string): Promise<void> => {
    if (!ownerId) return Promise.resolve();

    const key = ownerKey(ownerId, owner_type);
    if (activeLoadKey === key && activeLoadPromise) {
      return activeLoadPromise;
    }

    const generation = ++loadGeneration;
    currentAgentId.value = ownerId;
    currentOwnerType.value = owner_type;
    console.log(`[TopicStore] Loading topics for ${owner_type}: ${ownerId}`);
    loading.value = true;
    topics.value = [];

    const requestTopics = new Map<string, Topic>();
    let request!: Promise<void>;
    request = (async () => {
      try {
      // 1. 创建 Channel 用于接收流式数据
      const channel = new Channel<Topic[]>();

      channel.onmessage = (chunk) => {
        // owner + generation 双重校验覆盖同 owner 重入和 A -> B -> A。
        if (generation !== loadGeneration || !isCurrentOwner(ownerId, owner_type)) return;

        const mappedChunk = chunk.map((t) => ({
          ...t,
          ownerId: ownerId,
          ownerType: owner_type,
          name: t.name || (t as any).title || t.id,
          unreadCount: (t as any).unreadCount || 0,
          msgCount: (t as any).msgCount || 0,
        }));

        for (const topic of mappedChunk) {
          requestTopics.set(topic.id, topic);
        }
        topics.value = [...requestTopics.values()];
      };

      // 调用 Rust 命令开始流式获取
      await invoke("get_topics_streamed", { 
        ownerId, 
        ownerType: owner_type,
        onChunk: channel 
      });

      console.log(
        `[TopicStore] Topic list streaming completed for ${ownerId}`,
      );
      } catch (e) {
        if (generation === loadGeneration) {
          console.error("[TopicStore] Failed to load topics:", e);
        }
      } finally {
        if (generation === loadGeneration) {
          loading.value = false;
        }
        if (activeLoadPromise === request) {
          activeLoadKey = null;
          activeLoadPromise = null;
        }
      }
    })();

    activeLoadKey = key;
    activeLoadPromise = request;
    return request;
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

      if (isCurrentOwner(ownerId, ownerType)) {
        topics.value = [
          topicWithState,
          ...topics.value.filter((topic) => topic.id !== topicWithState.id),
        ];
      }
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

      if (isCurrentOwner(ownerId, ownerType)) {
        topics.value = topics.value.filter((t) => t.id !== topicId);
      }

      notificationStore.addNotification({
        type: "success",
        title: "话题删除成功",
        message: "话题及其记录已被移除",
        toastOnly: true,
      });

      // 如果删除的是当前选中的话题，自动载入最新的一个
      if (
        sessionStore.currentSelectedItem?.id === ownerId &&
        sessionStore.currentSelectedItem?.type === ownerType &&
        sessionStore.currentTopicId === topicId
      ) {
        const nextTopic = topics.value[0];
        if (nextTopic) {
          await sessionStore.selectTopicById(ownerId, nextTopic.id);
        } else {
          sessionStore.setConversation(sessionStore.currentSelectedItem, null);
          // chatHistoryStore 监听 currentTopicId 变化，会自动清空历史
        }
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

      if (!isCurrentOwner(ownerId, ownerType)) return;
      const index = topics.value.findIndex((t) => t.id === topicId);
      if (index !== -1) {
        topics.value[index] = { ...topics.value[index], name: newTitle };
        // 强制触发虚拟列表重绘
        topics.value = [...topics.value];
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
      if (!isCurrentOwner(ownerId, ownerType)) return;
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
      if (!isCurrentOwner(ownerId, ownerType)) return;
      const currentIndex = topics.value.findIndex((t) => t.id === topicId);
      if (currentIndex === -1) return;
      topics.value[currentIndex] = {
        ...topics.value[currentIndex],
        locked: targetLockState,
      };
      // 强制触发虚拟列表重绘
      topics.value = [...topics.value];
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

      if (!isCurrentOwner(ownerId, ownerType)) return;
      const index = topics.value.findIndex((t) => t.id === topicId);
      if (index !== -1) {
        topics.value[index] = { ...topics.value[index], unread: unread };
        // 强制触发虚拟列表重绘
        topics.value = [...topics.value];
      }
    } catch (e) {
      console.error("[TopicStore] Failed to set topic unread:", e);
      throw e;
    }
  };

  /**
   * 增加话题的消息计数 (UI 乐观更新)
   */
  const incrementTopicMsgCount = (topicId: string) => {
    const index = topics.value.findIndex((t) => t.id === topicId);
    if (index !== -1) {
      topics.value[index] = { 
        ...topics.value[index], 
        msgCount: (topics.value[index].msgCount || 0) + 1 
      };
      topics.value = [...topics.value];
    }
  };

  /**
   * 增加话题的未读计数 (UI 乐观更新)
   */
  const incrementTopicUnreadCount = (topicId: string) => {
    const index = topics.value.findIndex((t) => t.id === topicId);
    if (index !== -1) {
      const topic = topics.value[index];
      // 如果不是当前选中的话题，才增加未读数
      if (sessionStore.currentTopicId !== topicId) {
        topics.value[index] = { 
          ...topic, 
          unreadCount: (topic.unreadCount || 0) + 1,
          unread: true
        };
        topics.value = [...topics.value];
      }
    }
  };

  /**
   * 减少话题的消息计数 (UI 乐观更新)
   */
  const decrementTopicMsgCount = (topicId: string, count: number = 1) => {
    const index = topics.value.findIndex((t) => t.id === topicId);
    if (index !== -1) {
      topics.value[index] = { 
        ...topics.value[index], 
        msgCount: Math.max(0, (topics.value[index].msgCount || 0) - count) 
      };
      topics.value = [...topics.value];
    }
  };

  /**
   * 标记话题为已读 (清空未读数并取消未读标记)
   */
  const markTopicAsRead = (topicId: string) => {
    const index = topics.value.findIndex((t) => t.id === topicId);
    if (index !== -1) {
      const topic = topics.value[index];
      if (topic.unread || (topic.unreadCount && topic.unreadCount > 0)) {
        topics.value[index] = { 
          ...topic, 
          unread: false, 
          unreadCount: 0 
        };
        topics.value = [...topics.value];
        
        // 同步到后端
        invoke("set_topic_unread", { 
          ownerId: topic.ownerId, 
          ownerType: topic.ownerType, 
          topicId, 
          unread: false 
        }).catch(() => {});
      }
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
    incrementTopicMsgCount,
    incrementTopicUnreadCount,
    decrementTopicMsgCount,
    markTopicAsRead,
  };
});

import { defineStore } from "pinia";
import { ref, computed } from "vue";
import { invoke, Channel } from "@tauri-apps/api/core";
import {
  useChatSessionStore,
  type ConversationOwnerType,
} from "./chatSessionStore";
import { useAssistantStore } from "./assistant";
import type { TopicDto } from "../types/assistant";

import { useNotificationStore } from "./notification";

export type TopicSortMode = "created" | "updated";
export type Topic = TopicDto & { updatedAt: number };

const normalizeSortMode = (value: unknown): TopicSortMode =>
  value === "updated" ? "updated" : "created";

const normalizeTopicTimestamp = (
  value: number | null | undefined,
  fallback: number,
) => (
  typeof value === "number" && Number.isFinite(value) && value > 0
    ? value
    : fallback
);

interface TopicUnreadMutationDto {
  unreadCount: number;
  updatedAt: number | null;
}

const topicPreferenceKey = (
  ownerId: string,
  ownerType: string,
  topicId: string,
) => JSON.stringify([ownerType, ownerId, topicId]);

const parseTopicPreferenceKey = (
  key: string,
): [string, string, string] | null => {
  try {
    const parsed: unknown = JSON.parse(key);
    if (
      Array.isArray(parsed) &&
      parsed.length === 3 &&
      parsed.every((part) => typeof part === "string")
    ) {
      return parsed as [string, string, string];
    }
  } catch {
    // 损坏的本地展示偏好由下一次成功列表加载清理。
  }
  return null;
};

/**
 * 话题列表管理 Store
 */
export const useTopicStore = defineStore("topic", () => {
  const sessionStore = useChatSessionStore();
  const assistantStore = useAssistantStore();
  const notificationStore = useNotificationStore();

  // --- 状态 (State) ---
  const topics = ref<Topic[]>([]);
  const loading = ref(false);
  const searchTerm = ref("");
  const sortMode = ref<TopicSortMode>("created");
  const pinnedTopicKeys = ref<string[]>([]);
  const currentAgentId = ref<string | null>(null);
  const currentOwnerType = ref<string | null>(null);
  let loadGeneration = 0;
  let activeLoadKey: string | null = null;
  let activeLoadPromise: Promise<void> | null = null;
  let unreadMutationTail: Promise<void> = Promise.resolve();

  const ownerKey = (ownerId: string, ownerType: string) =>
    `${ownerType}:${ownerId}`;
  const isCurrentOwner = (ownerId: string, ownerType: string) =>
    currentAgentId.value === ownerId && currentOwnerType.value === ownerType;
  const effectiveSortMode = computed<TopicSortMode>(() =>
    normalizeSortMode(sortMode.value),
  );
  const validPinnedKeys = () =>
    Array.isArray(pinnedTopicKeys.value)
      ? pinnedTopicKeys.value.filter((key) => typeof key === "string")
      : [];
  const pinnedKeySet = computed(
    () => new Set(validPinnedKeys()),
  );
  const isTopicPinned = (
    ownerId: string,
    ownerType: string,
    topicId: string,
  ) => pinnedKeySet.value.has(topicPreferenceKey(ownerId, ownerType, topicId));
  const enqueueUnreadMutation = <T>(operation: () => Promise<T>): Promise<T> => {
    const queued = unreadMutationTail.catch(() => undefined).then(operation);
    unreadMutationTail = queued.then(
      () => undefined,
      () => undefined,
    );
    return queued;
  };

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
      const createdAt = topic.createdAt;
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

  const compareDescending = (left: number, right: number) => {
    if (left === right) return 0;
    return left > right ? -1 : 1;
  };

  const compareTopics = (left: Topic, right: Topic) => {
    if (effectiveSortMode.value === "updated") {
      const updatedComparison = compareDescending(
        left.updatedAt,
        right.updatedAt,
      );
      if (updatedComparison !== 0) return updatedComparison;
    }

    const createdComparison = compareDescending(left.createdAt, right.createdAt);
    if (createdComparison !== 0) return createdComparison;
    if (left.id === right.id) return 0;
    return left.id > right.id ? -1 : 1;
  };

  const topicSections = computed(() => {
    const pinned: Topic[] = [];
    const regular: Topic[] = [];
    const orderedTopics = [...filteredTopics.value].sort(compareTopics);

    for (const topic of orderedTopics) {
      const target = isTopicPinned(topic.ownerId, topic.ownerType, topic.id)
        ? pinned
        : regular;
      target.push(topic);
    }

    return { pinned, regular };
  });

  const setSortMode = (mode: TopicSortMode) => {
    sortMode.value = normalizeSortMode(mode);
  };

  const toggleTopicPinned = (
    ownerId: string,
    ownerType: string,
    topicId: string,
  ) => {
    const key = topicPreferenceKey(ownerId, ownerType, topicId);
    if (pinnedKeySet.value.has(key)) {
      pinnedTopicKeys.value = validPinnedKeys().filter(
        (candidate) => candidate !== key,
      );
      return false;
    }

    pinnedTopicKeys.value = [...validPinnedKeys(), key];
    return true;
  };

  const removePinnedTopic = (
    ownerId: string,
    ownerType: string,
    topicId: string,
  ) => {
    const key = topicPreferenceKey(ownerId, ownerType, topicId);
    if (!pinnedKeySet.value.has(key)) return;
    pinnedTopicKeys.value = validPinnedKeys().filter(
      (candidate) => candidate !== key,
    );
  };

  const pruneMissingPinsForOwner = (
    ownerId: string,
    ownerType: string,
    liveTopicIds: Set<string>,
  ) => {
    pinnedTopicKeys.value = validPinnedKeys().filter((key) => {
      const identity = parseTopicPreferenceKey(key);
      if (!identity) return false;
      const [savedOwnerType, savedOwnerId, savedTopicId] = identity;
      if (savedOwnerType !== ownerType || savedOwnerId !== ownerId) return true;
      return liveTopicIds.has(savedTopicId);
    });
  };

  const setTopicUpdatedAt = (
    ownerId: string,
    ownerType: string,
    topicId: string,
    updatedAt: number,
  ) => {
    if (!isCurrentOwner(ownerId, ownerType)) return;
    const index = topics.value.findIndex((topic) => topic.id === topicId);
    if (index === -1) return;
    const nextUpdatedAt = normalizeTopicTimestamp(
      updatedAt,
      topics.value[index].updatedAt,
    );
    if (nextUpdatedAt === topics.value[index].updatedAt) return;
    topics.value[index] = {
      ...topics.value[index],
      updatedAt: nextUpdatedAt,
    };
    topics.value = [...topics.value];
  };

  // --- 核心 Action (Actions) ---

  /**
   * 加载话题列表
   * @param ownerId Agent ID or Group ID
   * @param ownerType "agent" or "group"
   */
  const loadTopicList = (
    ownerId: string,
    owner_type: ConversationOwnerType,
  ): Promise<void> => {
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
      const channel = new Channel<TopicDto[]>();

      channel.onmessage = (chunk) => {
        // owner + generation 双重校验覆盖同 owner 重入和 A -> B -> A。
        if (generation !== loadGeneration || !isCurrentOwner(ownerId, owner_type)) return;

        const mappedChunk: Topic[] = chunk.map((t) => ({
          ...t,
          ownerId: ownerId,
          ownerType: owner_type,
          name: t.name || t.id,
          updatedAt: normalizeTopicTimestamp(t.updatedAt, t.createdAt),
          unreadCount: t.unreadCount ?? 0,
          msgCount: t.msgCount ?? 0,
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

      if (generation === loadGeneration && isCurrentOwner(ownerId, owner_type)) {
        pruneMissingPinsForOwner(
          ownerId,
          owner_type,
          new Set(requestTopics.keys()),
        );
        sessionStore.reconcileCurrentConversation(topics.value);
      }

      console.log(
        `[TopicStore] Topic list streaming completed for ${ownerId}`,
      );
      } catch (e) {
        if (generation === loadGeneration) {
          console.error("[TopicStore] Failed to load topics:", e);
          notificationStore.addNotification({
            type: "error",
            title: "话题加载失败",
            message: "暂时无法读取话题列表，请稍后重试",
            toastOnly: true,
          });
        }
        throw e;
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
    ownerType: ConversationOwnerType,
    name: string,
  ) => {
    try {
      console.log(
        `[TopicStore] Creating new topic "${name}" for ${ownerType} ${ownerId}`,
      );
      const newTopic = await invoke<TopicDto>("create_topic", {
        ownerId,
        ownerType,
        name,
      });

      // 初始化默认状态
      const topicWithState: Topic = {
        ...newTopic,
        ownerId,
        ownerType,
        updatedAt: normalizeTopicTimestamp(newTopic.updatedAt, newTopic.createdAt),
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
    ownerType: ConversationOwnerType,
    topicId: string,
  ) => {
    try {
      console.log(`[TopicStore] Deleting topic ${topicId}`);
      const replacementDto = await invoke<TopicDto | null>("delete_topic", {
        ownerId,
        ownerType,
        topicId,
      });
      const replacement: Topic | null = replacementDto
        ? {
            ...replacementDto,
            ownerId,
            ownerType,
            updatedAt: normalizeTopicTimestamp(
              replacementDto.updatedAt,
              replacementDto.createdAt,
            ),
          }
        : null;
      removePinnedTopic(ownerId, ownerType, topicId);

      if (isCurrentOwner(ownerId, ownerType)) {
        const remaining = topics.value.filter((t) => t.id !== topicId);
        topics.value = replacement ? [replacement, ...remaining] : remaining;
      }
      if (ownerType === "agent") {
        await enqueueUnreadMutation(() => assistantStore.refreshUnreadCounts());
      }

      notificationStore.addNotification({
        type: "success",
        title: "话题删除成功",
        message: "话题及其记录已被移除",
        toastOnly: true,
      });

      // 如果删除的是当前选中的话题，按剩余列表恢复或清空当前 Topic。
      if (
        sessionStore.currentSelectedItem?.id === ownerId &&
        sessionStore.currentSelectedItem?.type === ownerType &&
        sessionStore.currentTopicId === topicId
      ) {
        if (replacement) {
          await sessionStore.selectTopicById(ownerId, ownerType, replacement.id);
        } else {
          sessionStore.reconcileCurrentConversation(topics.value);
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
    expectedTitle?: string,
  ) => {
    try {
      console.log(
        `[TopicStore] Updating title for topic ${topicId} to "${newTitle}"`,
      );
      // 注意：确保 Rust 端已实现 update_topic_title 命令
      const updatedAt = await invoke<number | null>("update_topic_title", {
        ownerId,
        ownerType,
        topicId,
        title: newTitle,
        expectedTitle: expectedTitle ?? null,
      });

      if (updatedAt === null) return false;
      if (!isCurrentOwner(ownerId, ownerType)) return;
      const index = topics.value.findIndex((t) => t.id === topicId);
      if (index !== -1) {
        topics.value[index] = {
          ...topics.value[index],
          name: newTitle,
          updatedAt,
        };
        // 强制触发虚拟列表重绘
        topics.value = [...topics.value];
      }
      return true;
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
      const updatedAt = await invoke<number | null>("toggle_topic_lock", {
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
        updatedAt: updatedAt ?? topics.value[currentIndex].updatedAt,
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
  ) =>
    enqueueUnreadMutation(async () => {
      try {
        console.log(
          `[TopicStore] Setting unread state for ${topicId} to ${unread}`,
        );
        const updatedAt = await invoke<number | null>("set_topic_unread", {
          ownerId,
          ownerType,
          topicId,
          unread,
        });

        if (isCurrentOwner(ownerId, ownerType)) {
          const index = topics.value.findIndex((t) => t.id === topicId);
          if (index !== -1) {
            topics.value[index] = {
              ...topics.value[index],
              unread,
              unreadCount: unread ? topics.value[index].unreadCount : 0,
              updatedAt: updatedAt ?? topics.value[index].updatedAt,
            };
            topics.value = [...topics.value];
          }
        }
        await assistantStore.refreshUnreadCounts();
      } catch (e) {
        console.error("[TopicStore] Failed to set topic unread:", e);
        throw e;
      }
    });

  /**
   * 增加话题的消息计数 (UI 乐观更新)
   */
  const incrementTopicMsgCount = (ownerId: string, ownerType: string, topicId: string) => {
    if (!isCurrentOwner(ownerId, ownerType)) return;
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
   * 记录后台 Agent 回复并采用数据库返回的最终未读计数
   */
  const incrementTopicUnreadCount = (
    ownerId: string,
    ownerType: string,
    topicId: string,
  ): Promise<void> => {
    if (ownerType !== "agent") return Promise.resolve();
    return enqueueUnreadMutation(async () => {
      try {
        const result = await invoke<TopicUnreadMutationDto>("increment_topic_unread_count", {
          ownerId,
          ownerType,
          topicId,
        });
        const currentKey = sessionStore.currentConversationKey;
        const isCurrentConversation = Boolean(
          currentKey &&
            currentKey.ownerId === ownerId &&
            currentKey.ownerType === ownerType &&
            currentKey.topicId === topicId,
        );
        if (isCurrentOwner(ownerId, ownerType) && !isCurrentConversation) {
          const index = topics.value.findIndex((topic) => topic.id === topicId);
          if (index !== -1) {
            topics.value[index] = {
              ...topics.value[index],
              unreadCount: result.unreadCount,
              unread: true,
              updatedAt: result.updatedAt ?? topics.value[index].updatedAt,
            };
            topics.value = [...topics.value];
          }
        }
        await assistantStore.refreshUnreadCounts();
      } catch (e) {
        console.error("[TopicStore] Failed to increment topic unread count:", e);
        throw e;
      }
    });
  };

  /**
   * 减少话题的消息计数 (UI 乐观更新)
   */
  const decrementTopicMsgCount = (
    ownerId: string,
    ownerType: string,
    topicId: string,
    count: number = 1,
  ) => {
    if (!isCurrentOwner(ownerId, ownerType)) return;
    const index = topics.value.findIndex((t) => t.id === topicId);
    if (index !== -1) {
      topics.value[index] = { 
        ...topics.value[index], 
        msgCount: Math.max(0, (topics.value[index].msgCount || 0) - count) 
      };
      topics.value = [...topics.value];
    }
  };

  const setTopicMsgCount = (
    ownerId: string,
    ownerType: string,
    topicId: string,
    count: number,
  ) => {
    if (!isCurrentOwner(ownerId, ownerType)) return;
    const index = topics.value.findIndex((topic) => topic.id === topicId);
    if (index === -1) return;
    topics.value[index] = {
      ...topics.value[index],
      msgCount: Math.max(0, count),
    };
    topics.value = [...topics.value];
  };

  /**
   * 标记话题为已读 (清空未读数并取消未读标记)
   */
  const markTopicAsRead = (ownerId: string, ownerType: string, topicId: string) => {
    if (ownerType !== "agent") return;
    void setTopicUnread(ownerId, ownerType, topicId, false).catch(() => {});
  };

  return {
    topics,
    loading,
    searchTerm,
    sortMode,
    effectiveSortMode,
    pinnedTopicKeys,
    filteredTopics,
    topicSections,
    setSortMode,
    isTopicPinned,
    toggleTopicPinned,
    setTopicUpdatedAt,
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
    setTopicMsgCount,
    markTopicAsRead,
  };
}, {
  persist: {
    pick: ["sortMode", "pinnedTopicKeys"],
  },
});

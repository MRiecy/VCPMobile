import { defineStore } from "pinia";
import { computed, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { useNotificationStore } from "./notification";

export interface Topic {
  id: string;
  name: string;
  createdAt: number;
  locked: boolean;
  unread: boolean;
  unreadCount: number;
  msgCount: number;
  ownerId: string;
  ownerType: string;
}

export interface AgentConfig {
  id: string;
  name: string;
  model: string;
  systemPrompt: string;
  temperature: number;
  contextTokenLimit: number;
  maxOutputTokens: number;
  streamOutput: boolean;
  avatarCalculatedColor?: string;
  topics?: Topic[];
  currentTopicId?: string | null;
}

export interface GroupConfig {
  id: string;
  name: string;
  avatarCalculatedColor?: string;
  members: string[];
  mode: string;
  memberTags?: Record<string, any>;
  groupPrompt?: string;
  invitePrompt?: string;
  useUnifiedModel: boolean;
  unifiedModel?: string;
  tagMatchMode?: string;
  topics?: Topic[];
  currentTopicId?: string | null;
  createdAt?: number;
}

export const useAssistantStore = defineStore("assistant", () => {
  const agents = ref<AgentConfig[]>([]);
  const groups = ref<GroupConfig[]>([]);
  const loading = ref(false);
  const error = ref<string | null>(null);
  const notificationStore = useNotificationStore();

  // 同步完成刷新已集中到 main.ts（window.location.reload），此处无需重复监听

  // 记录每个 item (agent 或 group) 的未读数量
  const unreadCounts = ref<Record<string, number>>({});

  const refreshUnreadCountsForItems = async (
    fetchedItems: (AgentConfig | GroupConfig)[],
  ) => {
    try {
      for (const item of fetchedItems) {
        try {
          const ownerType = (item as any).members ? "group" : "agent";
          const topics = await invoke<any[]>("get_topics", {
            ownerId: item.id,
            ownerType,
          });
          let hasUnread = false;
          let totalCount = 0;

          for (const topic of topics) {
            if (topic.unread) hasUnread = true;
            if (topic.unreadCount > 0) {
              totalCount += topic.unreadCount;
              hasUnread = true;
            }
          }

          if (hasUnread) {
            unreadCounts.value[item.id] = totalCount > 0 ? totalCount : -1;
          } else {
            delete unreadCounts.value[item.id];
          }
        } catch (err) {
          console.warn(
            `[AssistantStore] Failed to fetch topics for unread count ${item.id}:`,
            err,
          );
        }
      }
    } catch (err) {
      console.error("[AssistantStore] refreshUnreadCountsForItems error", err);
    }
  };

  const combinedItems = computed(() => [
    ...agents.value.map((agent) => ({ ...agent, type: "agent" as const })),
    ...groups.value.map((group) => ({ ...group, type: "group" as const })),
  ]);

  const fetchAgents = async () => {
    loading.value = true;
    error.value = null;
    try {
      const fetchedAgents = await invoke<AgentConfig[]>("get_agents");
      agents.value = fetchedAgents;
      refreshUnreadCountsForItems(fetchedAgents);
    } catch (e: any) {
      const msg = e.toString();
      error.value = msg;
      console.error("[AssistantStore] fetchAgents failed:", e);
      throw e;
    } finally {
      loading.value = false;
    }
  };

  const fetchGroups = async () => {
    loading.value = true;
    error.value = null;
    try {
      const fetchedGroups = await invoke<GroupConfig[]>("get_groups");
      groups.value = fetchedGroups;
      refreshUnreadCountsForItems(fetchedGroups);
    } catch (e: any) {
      const msg = e.toString();
      error.value = msg;
      console.error("[AssistantStore] fetchGroups failed:", e);
      throw e;
    } finally {
      loading.value = false;
    }
  };

  const createAgent = async (name: string) => {
    loading.value = true;
    try {
      const newAgent = await invoke<AgentConfig>("create_agent", { name });
      notificationStore.addNotification({
        type: "success",
        title: "Agent 创建成功",
        message: `助手 "${name}" 已就绪`,
        toastOnly: true,
      });
      // 不再自动全局 fetch，由生命周期或调用方决定是否增量更新
      return newAgent;
    } catch (e: any) {
      error.value = e.toString();
      throw e;
    } finally {
      loading.value = false;
    }
  };

  const deleteAgent = async (id: string) => {
    try {
      await invoke("delete_agent", { agentId: id });
      await fetchAgents();
      notificationStore.addNotification({
        type: "success",
        title: "Agent 删除成功",
        message: "助手已从列表中移除",
        toastOnly: true,
      });
    } catch (e: any) {
      console.error("[AssistantStore] Failed to delete agent:", e);
      throw e;
    }
  };

  const createGroup = async (name: string) => {
    loading.value = true;
    try {
      const newGroup = await invoke<GroupConfig>("create_group", { name });
      notificationStore.addNotification({
        type: "success",
        title: "Group 创建成功",
        message: `群组 "${name}" 已创建`,
        toastOnly: true,
      });
      // 不再自动全局 fetch
      return newGroup;
    } catch (e: any) {
      error.value = e.toString();
      throw e;
    } finally {
      loading.value = false;
    }
  };

  const deleteGroup = async (id: string) => {
    try {
      await invoke("delete_group", { groupId: id });
      await fetchGroups();
      notificationStore.addNotification({
        type: "success",
        title: "Group 删除成功",
        message: "群组已解散",
        toastOnly: true,
      });
    } catch (e: any) {
      console.error("[AssistantStore] Failed to delete group:", e);
      throw e;
    }
  };

  const saveAgent = async (agent: AgentConfig) => {
    try {
      await invoke("save_agent_config", { agent });
      notificationStore.addNotification({
        type: "success",
        title: "Agent 配置保存成功",
        message: "助手的最新设置已同步到核心",
        toastOnly: true,
      });
      await fetchAgents();
    } catch (e: any) {
      error.value = e.toString();
      throw e;
    }
  };

  const saveGroup = async (group: GroupConfig) => {
    try {
      await invoke("save_group_config", { group });
      notificationStore.addNotification({
        type: "success",
        title: "Group 配置保存成功",
        message: "群组设置已更新",
        toastOnly: true,
      });
      await fetchGroups();
    } catch (e: any) {
      error.value = e.toString();
      throw e;
    }
  };

  const saveAvatar = async (ownerType: 'agent' | 'group' | 'user', ownerId: string, mimeType: string, imageData: number[]) => {
    try {
      const hash = await invoke<string>("save_avatar_data", {
        ownerType,
        ownerId,
        mimeType,
        imageData,
      });
      
      const label = ownerType === 'agent' ? 'Agent' : ownerType === 'group' ? 'Group' : '用户';
      notificationStore.addNotification({
        type: "success",
        title: `${label} 头像更新成功`,
        message: "新头像已生效",
        toastOnly: true,
      });
      
      return hash;
    } catch (e: any) {
      console.error(`[AssistantStore] Failed to save avatar for ${ownerType}:`, e);
      throw e;
    }
  };

  return {
    agents,
    groups,
    combinedItems,
    loading,
    error,
    unreadCounts,
    fetchAgents,
    fetchGroups,
    createAgent,
    deleteAgent,
    createGroup,
    deleteGroup,
    saveAgent,
    saveGroup,
    saveAvatar,
    refreshUnreadCountsForItems,
  };
});

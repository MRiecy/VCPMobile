import { defineStore } from "pinia";
import { computed, ref, shallowRef } from "vue";
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
  mobileSystemPrompt?: string;
  temperature?: number;
  contextTokenLimit?: number;
  maxOutputTokens?: number;
  streamOutput?: boolean;
  useTemperature?: boolean;
  avatarCalculatedColor?: string;
  topics?: Topic[];
}

export interface GroupConfig {
  id: string;
  name: string;
  avatarCalculatedColor?: string;
  members: string[];
  mode?: string;
  memberTags?: Record<string, any>;
  groupPrompt?: string;
  invitePrompt?: string;
  useUnifiedModel?: boolean;
  unifiedModel?: string;
  tagMatchMode?: string;
  topics?: Topic[];
  createdAt?: number;
}

export const useAssistantStore = defineStore("assistant", () => {
  const agents = shallowRef<AgentConfig[]>([]);
  const groups = shallowRef<GroupConfig[]>([]);
  const loading = ref(false);
  const error = ref<string | null>(null);
  const notificationStore = useNotificationStore();

  // 同步完成刷新已集中到 main.ts（window.location.reload），此处无需重复监听

  // 记录每个 item (agent 或 group) 的未读数量
  const unreadCounts = ref<Record<string, number>>({});

  /**
   * 批量刷新未读计数（替代 N+1 逐个查询）
   * 调用后端 get_unread_counts 一次获取所有 owner 的未读状态
   */
  const refreshUnreadCounts = async () => {
    try {
      const counts = await invoke<Record<string, number>>("get_unread_counts");
      unreadCounts.value = counts;
    } catch (err) {
      console.error("[AssistantStore] Failed to refresh unread counts:", err);
    }
  };



  const combinedItems = computed(() => [
    ...agents.value.map((agent) => ({ ...agent, type: "agent" as const })),
    ...groups.value.map((group) => ({ ...group, type: "group" as const })),
  ]);

  const fetchAgents = async () => {
    loading.value = true;
    error.value = null;
    const startTime = Date.now();
    try {
      console.log("[Profile] fetchAgents invoking get_assistants_snapshot...");
      const snapshot = await invoke<{
        agents: AgentConfig[];
        groups: GroupConfig[];
        unreadCounts: Record<string, number>;
      }>("get_assistants_snapshot");
      agents.value = snapshot.agents;
      unreadCounts.value = snapshot.unreadCounts;
      console.log(`[Profile] fetchAgents finished in ${Date.now() - startTime}ms`);
    } catch (e: any) {
      error.value = e.toString();
      console.error("[AssistantStore] fetchAgents failed:", e);
      throw e;
    } finally {
      loading.value = false;
    }
  };

  const fetchGroups = async () => {
    loading.value = true;
    error.value = null;
    const startTime = Date.now();
    try {
      console.log("[Profile] fetchGroups invoking get_assistants_snapshot...");
      const snapshot = await invoke<{
        agents: AgentConfig[];
        groups: GroupConfig[];
        unreadCounts: Record<string, number>;
      }>("get_assistants_snapshot");
      groups.value = snapshot.groups;
      unreadCounts.value = snapshot.unreadCounts;
      console.log(`[Profile] fetchGroups finished in ${Date.now() - startTime}ms`);
    } catch (e: any) {
      error.value = e.toString();
      console.error("[AssistantStore] fetchGroups failed:", e);
      throw e;
    } finally {
      loading.value = false;
    }
  };

  const fetchAgentsAndGroups = async () => {
    loading.value = true;
    error.value = null;
    const startTime = Date.now();
    try {
      console.log("[Profile] invoke('get_assistants_snapshot') starting...");
      const snapshot = await invoke<{
        agents: AgentConfig[];
        groups: GroupConfig[];
        unreadCounts: Record<string, number>;
      }>("get_assistants_snapshot");
      console.log(`[Profile] invoke('get_assistants_snapshot') resolved in ${Date.now() - startTime}ms`);
      
      // 在同一次 tick 中合并赋值，触发 Vue 3 渲染的批处理更新
      agents.value = snapshot.agents;
      groups.value = snapshot.groups;
      unreadCounts.value = snapshot.unreadCounts;
      
      console.log(`[Profile] fetchAgentsAndGroups finished in ${Date.now() - startTime}ms`);
    } catch (e: any) {
      error.value = e.toString();
      console.error("[AssistantStore] fetchAgentsAndGroups failed:", e);
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
      agents.value = agents.value.filter((a) => a.id !== id);
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
      groups.value = groups.value.filter((g) => g.id !== id);
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
      
      // 点对点局部更新（仅更新轻量列表渲染字段，防止大提示词等字段污染全局列表轻量缓存）
      const index = agents.value.findIndex((a) => a.id === agent.id);
      if (index !== -1) {
        const updated = [...agents.value];
        updated[index] = {
          ...updated[index],
          name: agent.name,
          model: agent.model,
          avatarCalculatedColor: agent.avatarCalculatedColor || updated[index].avatarCalculatedColor,
        };
        agents.value = updated;
      }

      notificationStore.addNotification({
        type: "success",
        title: "Agent 配置保存成功",
        message: "助手的最新设置已同步到核心",
        toastOnly: true,
      });
    } catch (e: any) {
      error.value = e.toString();
      throw e;
    }
  };

  const saveGroup = async (group: GroupConfig) => {
    try {
      await invoke("save_group_config", { group });

      // 点对点局部更新（仅更新轻量列表渲染字段）
      const index = groups.value.findIndex((g) => g.id === group.id);
      if (index !== -1) {
        const updated = [...groups.value];
        updated[index] = {
          ...updated[index],
          name: group.name,
          members: group.members,
          // mode 被邀请横条与 @提及 选择器消费，必须随保存同步
          mode: group.mode,
          avatarCalculatedColor: group.avatarCalculatedColor || updated[index].avatarCalculatedColor,
        };
        groups.value = updated;
      }

      notificationStore.addNotification({
        type: "success",
        title: "Group 配置保存成功",
        message: "群组设置已更新",
        toastOnly: true,
      });
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
    fetchAgentsAndGroups,
    createAgent,
    deleteAgent,
    createGroup,
    deleteGroup,
    saveAgent,
    saveGroup,
    saveAvatar,
    refreshUnreadCounts,
  };
});

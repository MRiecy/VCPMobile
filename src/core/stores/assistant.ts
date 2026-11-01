import { defineStore } from "pinia";
import { computed, ref, shallowRef } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { useAvatarStore } from "./avatar";
import { useNotificationStore } from "./notification";
import { allowChatMutation, notifyCallDeletionBlocked } from "../../features/chat/chatMutationPolicy";
import type {
  AgentConfigDto,
  AgentListItemDto,
  AssistantListItem,
  AssistantsSnapshotDto,
  GroupConfigDto,
  GroupListItemDto,
} from "../types/assistant";

export const useAssistantStore = defineStore("assistant", () => {
  const agents = shallowRef<AgentListItemDto[]>([]);
  const groups = shallowRef<GroupListItemDto[]>([]);
  const loading = ref(false);
  const error = ref<string | null>(null);
  const notificationStore = useNotificationStore();
  const avatarStore = useAvatarStore();
  let activeLoadingOperations = 0;
  let snapshotLoadId = 0;
  const beginLoading = () => {
    activeLoadingOperations += 1;
    loading.value = true;
  };
  const endLoading = () => {
    activeLoadingOperations = Math.max(0, activeLoadingOperations - 1);
    loading.value = activeLoadingOperations > 0;
  };
  const invalidateSnapshotLoads = () => {
    snapshotLoadId += 1;
  };

  // 同步关闭时由 syncSession 的统一数据重载刷新列表，此处无需重复监听。

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



  const combinedItems = computed<AssistantListItem[]>(() => [
    ...agents.value.map((agent) => ({ ...agent, type: "agent" as const })),
    ...groups.value.map((group) => ({ ...group, type: "group" as const })),
  ]);

  const fetchAgentsAndGroups = async () => {
    const loadId = ++snapshotLoadId;
    beginLoading();
    error.value = null;
    const startTime = Date.now();
    try {
      console.log("[Profile] invoke('get_assistants_snapshot') starting...");
      const snapshot = await invoke<AssistantsSnapshotDto>("get_assistants_snapshot");
      console.log(`[Profile] invoke('get_assistants_snapshot') resolved in ${Date.now() - startTime}ms`);
      if (loadId !== snapshotLoadId) return;

      // 在同一次 tick 中合并赋值，触发 Vue 3 渲染的批处理更新
      agents.value = snapshot.agents;
      groups.value = snapshot.groups;
      unreadCounts.value = snapshot.unreadCounts;
      
      console.log(`[Profile] fetchAgentsAndGroups finished in ${Date.now() - startTime}ms`);
    } catch (e: any) {
      if (loadId === snapshotLoadId) error.value = e.toString();
      console.error("[AssistantStore] fetchAgentsAndGroups failed:", e);
      throw e;
    } finally {
      endLoading();
    }
  };

  const createAgent = async (name: string) => {
    beginLoading();
    try {
      const newAgent = await invoke<AgentConfigDto>("create_agent", { name });
      invalidateSnapshotLoads();
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
      endLoading();
    }
  };

  const deleteAgent = async (id: string) => {
    if (!allowChatMutation({ ownerId: id, ownerType: "agent" })) return false;
    try {
      await invoke("delete_agent", { agentId: id });
      invalidateSnapshotLoads();
      avatarStore.clearCache("agent", id);
      agents.value = agents.value.filter((a) => a.id !== id);
      groups.value = groups.value.map((group) => ({
        ...group,
        members: group.members.filter((memberId) => memberId !== id),
      }));
      notificationStore.addNotification({
        type: "success",
        title: "Agent 删除成功",
        message: "助手已从列表中移除",
        toastOnly: true,
      });
      return true;
    } catch (e: any) {
      if (notifyCallDeletionBlocked(e)) return false;
      console.error("[AssistantStore] Failed to delete agent:", e);
      throw e;
    }
  };

  const createGroup = async (name: string) => {
    beginLoading();
    try {
      const newGroup = await invoke<GroupConfigDto>("create_group", { name });
      invalidateSnapshotLoads();
      notificationStore.addNotification({
        type: "success",
        title: "群组已创建",
        message: `群组 "${name}" 已创建`,
        toastOnly: true,
      });
      // 不再自动全局 fetch
      return newGroup;
    } catch (e: any) {
      error.value = e.toString();
      throw e;
    } finally {
      endLoading();
    }
  };

  const deleteGroup = async (id: string) => {
    if (!allowChatMutation({ ownerId: id, ownerType: "group" })) return false;
    try {
      await invoke("delete_group", { groupId: id });
      invalidateSnapshotLoads();
      avatarStore.clearCache("group", id);
      groups.value = groups.value.filter((g) => g.id !== id);
      notificationStore.addNotification({
        type: "success",
        title: "群组已解散",
        message: "群组已解散",
        toastOnly: true,
      });
      return true;
    } catch (e: any) {
      if (notifyCallDeletionBlocked(e)) return false;
      console.error("[AssistantStore] Failed to delete group:", e);
      throw e;
    }
  };

  const saveAgent = async (agent: AgentConfigDto, baseline: AgentConfigDto): Promise<AgentConfigDto> => {
    try {
      const canonical = await invoke<AgentConfigDto>("save_agent_config", { agent, baseline });
      invalidateSnapshotLoads();
      
      // 点对点局部更新（仅更新轻量列表渲染字段，防止大提示词等字段污染全局列表轻量缓存）
      const index = agents.value.findIndex((a) => a.id === agent.id);
      if (index !== -1) {
        const updated = [...agents.value];
        updated[index] = {
          ...updated[index],
          name: canonical.name,
          model: canonical.model,
          avatarCalculatedColor: canonical.avatarCalculatedColor,
        };
        agents.value = updated;
      }

      notificationStore.addNotification({
        type: "success",
        title: "Agent 配置保存成功",
        message: "助手的最新设置已同步到核心",
        toastOnly: true,
      });
      return canonical;
    } catch (e: any) {
      error.value = e.toString();
      throw e;
    }
  };

  const saveGroup = async (group: GroupConfigDto, baseline?: GroupConfigDto) => {
    try {
      // A local behavior edit must not write back shared fields captured before
      // a concurrent sync. Send only the fields this editor actually changed.
      const editableFields = [
        "name", "members", "memberTags", "groupPrompt", "invitePrompt",
        "useUnifiedModel", "unifiedModel", "tagMatchMode", "chatMode",
        "modeSettings", "enableContextMessageWindow", "contextMessageWindowSize",
      ] as const;
      const updates = baseline ? Object.fromEntries(editableFields
        .filter(field => JSON.stringify(group[field]) !== JSON.stringify(baseline[field]))
        .map(field => [field, group[field]])) : undefined;
      const canonical = baseline
        ? await invoke<GroupConfigDto>("update_group_config", { groupId: group.id, updates })
        : await invoke<GroupConfigDto>("save_group_config", { group });
      invalidateSnapshotLoads();

      // 点对点局部更新（仅更新轻量列表渲染字段）
      const index = groups.value.findIndex((g) => g.id === group.id);
      if (index !== -1) {
        const updated = [...groups.value];
        updated[index] = {
          ...updated[index],
          name: canonical.name,
          members: canonical.members,
          // 群聊模式独立保存，同步的 mode 不参与运行或列表呈现。
          mode: canonical.chatMode,
          avatarCalculatedColor: canonical.avatarCalculatedColor || updated[index].avatarCalculatedColor,
        };
        groups.value = updated;
      }

      notificationStore.addNotification({
        type: "success",
        title: "群组设置已保存",
        message: "群组设置已更新",
        toastOnly: true,
      });
      return canonical;
    } catch (e: any) {
      error.value = e.toString();
      throw e;
    }
  };

  const saveAvatar = async (ownerType: 'agent' | 'group' | 'user', ownerId: string, mimeType: string, imageData: Uint8Array) => {
    try {
      const hash = await invoke<string>("save_avatar_data", {
        ownerType,
        ownerId,
        mimeType,
        imageData,
      });
      await avatarStore.refreshAvatar(ownerType, ownerId, hash);
      
      const label = ownerType === 'agent' ? 'Agent' : ownerType === 'group' ? '群组' : '用户';
      notificationStore.addNotification({
        type: "success",
        title: `${label} 头像更新成功`,
        message: "新头像已生效",
        toastOnly: true,
      });
      
      return hash;
    } catch (e: any) {
      console.error(`[AssistantStore] Failed to save avatar for ${ownerType}:`, e);
      const label = ownerType === 'agent' ? 'Agent' : ownerType === 'group' ? '群组' : '用户';
      notificationStore.addNotification({
        type: "error",
        title: `${label} 头像更新失败`,
        message: e?.toString?.() || "请重新选择图片后重试",
        toastOnly: true,
      });
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

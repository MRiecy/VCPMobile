import { defineStore } from "pinia";
import { computed, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";

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
  top_p?: number;
  top_k?: number;
  streamOutput: boolean;
  avatarCalculatedColor?: string;
  topics: Topic[];
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
  topics: Topic[];
}

export const useAssistantStore = defineStore("assistant", () => {
  const agents = ref<AgentConfig[]>([]);
  const groups = ref<GroupConfig[]>([]);
  const loading = ref(false);
  const error = ref<string | null>(null);

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
      // 不再自动全局 fetch，由生命周期或调用方决定是否增量更新
      return newAgent;
    } catch (e: any) {
      error.value = e.toString();
      throw e;
    } finally {
      loading.value = false;
    }
  };

  const createGroup = async (name: string) => {
    loading.value = true;
    try {
      const newGroup = await invoke<GroupConfig>("create_group", { name });
      // 不再自动全局 fetch
      return newGroup;
    } catch (e: any) {
      error.value = e.toString();
      throw e;
    } finally {
      loading.value = false;
    }
  };

  const saveAgent = async (agent: AgentConfig) => {
    try {
      await invoke("save_agent_config", { agent });
      await fetchAgents();
    } catch (e: any) {
      error.value = e.toString();
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
    createGroup,
    saveAgent,
    refreshUnreadCountsForItems,
  };
});

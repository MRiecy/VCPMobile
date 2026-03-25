import { defineStore } from 'pinia';
import { ref } from 'vue';
import { invoke, convertFileSrc } from '@tauri-apps/api/core';

export interface AgentConfig {
  id: string;
  name: string;
  model: string;
  systemPrompt: string;
  avatarUrl?: string;
  avatarCalculatedColor?: string;
  resolvedAvatarUrl?: string;
  temperature: number;
  contextTokenLimit?: number;
  maxOutputTokens?: number;
  topP?: number;
  topK?: number;
  disableCustomColors?: boolean;
  useThemeColorsInChat?: boolean;
  avatarBorderColor?: string;
  nameTextColor?: string;
  customCss?: string;
  cardCss?: string;
  chatCss?: string;
  stripRegexes?: any[];
}

export interface GroupConfig {
  id: string;
  name: string;
  avatar?: string;
  avatarCalculatedColor?: string;
  resolvedAvatarUrl?: string;
  members: string[];
  mode: string;
  groupPrompt?: string;
  invitePrompt?: string;
  tagMatchMode?: string;
}

export const useAssistantStore = defineStore('assistant', () => {
  const agents = ref<AgentConfig[]>([]);
  const groups = ref<GroupConfig[]>([]);
  const loading = ref(false);
  const error = ref<string | null>(null);
  
  // 记录每个 item (agent 或 group) 的未读数量
  const unreadCounts = ref<Record<string, number>>({});

  const fetchUnreadCounts = async (fetchedItems: (AgentConfig | GroupConfig)[]) => {
    try {
      for (const item of fetchedItems) {
        try {
          const topics = await invoke<any[]>('get_topics', { itemId: item.id });
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
          console.warn(`[AssistantStore] Failed to fetch topics for unread count ${item.id}:`, err);
        }
      }
    } catch(err) {
       console.error('[AssistantStore] fetchUnreadCounts error', err);
    }
  };

  const fetchAgents = async () => {
    loading.value = true;
    error.value = null;
    try {
      const fetchedAgents = await invoke<AgentConfig[]>('get_agents');
      fetchedAgents.forEach((agent) => {
        if (agent.avatarUrl && !agent.avatarUrl.startsWith('http') && !agent.avatarUrl.startsWith('data:')) {
          try {
            agent.resolvedAvatarUrl = convertFileSrc(agent.avatarUrl);
          } catch (err) {
            console.warn(`[AssistantStore] Failed to convert avatar path for ${agent.id}:`, err);
          }
        }
      });
      agents.value = fetchedAgents;
      fetchUnreadCounts(fetchedAgents);
    } catch (e: any) {
      error.value = e.toString();
    } finally {
      loading.value = false;
    }
  };

  const fetchGroups = async () => {
    try {
      const fetchedGroups = await invoke<GroupConfig[]>('get_groups');
      fetchedGroups.forEach((group) => {
        if (group.avatar && !group.avatar.startsWith('http') && !group.avatar.startsWith('data:')) {
           try {
             group.resolvedAvatarUrl = convertFileSrc(group.avatar);
           } catch (err) {
             console.warn(`[AssistantStore] Failed to convert group avatar path for ${group.id}:`, err);
           }
        }
      });
      groups.value = fetchedGroups;
      fetchUnreadCounts(fetchedGroups);
    } catch (e: any) {
      console.error('Failed to fetch groups:', e);
    }
  };

  const createAgent = async (name: string) => {
    loading.value = true;
    try {
      const newAgent = await invoke<AgentConfig>('create_agent', { name });
      await fetchAgents();
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
      const newGroup = await invoke<GroupConfig>('create_group', { name });
      await fetchGroups();
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
      await invoke('save_agent_config', { agent });
      await fetchAgents();
    } catch (e: any) {
      error.value = e.toString();
      throw e;
    }
  };

  return {
    agents,
    groups,
    loading,
    error,
    unreadCounts,
    fetchAgents,
    fetchGroups,
    createAgent,
    createGroup,
    saveAgent,
    fetchUnreadCounts
  };
});


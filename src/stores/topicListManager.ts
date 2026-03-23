import { defineStore } from 'pinia';
import { ref, computed } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { useChatManagerStore } from './chatManager';

/**
 * 话题接口定义
 */
export interface Topic {
  id: string;
  name: string;
  createdAt: number; // 修正为驼峰命名，对齐 Rust 端的 #[serde(rename = "createdAt")]
  locked?: boolean;
  unread?: boolean;
  unreadCount?: number; // 界面显示的计数: >0 为数字, -1 为小点
  messageCount?: number; // 话题中的总消息数
}

/**
 * 话题列表管理 Store
 */
export const useTopicStore = defineStore('topic', () => {
  const chatManager = useChatManagerStore();
  
  // --- 状态 (State) ---
  const topics = ref<Topic[]>([]);
  const loading = ref(false);
  const searchTerm = ref('');
  const currentAgentId = ref<string | null>(null);

  // --- 计算属性 (Getters) ---
  
  /**
   * 过滤后的搜索列表 (支持标题和日期搜索)
   */
  const filteredTopics = computed(() => {
    const term = searchTerm.value.toLowerCase().trim();
    if (!term) return topics.value;

    return topics.value.filter(topic => {
      // 标题匹配
      const nameMatch = topic.name.toLowerCase().includes(term);
      
      // 日期匹配 (格式化后搜索)
      let dateMatch = false;
      const createdAt = (topic as any).createdAt || (topic as any).created_at;
      if (createdAt) {
        // Rust 返回的是毫秒级时间戳 (i64) 或秒级
        const date = new Date(createdAt > 1e11 ? createdAt : createdAt * 1000);
        const fullDateStr = `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, '0')}-${String(date.getDate()).padStart(2, '0')} ${String(date.getHours()).padStart(2, '0')}:${String(date.getMinutes()).padStart(2, '0')}`;
        const shortDateStr = `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, '0')}-${String(date.getDate()).padStart(2, '0')}`;
        dateMatch = fullDateStr.toLowerCase().includes(term) || shortDateStr.toLowerCase().includes(term);
      }
      
      return nameMatch || dateMatch;
    });
  });

  // --- 核心 Action (Actions) ---

  /**
   * 加载话题列表
   * @param agentId Agent ID
   */
  const loadTopicList = async (agentId: string) => {
    if (!agentId) return;
    
    currentAgentId.value = agentId;
    console.log(`[TopicStore] Loading topics for agent: ${agentId}`);
    loading.value = true;
    
    try {
      // 1. 从 Rust 获取基础话题列表
      // 命令对应 Rust 中的 get_topics
      // 这里的 Topic 结构体已经由 Rust 后端在扫描 history.json 时算好了 unread_count 和 msg_count
      const result = await invoke<any[]>('get_topics', { itemId: agentId });
      
      // 映射 Rust 字段到前端状态 (Rust 已对齐 camelCase)
      topics.value = result.map(t => ({
        ...t,
        name: t.title || t.name || t.id,
        unreadCount: t.unreadCount || 0,
        messageCount: t.messageCount || 0
      }));

      console.log(`[TopicStore] Topic list loaded (Backend computed): ${result.length} topics`);
    } catch (e) {
      console.error('[TopicStore] Failed to load topics:', e);
    } finally {
      loading.value = false;
    }
  };

  /**
   * 创建新话题
   */
  const createTopic = async (agentId: string, name: string) => {
    try {
      console.log(`[TopicStore] Creating new topic "${name}" for agent ${agentId}`);
      const newTopic = await invoke<Topic>('create_topic', { itemId: agentId, name });
      
      // 初始化默认状态
      const topicWithState: Topic = {
        ...newTopic,
        unreadCount: 0,
        messageCount: 0,
        unread: false,
        locked: false
      };
      
      topics.value.unshift(topicWithState);
      return topicWithState;
    } catch (e) {
      console.error('[TopicStore] Failed to create topic:', e);
      throw e;
    }
  };

  /**
   * 删除话题
   */
  const deleteTopic = async (agentId: string, topicId: string) => {
    try {
      console.log(`[TopicStore] Deleting topic ${topicId}`);
      // 注意：确保 Rust 端已实现 delete_topic 命令
      await invoke('delete_topic', { itemId: agentId, topicId });
      
      topics.value = topics.value.filter(t => t.id !== topicId);
      
      // 如果删除的是当前选中的话题，通知 chatManager
      if (chatManager.currentTopicId === topicId) {
        chatManager.currentTopicId = null;
        chatManager.currentChatHistory = [];
      }
    } catch (e) {
      console.error('[TopicStore] Failed to delete topic:', e);
      throw e;
    }
  };

  /**
   * 更新话题标题
   */
  const updateTopicTitle = async (agentId: string, topicId: string, newTitle: string) => {
    try {
      console.log(`[TopicStore] Updating title for topic ${topicId} to "${newTitle}"`);
      // 注意：确保 Rust 端已实现 update_topic_title 命令
      await invoke('update_topic_title', { itemId: agentId, topicId, title: newTitle });
      
      const topic = topics.value.find(t => t.id === topicId);
      if (topic) {
        topic.name = newTitle;
      }
    } catch (e) {
      console.error('[TopicStore] Failed to update topic title:', e);
      throw e;
    }
  };

  /**
   * 切换话题锁定状态
   */
  const toggleTopicLock = async (agentId: string, topicId: string) => {
    try {
      const topic = topics.value.find(t => t.id === topicId);
      if (!topic) return;

      const targetLockState = !topic.locked;
      console.log(`[TopicStore] Toggling lock for ${topicId} to ${targetLockState}`);
      
      // 调用 Rust 命令切换锁定
      await invoke('toggle_topic_lock', { itemId: agentId, topicId, locked: targetLockState });
      topic.locked = targetLockState;
    } catch (e) {
      console.error('[TopicStore] Failed to toggle topic lock:', e);
      throw e;
    }
  };

  /**
   * 设置未读状态 (手动标记)
   */
  const setTopicUnread = async (agentId: string, topicId: string, unread: boolean) => {
    try {
      console.log(`[TopicStore] Setting unread state for ${topicId} to ${unread}`);
      // 调用 Rust 命令更新状态
      await invoke('set_topic_unread', { itemId: agentId, topicId, unread });
      
      const topic = topics.value.find(t => t.id === topicId);
      if (topic) {
        topic.unread = unread;
      }
    } catch (e) {
      console.error('[TopicStore] Failed to set topic unread:', e);
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
    toggleTopicLock,
    setTopicUnread,
  };
});

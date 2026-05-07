import { clearHtmlCache } from '../utils/astRenderer';
import { useChatHistoryStore } from '../stores/chatHistoryStore';
import { useChatSessionStore } from '../stores/chatSessionStore';
import { useAssistantStore } from '../stores/assistant';
import { useTopicStore } from '../stores/topicListManager';

/**
 * 统一的数据重载逻辑：重建/同步/主题切换后调用，
 * 确保前端所有缓存层（AST HTML 缓存、消息数组、话题列表）与后端一致。
 */
export function useDataReload() {
  const chatHistoryStore = useChatHistoryStore();
  const sessionStore = useChatSessionStore();
  const assistantStore = useAssistantStore();
  const topicStore = useTopicStore();

  const performFullReload = async () => {
    // 1. 清理 AST HTML 缓存（重建/同步后 AST 结构可能已变）
    clearHtmlCache();

    // 2. 刷新 agents/groups 元数据
    await Promise.all([
      assistantStore.fetchAgents(),
      assistantStore.fetchGroups(),
    ]);

    // 3. 清理话题列表缓存
    topicStore.invalidateAllTopicCaches();

    // 4. 如果当前在某个话题中，重新加载消息以获取最新 AST
    if (sessionStore.currentTopicId && sessionStore.currentSelectedItem) {
      await chatHistoryStore.loadHistoryPaginated(
        sessionStore.currentSelectedItem.id,
        sessionStore.currentSelectedItem.type,
        sessionStore.currentTopicId,
      );
    }
  };

  return { performFullReload };
}

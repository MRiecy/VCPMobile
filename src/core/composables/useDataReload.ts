import { clearHtmlCache } from '../utils/astRenderer';
import { useChatHistoryStore } from '../stores/chatHistoryStore';
import { useChatSessionStore } from '../stores/chatSessionStore';
import { useAssistantStore } from '../stores/assistant';
import { useAvatarStore } from '../stores/avatar';
import { useTopicStore } from '../stores/topicListManager';

/**
 * 统一的数据重载逻辑：重建/同步/主题切换后调用，
 * 确保前端所有缓存层（AST HTML 缓存、消息数组、话题列表）与后端一致。
 */
export function useDataReload() {
  const chatHistoryStore = useChatHistoryStore();
  const sessionStore = useChatSessionStore();
  const assistantStore = useAssistantStore();
  const avatarStore = useAvatarStore();
  const topicStore = useTopicStore();

  const performFullReload = async () => {
    // 1. 清理 AST HTML 缓存（重建/同步后 AST 结构可能已变）
    clearHtmlCache();

    // 2. 刷新 agents/groups 元数据与同步后可能变化的头像缓存
    await Promise.all([
      assistantStore.fetchAgents(),
      assistantStore.fetchGroups(),
      avatarStore.refreshAll(),
    ]);
    sessionStore.reconcileCurrentConversation();

    // 3. 清理话题列表缓存
    topicStore.invalidateAllTopicCaches();

    // 4. 当前 owner 的列表必须显式重载；selection watch 的依赖没有变化，不会自行触发。
    if (sessionStore.currentSelectedItem) {
      await topicStore.loadTopicList(
        sessionStore.currentSelectedItem.id,
        sessionStore.currentSelectedItem.type,
      );
    }

    // 5. 如果当前在某个话题中，重新加载消息以获取最新 AST
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

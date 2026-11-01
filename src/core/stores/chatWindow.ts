import { computed, onScopeDispose, ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import type { ChatMessage } from '../types/chat';
import { useChatSessionStore, type ConversationKey } from './chatSessionStore';
import { useChatStreamStore } from './chatStreamStore';
import { useAttachmentStore } from './attachmentStore';
import { useNotificationStore } from './notification';
import { useTopicStore } from './topicListManager';

export const MAX_HISTORY_MESSAGES = 500;

export interface HistoryPageResult {
  addedCount: number;
  error?: unknown;
  aborted?: boolean;
}

interface WindowPage {
  memberIds: string[];
  messages: ChatMessage[];
  hasMoreOlder: boolean;
  hasMoreNewer: boolean;
  pinned: boolean;
  anchorRemoved: boolean;
  replacementAnchorId: string | null;
  topicActivity: { msgCount: number; updatedAt: number; unread: boolean; unreadCount: number };
}

type TopicKey = Omit<ConversationKey, 'epoch'>;
const matchesTopic = (left: TopicKey, right: TopicKey) =>
  left.ownerId === right.ownerId && left.ownerType === right.ownerType && left.topicId === right.topicId;
export const compareMessages = (a: ChatMessage, b: ChatMessage) =>
  a.timestamp - b.timestamp || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0);

/** One projection of the Rust-accepted observation window. Scroll physics stay in ChatView. */
export function useChatWindow() {
  const session = useChatSessionStore();
  const streams = useChatStreamStore();
  const topics = useTopicStore();
  const attachments = useAttachmentStore();
  const notifications = useNotificationStore();
  const currentChatHistory = ref<ChatMessage[]>([]);
  const loadedConversationKey = ref<ConversationKey | null>(null);
  const loadedWindowRevision = ref<number | null>(null);
  const hasMoreHistory = ref(false);
  const hasEvictedNewer = ref(false);
  const anchorRemoved = ref(false);
  const replacementAnchorId = ref<string | null>(null);
  const pinned = ref(false);
  const isLoadingHistory = ref(false);
  const followingLatest = ref(false);
  const anchored = computed(() => Boolean(session.acceptedWindow?.anchorId));
  const ready = computed(() => session.isConversationCurrent(loadedConversationKey.value)
    && loadedWindowRevision.value === (session.acceptedWindow?.windowRevision ?? null));
  const canCommitConversation = (key: ConversationKey) => ready.value && session.isConversationCurrent(key);

  type Operation = 'open' | 'older' | 'reconcile';
  interface Read {
    key: ConversationKey;
    revision: number;
    operation: Operation;
    changed: Set<string> | null;
    force: boolean;
    result: Promise<HistoryPageResult>;
  }
  let reading: Read | null = null;
  let disposed = false;
  const resetHistoryForConversation = () => {
    reading = null;
    currentChatHistory.value = [];
    loadedConversationKey.value = null;
    loadedWindowRevision.value = null;
    hasMoreHistory.value = false;
    hasEvictedNewer.value = false;
    anchorRemoved.value = false;
    replacementAnchorId.value = null;
    pinned.value = false;
    isLoadingHistory.value = false;
  };
  onScopeDispose(() => { disposed = true; reading = null; });

  // Capture committed changes across the SQLite/IPC await. A burst larger
  // than a whole window requests one fresh baseline instead of growing a log.
  const noteMessageChange = (key: TopicKey, id: string) => {
    if (!reading || !matchesTopic(reading.key, key) || !reading.changed) return;
    reading.changed.add(id);
    if (reading.changed.size > MAX_HISTORY_MESSAGES) reading.changed = null;
  };

  const projectLiveMessage = (key: ConversationKey, message: ChatMessage) => {
    noteMessageChange(key, message.id);
    if (!canCommitConversation(key)) return;
    const existing = currentChatHistory.value.findIndex(row => row.id === message.id);
    if (existing !== -1) {
      currentChatHistory.value[existing] = message;
      return;
    }
    const oldest = currentChatHistory.value[0];
    if (oldest && compareMessages(message, oldest) < 0) return;
    if (anchored.value || pinned.value || hasEvictedNewer.value ||
      (!followingLatest.value && currentChatHistory.value.length >= MAX_HISTORY_MESSAGES)) {
      hasEvictedNewer.value = true;
      return;
    }
    const rows = [...currentChatHistory.value, message].sort(compareMessages);
    if (rows.length > MAX_HISTORY_MESSAGES) hasMoreHistory.value = true;
    currentChatHistory.value = rows.slice(-MAX_HISTORY_MESSAGES);
  };

  const read = (operation: Operation, force = false, pinOlder = true): Promise<HistoryPageResult> => {
    const key = session.currentConversationKey;
    const view = session.acceptedWindow;
    if (disposed || !key || !view) return Promise.resolve({ addedCount: 0, aborted: true });
    if (operation === 'open' && ready.value) return Promise.resolve({ addedCount: 0 });
    if (reading?.revision === view.windowRevision && session.isConversationCurrent(reading.key)) {
      const pending = reading;
      if (operation === 'open' || (operation === pending.operation && (!force || pending.force))) return pending.result;
      return pending.result.then(previous => previous.aborted || !session.isConversationCurrent(key)
        || session.acceptedWindow?.windowRevision !== view.windowRevision
        ? { addedCount: 0, aborted: true } : read(operation, force, pinOlder));
    }
    if (loadedConversationKey.value && !session.isConversationCurrent(loadedConversationKey.value)) {
      resetHistoryForConversation();
    }
    const topic = { ownerId: key.ownerId, ownerType: key.ownerType, topicId: key.topicId };
    const identity = { key: topic, documentId: view.documentId, focusRevision: view.focusRevision, windowRevision: view.windowRevision };
    const task: Read = { key, revision: view.windowRevision, operation, force, changed: new Set(),
      result: Promise.resolve({ addedCount: 0 }) };
    const current = () => !disposed && reading === task && session.isConversationCurrent(key)
      && session.acceptedWindow?.windowRevision === view.windowRevision
      && session.acceptedWindow.documentId === view.documentId;
    reading = task;
    isLoadingHistory.value = true;
    task.result = (async (): Promise<HistoryPageResult> => {
      let mode = operation;
      do {
        const existing = ready.value ? currentChatHistory.value : [];
        const oldest = existing[0];
        if (mode === 'older' && !oldest) return { addedCount: 0, aborted: true };
        const changes = task.changed;
        const observedIds = existing.map(row => row.id);
        const known = force || !changes ? [] : existing.filter(row => !changes.has(row.id)).map(row => ({
          id: row.id, contentHash: row.content_hash, finishReason: row.finishReason, updatedAt: row.updatedAt,
        }));
        task.changed = new Set();
        const page = mode === 'reconcile'
          ? await invoke<WindowPage>('reconcile_chat_window', {
            ...identity, observedIds, followLatest: followingLatest.value && !anchored.value && !pinned.value && !hasEvictedNewer.value,
            known, force,
          })
          : await invoke<WindowPage>('read_chat_window', {
            ...identity, observedIds, known, older: mode === 'older', pinOlder, limit: mode === 'older' ? 10 : 5,
            before: mode === 'older' ? { timestamp: oldest.timestamp, id: oldest.id } : null,
          });
        if (!current()) return { addedCount: 0, aborted: true };
        if (!task.changed || task.changed.size) {
          // Rust may have advanced pagination already. Re-read those members
          // without advancing a second page or installing the stale result.
          mode = 'reconcile';
          continue;
        }
        const byId = new Map(existing.map(row => [row.id, row]));
        let addedCount = 0;
        for (const row of page.messages) {
          if (!byId.has(row.id)) addedCount++;
          byId.set(row.id, streams.acceptPersistedMessage(key, row));
          attachments.resolveMessageAssets(row);
        }
        const rows = page.memberIds.map(id => {
          const row = byId.get(id);
          if (!row) throw new Error('聊天窗口缺少消息内容');
          return row;
        });
        if (page.anchorRemoved && (!ready.value || !anchorRemoved.value)) {
          notifications.addNotification({ type: 'info', title: '原消息已删除', message: '已保留此处仍存在的聊天记录。', toastOnly: true });
        }
        currentChatHistory.value = rows;
        hasMoreHistory.value = page.hasMoreOlder;
        hasEvictedNewer.value = page.hasMoreNewer;
        anchorRemoved.value = page.anchorRemoved;
        replacementAnchorId.value = page.replacementAnchorId;
        pinned.value = page.pinned;
        loadedConversationKey.value = key;
        loadedWindowRevision.value = view.windowRevision;
        topics.projectWindowActivity(key.ownerId, key.ownerType, key.topicId, page.topicActivity);
        return { addedCount };
      } while (current());
      return { addedCount: 0, aborted: true };
    })().catch(error => {
      if (!current() || String(error) === 'stale_window') return { addedCount: 0, aborted: true };
      if (operation !== 'reconcile') notifications.addNotification({
        id: 'chat-window-recovery', type: 'error', title: '历史加载失败', message: String(error), toastOnly: true,
      });
      return { addedCount: 0, error };
    }).finally(() => {
      if (reading === task) { reading = null; isLoadingHistory.value = false; }
    });
    return task.result;
  };

  const ensureWindow = () => read('open');
  const reconcileWindow = async (force = false) => {
    if (!session.acceptedWindow) return;
    const result = await read(ready.value ? 'reconcile' : 'open', force);
    if (result.error) throw result.error;
  };
  const loadMoreHistory = (pinOlder = true) => ready.value && hasMoreHistory.value
    ? read('older', false, pinOlder) : Promise.resolve<HistoryPageResult>({ addedCount: 0, aborted: true });
  const returnToLatest = async (): Promise<HistoryPageResult> => {
    const key = session.currentConversationKey;
    if (!key || !ready.value) return { addedCount: 0, aborted: true };
    if (anchored.value || pinned.value || hasEvictedNewer.value) {
      try {
        const result = await session.selectTopicById(key.ownerId, key.ownerType, key.topicId);
        if (result !== 'accepted') {
          session.notifyNavigationResult(result);
          return { addedCount: 0, aborted: true };
        }
      } catch (error) {
        notifications.addNotification({ type: 'error', title: '无法返回最新消息', message: String(error), toastOnly: true });
        return { addedCount: 0, error };
      }
    }
    return ensureWindow();
  };
  return { currentChatHistory, loadedConversationKey, loadedWindowRevision, isLoadingHistory, hasMoreHistory,
    hasEvictedNewer, followingLatest, pinned, anchored, anchorRemoved, replacementAnchorId, ready, canCommitConversation,
    noteMessageChange, projectLiveMessage, ensureWindow, loadMoreHistory, returnToLatest,
    reconcileWindow, resetHistoryForConversation };
}

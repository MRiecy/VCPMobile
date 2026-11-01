import type { ConversationOwnerItem } from '../../core/types/assistant';
import type { ChatMessage } from '../../core/types/chat';
import type { useChatSessionStore } from '../../core/stores/chatSessionStore';
import { mockInvoke } from '../mocks/tauri';

type WindowFlags = { hasMoreOlder?: boolean; hasMoreNewer?: boolean; pinned?: boolean; anchorRemoved?: boolean; replacementAnchorId?: string | null };

export function installWindow(
  session: ReturnType<typeof useChatSessionStore>, item: ConversationOwnerItem,
  topicId: string, anchorId: string | null = null,
) {
  const revision = (session.acceptedWindow?.windowRevision ?? 0) + 1;
  session.setConversation(item, topicId, {
    documentId: 1, focusRevision: revision, windowRevision: revision, anchorId,
  });
}

export function windowPage(rows: ChatMessage[], flags: WindowFlags = {}) {
  return {
    messages: rows, memberIds: rows.map(row => row.id),
    hasMoreOlder: false, hasMoreNewer: false, pinned: false, anchorRemoved: false, replacementAnchorId: null, ...flags,
    topicActivity: { msgCount: rows.length, updatedAt: 0, unread: false, unreadCount: 0 },
  };
}

export function mockWindowRead(
  rows: ChatMessage[] | ((args?: Record<string, unknown>) => ChatMessage[] | Promise<ChatMessage[]>),
  flags?: WindowFlags,
) {
  mockInvoke('read_chat_window', async args =>
    windowPage(typeof rows === 'function' ? await rows(args) : rows, flags));
}

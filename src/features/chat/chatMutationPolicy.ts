import { useAssistantStore } from "../../core/stores/assistant";
import { useChatStreamStore } from "../../core/stores/chatStreamStore";
import { useNotificationStore } from "../../core/stores/notification";
import type { ConversationOwnerType } from "../../core/stores/chatSessionStore";

/** A missing topic means the mutation removes the complete owner. */
export interface ChatMutationScope {
  ownerId: string;
  ownerType: ConversationOwnerType;
  topicId?: string;
}

// Read the stream owner's activity at render time and again immediately before
// mutation. A menu or confirmation may have been opened before generation began.
export function chatMutationBlockReason(scope: ChatMutationScope): string | null {
  const streams = useChatStreamStore();
  if (!streams.activityInventoryKnown) {
    return streams.connectionError
      ? "尚未取得生成状态，请返回前台或在网络恢复后重试。"
      : "正在连接聊天状态，请稍候。";
  }
  if (scope.topicId !== undefined) {
    return streams.isConversationBusy(scope.ownerId, scope.ownerType, scope.topicId)
      ? "此会话正在生成或接续，请等待结束后再操作。"
      : null;
  }
  if (streams.isOwnerBusy(scope.ownerId, scope.ownerType)) {
    return "包含正在生成或接续的会话，请等待结束后再删除。";
  }
  // Deleting an Agent also removes its membership from groups. Keep an active
  // group's next speaker configuration intact until its turn finishes.
  if (scope.ownerType === "agent" && useAssistantStore().groups.some(group =>
    group.members.includes(scope.ownerId) && streams.isOwnerBusy(group.id, "group"),
  )) {
    return "此 Agent 所在的群组正在发言，请等待结束后再删除。";
  }
  return null;
}

export function allowChatMutation(scope: ChatMutationScope): boolean {
  const reason = chatMutationBlockReason(scope);
  if (!reason) return true;
  useNotificationStore().addNotification({
    type: "info",
    title: useChatStreamStore().activityInventoryKnown ? "请等待生成结束" : "聊天状态尚未就绪",
    message: reason,
    toastOnly: true,
  });
  return false;
}

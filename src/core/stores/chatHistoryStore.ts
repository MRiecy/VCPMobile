import { defineStore } from "pinia";
import { ref, watch } from "vue";
import { useChatWindow } from "./chatWindow";
import { invoke, Channel } from "@tauri-apps/api/core";
import { useChatSessionStore } from "./chatSessionStore";
import { useChatStreamStore } from "./chatStreamStore";
import { useAttachmentStore } from "./attachmentStore";
import { useAssistantStore } from "./assistant";
import { useSettingsStore } from "./settings";
import { SETTINGS_DEFAULTS } from "../config/generated/defaults";
import { useTopicStore } from "./topicListManager";
import { useNotificationStore } from "./notification";
import { useShareStore, type ShareIdentity } from "./shareStore";
import { clearMessageCache } from "../utils/astRenderer";
import { extractMentionedMemberIds } from "../utils/mention";
import { triggerMessageHaptic } from "../utils/nativeHaptics";
import { allowChatMutation } from "../../features/chat/chatMutationPolicy";
import { useChatPreferencesStore } from "../../features/chat/preferences/store";
import { useBubbleNarration } from "../../features/chat/voice/useBubbleNarration";

import type {
  ChatMessage,
  ContentBlock,
  GroupChatResultDto,
  MessageWriteResultDto,
  RegenerateTopicResultDto,
  StreamEventDto,
  TopicActivityDto,
} from "../types/chat";
import type {
  ConversationKey,
} from "./chatSessionStore";

export { MAX_HISTORY_MESSAGES, type HistoryPageResult } from './chatWindow';

export interface MessageActionKey {
  conversation: ConversationKey;
  messageId: string;
}

interface EditingMessageIntent {
  key: MessageActionKey;
  initialContent: string;
}


const sameConversation = (a: ConversationKey | null, b: ConversationKey | null) =>
  Boolean(
    a &&
      b &&
      a.ownerId === b.ownerId &&
      a.ownerType === b.ownerType &&
      a.topicId === b.topicId &&
      a.epoch === b.epoch,
  );

export const useChatHistoryStore = defineStore("chatHistory", () => {
  const window = useChatWindow();
  const { currentChatHistory, hasEvictedNewer, canCommitConversation,
    projectLiveMessage, reconcileWindow, noteMessageChange, returnToLatest } = window;

  // “编辑重发”意图必须绑定完整会话身份，避免异步读取后误落到其他话题。
  const editingMessage = ref<EditingMessageIntent | null>(null);
  const isSubmitting = ref(false);


  const sessionStore = useChatSessionStore();
  watch(() => sessionStore.sessionEpoch, () => { editingMessage.value = null; }, { flush: 'sync' });
  const streamStore = useChatStreamStore();
  const attachmentStore = useAttachmentStore();
  const assistantStore = useAssistantStore();
  const settingsStore = useSettingsStore();
  const topicStore = useTopicStore();
  const notificationStore = useNotificationStore();
  const shareStore = useShareStore();
  const chatPreferences = useChatPreferencesStore();

  const errorMessage = (error: unknown) =>
    error instanceof Error ? error.message : String(error);
  const captureLoadedConversation = (): ConversationKey | null => {
    const key = sessionStore.currentConversationKey;
    return key && canCommitConversation(key) ? { ...key } : null;
  };

  const captureMessageActionKey = (messageId: string): MessageActionKey | null => {
    const conversation = captureLoadedConversation();
    return conversation ? { conversation, messageId } : null;
  };

  const isMessageActionCurrent = (key: MessageActionKey) =>
    canCommitConversation(key.conversation);

  // Reading another section of the same conversation must preserve its composer.
  const resetHistoryForConversation = () => {
    window.resetHistoryForConversation();
    editingMessage.value = null;
  };

  const openLoadedReference = async (id: string) => {
    const key = captureLoadedConversation();
    if (!key || !currentChatHistory.value.some(row => row.id === id)) return;
    const intent = sessionStore.beginNavigation();
    try {
      const result = await sessionStore.selectTopicById(key.ownerId, key.ownerType, key.topicId, intent, id);
      if (sessionStore.isNavigationCurrent(intent)) sessionStore.notifyNavigationResult(result);
    } catch (error) {
      if (sessionStore.isNavigationCurrent(intent)) notificationStore.addNotification({
        type: 'error', title: '无法打开引用消息', message: errorMessage(error), toastOnly: true,
      });
    }
  };

  /**
   * 尝试为话题生成 AI 总结标题
   * 触发条件：消息数 >= 4 且标题仍为初始的 "新话题 HH:MM:SS" 格式
   */
  const summarizeTopic = async () => {
    if (!sessionStore.currentTopicId || !sessionStore.currentSelectedItem?.id) return;

    const topicId = sessionStore.currentTopicId;
    const ownerId = sessionStore.currentSelectedItem.id;
    const ownerType = sessionStore.currentSelectedItem.type;

    const topic = topicStore.topics.find(
      (candidate) =>
        candidate.id === topicId &&
        candidate.ownerId === ownerId &&
        candidate.ownerType === ownerType,
    );
    const defaultTitle = topic?.name;
    const isDefaultName =
      defaultTitle && /^(新话题|新会话) \d{2}:\d{2}:\d{2}$/.test(defaultTitle);
    const messageCount = currentChatHistory.value.filter(
      (m) => m.role !== "system",
    ).length;

    if (isDefaultName && messageCount >= 4) {
      console.log(`[ChatHistoryStore] Triggering AI summary for topic: ${topicId}`);
      try {
        const agentName = ownerType === "agent"
          ? assistantStore.agents.find((a) => a.id === ownerId)?.name || "AI"
          : assistantStore.groups.find((g) => g.id === ownerId)?.name || "AI";
        const newTitle = await invoke<string>("summarize_topic", {
          ownerId,
          ownerType,
          topicId,
          agentName,
        });

        if (newTitle) {
          console.log(`[ChatHistoryStore] AI Summarized Title: ${newTitle}`);
          await topicStore.updateTopicTitle(
            ownerId,
            ownerType,
            topicId,
            newTitle,
            defaultTitle,
          );
        }
      } catch (e) {
        console.error("[ChatHistoryStore] AI Summary failed:", e);
      }
    }
  };


  /**
   * 构造群聊/单聊共用的流式事件 Channel 接线
   */
  const makeStreamChannel = (key: ConversationKey | null) => {
    const streamChannel = new Channel<StreamEventDto>();
    streamChannel.onmessage = (event) => {
      const target = key ?? captureLoadedConversation();
      const canCommitEvent = () => target !== null
        && event.context?.ownerId === target.ownerId
        && event.context?.ownerType === target.ownerType
        && event.context?.topicId === target.topicId
        && canCommitConversation(target);
      return streamStore.processStreamEvent(event, {
        onMessageCreated: (msg) => {
          if (target && canCommitEvent()) projectLiveMessage(target, msg);
        },
        onStreamFinished: () => {
          if (canCommitEvent()) {
            summarizeTopic();
          }
        },
      });
    };
    return streamChannel;
  };

  /**
   * 同一次邀约由 Rust 的同一个群轮 owner 顺序执行，停止会覆盖剩余成员。
   */
  const inviteGroupMembers = async (agentIds: string[], frozenKey?: ConversationKey) => {
    const key = frozenKey || captureLoadedConversation();
    if (!key || key.ownerType !== "group" || agentIds.length === 0) return;
    const releaseOperation = streamStore.beginConversationOperation(key.ownerId, key.ownerType, key.topicId);
    try {
      const settings = settingsStore.settings;
      if (!settings) throw new Error("应用尚未完成初始化");

      const streamChannel = makeStreamChannel(key);
      await invoke("invite_group_member_to_speak", {
        payload: {
          groupId: key.ownerId,
          topicId: key.topicId,
          agentIds,
        },
        streamChannel,
      });
    } catch (e) {
      console.error("[ChatHistoryStore] Invite failed:", e);
      notificationStore.addNotification({
        type: "error",
        title: "邀请发言失败",
        message: e instanceof Error ? e.message : String(e),
        toastOnly: true,
        duration: 6000,
      });
    } finally {
      releaseOperation();
    }
  };

  const inviteGroupMember = (agentId: string, frozenKey?: ConversationKey) =>
    inviteGroupMembers([agentId], frozenKey);

  /** 只提交用户意图；会话、仲裁和后续成员始终由 Rust 群轮 owner 执行。 */
  const controlGroupSession = async (action: "start" | "continue") => {
    const key = captureLoadedConversation();
    if (!key || key.ownerType !== "group") return;
    const releaseOperation = streamStore.beginConversationOperation(key.ownerId, "group", key.topicId);
    try {
      await invoke<GroupChatResultDto>("control_group_session", {
        payload: { groupId: key.ownerId, topicId: key.topicId, action },
        streamChannel: makeStreamChannel(key),
      });
    } catch (error) {
      notificationStore.addNotification({
        type: "error",
        title: action === "start" ? "发起群聊失败" : "继续群聊失败",
        message: errorMessage(error),
        toastOnly: true,
        duration: 6000,
      });
    } finally {
      releaseOperation();
    }
  };

  /**
   * 触发 AI 生成逻辑
   */
  const triggerGeneration = async (
    userMsg: ChatMessage,
    frozenKey?: ConversationKey,
    shareIdentity?: ShareIdentity,
    commit: { persisted?: MessageWriteResultDto; onCommitted?: () => void } = {},
  ): Promise<number | null> => {
    const key = frozenKey || captureLoadedConversation();
    if (!key) return null;
    const releaseOperation = streamStore.beginConversationOperation(key.ownerId, key.ownerType, key.topicId);
    let topicUpdatedAt: number;
    let alreadySent = false;
    try {
      const shareResult = shareIdentity ? await shareStore.commit(shareIdentity, userMsg) : null;
      if (shareResult) {
        Object.assign(userMsg, shareResult.message);
        alreadySent = shareResult.alreadySent;
      }
      const persisted = commit.persisted ?? shareResult?.persisted ?? await invoke<MessageWriteResultDto>("append_single_message", {
        ownerId: key.ownerId,
        ownerType: key.ownerType,
        topicId: key.topicId,
        message: {
          ...userMsg,
          blocks: undefined, // 强行设为 undefined，迫使后端执行真正的编译，生成 markdown AST 节点与表情包匹配
        },
      });

      projectLiveMessage(key, { ...userMsg, blocks: persisted.blocks });
      topicUpdatedAt = persisted.topicUpdatedAt;
      topicStore.setTopicUpdatedAt(
        key.ownerId,
        key.ownerType,
        key.topicId,
        topicUpdatedAt,
      );
    } catch (e) {
      console.error("[ChatHistoryStore] Message persistence failed:", e);
      notificationStore.addNotification({
        type: "error",
        title: "消息保存失败",
        message: e instanceof Error ? e.message : String(e),
        toastOnly: true,
        duration: 6000,
      });
      releaseOperation();
      return null;
    }

    // This receipt belongs to persistence, independently of the model request.
    commit.onCommitted?.();
    if (alreadySent) {
      releaseOperation();
      return topicUpdatedAt;
    }
    try {
      const settings = settingsStore.settings;
      if (!settings) throw new Error("应用尚未完成初始化");

      const streamChannel = makeStreamChannel(key);

      if (key.ownerType === "group") {
        const result = await invoke<GroupChatResultDto>("handle_group_chat_message", {
          payload: {
            groupId: key.ownerId,
            topicId: key.topicId,
            userMessage: userMsg,
          },
          streamChannel
        });
        // 群组回合可能合法地"无人发言"（如未实现的发言模式），必须显式可见，
        // 否则用户看到的是"发送后没有任何反应"
        if (result?.status === "no_ai_response") {
          if (result.reason === "invite_only") {
            // 提及即邀约：整批成员按出现顺序交给同一群轮，停止不会另启后续邀约；
            // 未提及任何成员时引导用户使用邀约横条
            const group = assistantStore.groups.find((g) => g.id === key.ownerId);
            const mentionedIds = group
              ? extractMentionedMemberIds(
                  userMsg.content ?? "",
                  group.members.map((id) => ({
                    id,
                    name: assistantStore.agents.find((a) => a.id === id)?.name ?? "",
                  })),
                )
              : [];
            if (mentionedIds.length > 0) {
              await inviteGroupMembers(mentionedIds, key);
            } else {
              notificationStore.addNotification({
                type: "info",
                title: "群组未产生回复",
                message: "邀请发言模式下成员不会自动回复，可点击上方成员头像邀约，或在消息中 @ 成员后发送。",
                toastOnly: true,
                duration: 6000,
              });
            }
          } else {
            notificationStore.addNotification({
              type: "info",
              title: "群组未产生回复",
              message: "没有成员满足发言条件，可尝试 @提及 成员或调整发言模式。",
              toastOnly: true,
              duration: 6000,
            });
          }
        }
      } else {
        await invoke("handle_agent_chat_message", { 
          payload: {
            agentId: key.ownerId,
            topicId: key.topicId,
            userMessage: userMsg,
          }, 
          streamChannel 
        });
      }
    } catch (e) {
      console.error("[ChatHistoryStore] Generation failed:", e);
      notificationStore.addNotification({
        type: "error",
        title: "消息生成失败",
        message: e instanceof Error ? e.message : String(e),
        toastOnly: true,
        duration: 6000,
      });
    } finally {
      releaseOperation();
    }
    return topicUpdatedAt;
  };

  /**
   * 发送消息
   */
  const sendMessage = async (content: string, onCommitted?: () => void) => {
    if (isSubmitting.value) return;
    const key = captureLoadedConversation();
    if (!key || (!content.trim() && attachmentStore.stagedAttachments.length === 0)) return;
    if (attachmentStore.stagedAttachments.some(attachment => attachment.status !== "done")) return;
    const intentAtSend = editingMessage.value;
    const stagedAtSend = [...attachmentStore.stagedAttachments];
    const currentStaged = stagedAtSend.map((attachment, attachmentOrder) => ({ ...attachment, attachmentOrder }));
    let awaitingCommit = true;
    const committed = () => {
      awaitingCommit = false;
      isSubmitting.value = false;
      onCommitted?.();
    };
    isSubmitting.value = true;
    try {
      const shareAtSend = shareStore.activeDraft ? {
        generation: shareStore.activeDraft.generation,
        deliveryId: shareStore.activeDraft.deliveryId,
      } : undefined;
      if ((window.anchored.value || window.pinned.value || hasEvictedNewer.value) && !editingMessage.value) {
        const latest = await returnToLatest();
        if (latest.error || latest.aborted || window.anchored.value || window.pinned.value || hasEvictedNewer.value) return;
      }
      if (!canCommitConversation(key) || editingMessage.value !== intentAtSend
        || shareStore.activeDraft?.deliveryId !== shareAtSend?.deliveryId) return;

      if (editingMessage.value) {
        const intent = editingMessage.value;
        if (!sameConversation(intent.key.conversation, key)) return;
        if (!allowChatMutation(key)) return;
        const originalId = intent.key.messageId;
        const targetIndex = currentChatHistory.value.findIndex(m => m.id === originalId);
        if (targetIndex === -1) return;
        if (chatPreferences.messageHapticsEnabled) triggerMessageHaptic('send');
        const targetMsg = {
          ...currentChatHistory.value[targetIndex],
          content,
          attachments: currentStaged.length > 0 ? currentStaged : currentChatHistory.value[targetIndex].attachments,
          blocks: [{ type: "markdown" as const, content }],
        };
        const releaseOperation = streamStore.beginConversationOperation(key.ownerId, key.ownerType, key.topicId);
        try {
          await useBubbleNarration().stopMessage(intent.key);
          if (!isMessageActionCurrent(intent.key)) return;
          const persisted = await invoke<MessageWriteResultDto & { msgCount: number }>("edit_resend_message", {
            ownerId: key.ownerId,
            ownerType: key.ownerType,
            topicId: key.topicId,
            message: { ...targetMsg, blocks: undefined },
          });
          topicStore.setTopicMsgCount(
            key.ownerId,
            key.ownerType,
            key.topicId,
            persisted.msgCount,
          );
          topicStore.setTopicUpdatedAt(
            key.ownerId,
            key.ownerType,
            key.topicId,
            persisted.topicUpdatedAt,
          );
          if (canCommitConversation(key)) {
            const currentIndex = currentChatHistory.value.findIndex(message => message.id === originalId);
            if (currentIndex !== -1) {
              currentChatHistory.value[currentIndex] = { ...targetMsg, blocks: persisted.blocks };
              currentChatHistory.value = currentChatHistory.value.slice(0, currentIndex + 1);
            }
          }
          if (editingMessage.value === intent) editingMessage.value = null;
          await triggerGeneration(targetMsg, key, undefined, {
            persisted,
            onCommitted: () => {
              if (currentStaged.length > 0) {
                attachmentStore.stagedAttachments = attachmentStore.stagedAttachments.filter(item =>
                  !stagedAtSend.some(sent => sent.id ? sent.id === item.id : sent === item),
                );
              }
              committed();
            },
          });
        } catch (e) {
          notificationStore.addNotification({
            type: "error",
            title: "编辑重发失败",
            message: errorMessage(e),
            toastOnly: true,
            duration: 6000,
          });
        } finally {
          releaseOperation();
        }
        return;
      }

      const shareIdentity = shareAtSend;
      if (shareIdentity && shareStore.sending) return;
      if (chatPreferences.messageHapticsEnabled) triggerMessageHaptic('send');
      if (shareIdentity) shareStore.sending = true;

      const now = Date.now();
      const userName = settingsStore.settings?.identity.userName ?? SETTINGS_DEFAULTS.identity.userName;
      const userMsg: ChatMessage = {
        id: shareIdentity ? `msg_share_${shareIdentity.deliveryId}` : `msg_${now}_user_${Math.random().toString(36).substring(2, 9)}`,
        role: "user",
        name: userName,
        content,
        timestamp: now,
        attachments: currentStaged.length > 0 ? currentStaged : undefined,
        shell: streamStore.computeShell({ role: "user", name: userName }),
        blocks: [{ type: "markdown" as const, content }],
      };

      if (canCommitConversation(key)) {
        projectLiveMessage(key, userMsg);
      }
      topicStore.incrementTopicMsgCount(key.ownerId, key.ownerType, key.topicId);
      const topicUpdatedAt = await triggerGeneration(userMsg, key, shareIdentity, {
        onCommitted: () => {
          if (!shareIdentity) {
            attachmentStore.stagedAttachments = attachmentStore.stagedAttachments.filter(item =>
              !stagedAtSend.some(sent => sent.id ? sent.id === item.id : sent === item),
            );
          }
          committed();
        },
      });
      if (topicUpdatedAt === null) {
        if (canCommitConversation(key)) {
          currentChatHistory.value = currentChatHistory.value.filter(
            message => message.id !== userMsg.id,
          );
        }
        topicStore.decrementTopicMsgCount(key.ownerId, key.ownerType, key.topicId);
      }
    } finally {
      // A previous model call can finish after the next submission has started.
      if (awaitingCommit) isSubmitting.value = false;
    }
  };

  /** Project an already committed Rust message without touching the composer. */
  const mergePersistedVoiceMessage = (key: ConversationKey, message: ChatMessage, topicUpdatedAt: number) => {
    topicStore.setTopicUpdatedAt(key.ownerId, key.ownerType, key.topicId, topicUpdatedAt);
    const hydrated = { ...message, shell: streamStore.computeShell({
      role: message.role, agentId: message.agentId, name: message.name ?? undefined,
    }) };
    projectLiveMessage(key, hydrated);
  };

  /**
   * 删除消息
   */
  const deleteMessage = async (messageKey: MessageActionKey) => {
    const key = messageKey.conversation;
    const messageId = messageKey.messageId;
    if (!isMessageActionCurrent(messageKey)) return;
    if (!allowChatMutation(key)) return;

    const targetIndex = currentChatHistory.value.findIndex(m => m.id === messageId);
    if (targetIndex === -1) return;

    try {
      await useBubbleNarration().stopMessage(messageKey);
      if (!isMessageActionCurrent(messageKey)) return;
      const activity = await invoke<TopicActivityDto>("delete_messages", {
        ownerId: key.ownerId,
        ownerType: key.ownerType,
        topicId: key.topicId,
        msgIds: [messageId],
      });
      noteMessageChange(key, messageId);
      streamStore.retireStreamProjection(key.ownerId, key.ownerType, key.topicId, messageId);
      if (canCommitConversation(key)) {
        const currentIndex = currentChatHistory.value.findIndex(message => message.id === messageId);
        if (currentIndex !== -1) currentChatHistory.value.splice(currentIndex, 1);
      }
      topicStore.setTopicMsgCount(
        key.ownerId,
        key.ownerType,
        key.topicId,
        activity.msgCount,
      );
      topicStore.setTopicUpdatedAt(
        key.ownerId,
        key.ownerType,
        key.topicId,
        activity.updatedAt,
      );
      if (canCommitConversation(key) && !window.isLoadingHistory.value && sessionStore.acceptedWindow?.anchorId === messageId) {
        await refreshAfterMutation();
      }
    } catch (e) {
      notificationStore.addNotification({
        type: "error",
        title: "删除消息失败",
        message: errorMessage(e),
        toastOnly: true,
        duration: 6000,
      });
    }
  };

  const deleteAttachment = async (
    messageKey: MessageActionKey,
    attachmentOrder: number,
    hash: string,
  ) => {
    const key = messageKey.conversation;
    const messageId = messageKey.messageId;
    if (!isMessageActionCurrent(messageKey)) return;
    // 1. 调用后端逻辑删除命令
    const activity = await invoke<TopicActivityDto>("delete_message_attachment", {
      ownerId: key.ownerId,
      ownerType: key.ownerType,
      topicId: key.topicId,
      messageId,
      attachmentOrder,
      hash,
    });
    noteMessageChange(key, messageId);
    topicStore.setTopicUpdatedAt(
      key.ownerId,
      key.ownerType,
      key.topicId,
      activity.updatedAt,
    );

    // 2. 更新本地状态，以便在界面上实时隐藏该附件
    if (!canCommitConversation(key)) return;
    const targetIndex = currentChatHistory.value.findIndex(m => m.id === messageId);
    if (targetIndex !== -1) {
      const msg = currentChatHistory.value[targetIndex];
      if (msg.attachments) {
        msg.attachments = msg.attachments.filter((att, index) =>
          (att.attachmentOrder ?? index) !== attachmentOrder || att.hash !== hash
        );
      }
    }
  };

  const updateMessageContent = async (messageKey: MessageActionKey, newContent: string) => {
    const key = messageKey.conversation;
    const messageId = messageKey.messageId;
    if (!isMessageActionCurrent(messageKey)) return;
    const targetIndex = currentChatHistory.value.findIndex(m => m.id === messageId);
    if (targetIndex === -1) return;

    const msg = { ...currentChatHistory.value[targetIndex] };

    try {
      await useBubbleNarration().stopMessage(messageKey);
      if (!isMessageActionCurrent(messageKey)) return;
      const persisted = await invoke<MessageWriteResultDto>("patch_single_message", {
        ownerId: key.ownerId,
        ownerType: key.ownerType,
        topicId: key.topicId,
        message: {
          ...msg,
          content: newContent,
          blocks: undefined,
        },
      });
      noteMessageChange(key, messageId);
      topicStore.setTopicUpdatedAt(
        key.ownerId,
        key.ownerType,
        key.topicId,
        persisted.topicUpdatedAt,
      );
      if (canCommitConversation(key)) {
        clearMessageCache(messageId);
        const currentIndex = currentChatHistory.value.findIndex(message => message.id === messageId);
        if (currentIndex !== -1) {
          currentChatHistory.value[currentIndex] = {
            ...currentChatHistory.value[currentIndex],
            content: newContent,
            blocks: persisted.blocks,
          };
        }
      }
    } catch (e) {
      console.error("[updateMessageContent] patch_single_message failed:", e);
      notificationStore.addNotification({
        type: "error",
        title: "保存消息失败",
        message: errorMessage(e),
        toastOnly: true,
        duration: 6000,
      });
      throw e;
    }
  };

  const regenerateResponse = async (messageKey: MessageActionKey) => {
    const key = messageKey.conversation;
    const targetMessageId = messageKey.messageId;
    if (!isMessageActionCurrent(messageKey)) return;
    if (!allowChatMutation(key)) return;
    const targetIndex = currentChatHistory.value.findIndex(m => m.id === targetMessageId);
    if (targetIndex === -1) return;
    const releaseOperation = streamStore.beginConversationOperation(key.ownerId, key.ownerType, key.topicId);

    // 当前窗口仅负责乐观更新；权威用户锚点由后端从完整历史中寻找。
    let lastUserMsgIndex = targetIndex - 1;
    while (lastUserMsgIndex >= 0 && currentChatHistory.value[lastUserMsgIndex].role !== "user") {
      lastUserMsgIndex--;
    }
    const didOptimisticTruncate = lastUserMsgIndex >= 0;
    const narration = useBubbleNarration();
    const narratedKey = narration.activeMessageKey;
    if (narratedKey && sameConversation(narratedKey.conversation, key)
      && currentChatHistory.value.slice(lastUserMsgIndex + 1).some(message => message.id === narratedKey.messageId)) {
      try {
        await narration.stopMessage(narratedKey);
      } catch (error) {
        releaseOperation();
        notificationStore.addNotification({ type: 'error', title: '停止朗读失败', message: errorMessage(error), toastOnly: true });
        return;
      }
      if (!isMessageActionCurrent(messageKey)) { releaseOperation(); return; }
    }
    if (didOptimisticTruncate) {
      const countToDelete = currentChatHistory.value.length - (lastUserMsgIndex + 1);
      currentChatHistory.value = currentChatHistory.value.slice(0, lastUserMsgIndex + 1);
      topicStore.decrementTopicMsgCount(key.ownerId, key.ownerType, key.topicId, countToDelete);
    }

    try {
      const streamChannel = makeStreamChannel(key);

      const result = await invoke<RegenerateTopicResultDto>("regenerate_topic_response", {
        ownerId: key.ownerId,
        ownerType: key.ownerType,
        topicId: key.topicId,
        targetResponseMsgId: targetMessageId,
        streamChannel
      });
      topicStore.setTopicMsgCount(key.ownerId, key.ownerType, key.topicId, result.msgCount);
    } catch (e) {
      console.error("[ChatHistoryStore] Regeneration failed:", e);
      notificationStore.addNotification({
        type: "error",
        title: "重新生成失败",
        message: e instanceof Error ? e.message : String(e),
        toastOnly: true,
        duration: 6000,
      });
    } finally {
      releaseOperation();
      if (canCommitConversation(key)) await refreshAfterMutation();
    }
  };

  // A failed refresh is separate from the result of the SQLite mutation.
  const refreshAfterMutation = async () => {
    try { await reconcileWindow(true); }
    catch (error) {
      notificationStore.addNotification({ type: 'error', title: '聊天记录核对失败', message: errorMessage(error), toastOnly: true });
    }
  };


  const fetchRawContent = async (messageKey: MessageActionKey): Promise<string> => {
    const key = messageKey.conversation;
    const messageId = messageKey.messageId;
    if (!isMessageActionCurrent(messageKey)) return "";
    const existingMsg = currentChatHistory.value.find(m => m.id === messageId);
    if (existingMsg && existingMsg.content) return existingMsg.content;
    try {
      const content = await invoke<string>('fetch_raw_message_content', {
        ownerId: key.ownerId,
        ownerType: key.ownerType,
        topicId: key.topicId,
        messageId,
      });
      if (canCommitConversation(key)) {
        const current = currentChatHistory.value.find(message => message.id === messageId);
        if (current) current.content = content;
      }
      return content;
    } catch (e) {
      console.error(`[ChatHistoryStore] Failed to fetch raw content for message ${messageId}:`, e);
      return "";
    }
  };

  const beginEditResend = async (messageKey: MessageActionKey) => {
    if (!allowChatMutation(messageKey.conversation)) return;
    const initialContent = await fetchRawContent(messageKey);
    if (!isMessageActionCurrent(messageKey)) return;
    if (!allowChatMutation(messageKey.conversation)) return;
    editingMessage.value = {
      key: messageKey,
      initialContent,
    };
  };

  const cancelEditResend = () => {
    editingMessage.value = null;
  };

  const reRenderMessage = async (messageKey: MessageActionKey) => {
    const key = messageKey.conversation;
    const messageId = messageKey.messageId;
    if (!isMessageActionCurrent(messageKey)) throw new Error("消息不属于当前会话");
    const targetIndex = currentChatHistory.value.findIndex(m => m.id === messageId);
    if (targetIndex === -1) {
      throw new Error("消息未在当前历史记录中找到");
    }

    clearMessageCache(messageId);

    try {
      const compiledBlocks = await invoke<ContentBlock[]>("re_render_message", {
        ownerId: key.ownerId,
        ownerType: key.ownerType,
        messageId,
        topicId: key.topicId,
      });
      if (!canCommitConversation(key)) return;
      clearMessageCache(messageId);
      const currentIndex = currentChatHistory.value.findIndex(message => message.id === messageId);
      if (currentIndex !== -1) {
        currentChatHistory.value[currentIndex] = {
          ...currentChatHistory.value[currentIndex],
          blocks: compiledBlocks,
        };
      }
    } catch (e) {
      console.error("[reRenderMessage] re_render_message failed:", e);
      throw e;
    }
  };

  return {
    ...window,
    loading: window.isLoadingHistory,
    resetHistoryForConversation,
    openLoadedReference,
    editingMessage,
    sendMessage,
    isSubmitting,
    controlGroupSession,
    mergePersistedVoiceMessage,
    deleteMessage,
    deleteAttachment,
    triggerGeneration,
    makeStreamChannel,
    inviteGroupMember,
    summarizeTopic,
    captureMessageActionKey,
    isMessageActionCurrent,
    updateMessageContent,
    beginEditResend,
    cancelEditResend,
    regenerateResponse,
    fetchRawContent,
    reRenderMessage,
  };
});

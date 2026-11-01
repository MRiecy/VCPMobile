import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { useChatHistoryStore } from '../../../core/stores/chatHistoryStore';
import { useChatSessionStore, type ConversationKey } from '../../../core/stores/chatSessionStore';
import { useTopicStore } from '../../../core/stores/topicListManager';
import type { ChatMessage } from '../../../core/types/chat';
import type { TopicDto } from '../../../core/types/assistant';
import { useVoiceCallStore } from './voiceCallStore';
import type { VoiceCallTarget } from './voiceConversation';

interface PersistedEvent {
  callId: string;
  target: VoiceCallTarget;
  revision: number;
  message: ChatMessage;
  topicUpdatedAt: number;
}

/** Reconcile committed messages only; this consumer never submits or narrates. */
export function createVoiceHistorySync() {
  const session = useChatSessionStore();
  const history = useChatHistoryStore();
  const topics = useTopicStore();
  const voice = useVoiceCallStore();
  let disposed = false;
  let unlisten: UnlistenFn | undefined;
  let activityRevision = 0;
  let activityTask: Promise<void> | null = null;
  let latestTarget: VoiceCallTarget | null = null;

  const currentKey = (target: VoiceCallTarget): ConversationKey | null => {
    const key = session.currentConversationKey;
    return key && key.ownerId === target.ownerId && key.ownerType === target.ownerType
      && key.topicId === target.topicId ? { ...key } : null;
  };

  const refreshActivity = (target: VoiceCallTarget) => {
    latestTarget = target;
    activityRevision += 1;
    if (activityTask) return activityTask;
    activityTask = (async () => {
      let completed = 0;
      while (!disposed && completed !== activityRevision) {
        completed = activityRevision;
        const target = latestTarget;
        const key = target && currentKey(target);
        if (!key) continue;
        const rows = await invoke<TopicDto[]>('get_topics', {
          ownerId: key.ownerId, ownerType: key.ownerType,
        });
        if (disposed || !session.isConversationCurrent(key)) continue;
        // A newer commit while the query ran requires a fresh authoritative read.
        if (completed !== activityRevision) continue;
        const topic = rows.find(item => item.id === key.topicId);
        if (topic) {
          topics.setTopicMsgCount(key.ownerId, key.ownerType, key.topicId, topic.msgCount ?? 0);
          topics.setTopicUpdatedAt(key.ownerId, key.ownerType, key.topicId, topic.updatedAt ?? topic.createdAt);
        }
      }
    })().catch(error => console.warn('[VoiceHistory] Activity reconciliation failed:', error))
      .finally(() => { activityTask = null; });
    return activityTask;
  };

  const ready = listen<PersistedEvent>('vcp-voice-message-persisted', ({ payload }) => {
    if (disposed) return;
    const key = currentKey(payload.target);
    if (!key) return;
    history.mergePersistedVoiceMessage(key, payload.message, payload.topicUpdatedAt);
    void refreshActivity(payload.target);
  }).then(stop => {
    if (disposed) stop();
    else unlisten = stop;
  }).catch(error => console.warn('[VoiceHistory] Message listener unavailable:', error));

  const reconcile = async () => {
    await ready;
    if (disposed) return;
    // History recovery is shared with text and group messages, after output reconnects.
    await voice.recover();
  };

  const dispose = () => {
    disposed = true;
    unlisten?.();
    unlisten = undefined;
  };
  return { reconcile, dispose };
}

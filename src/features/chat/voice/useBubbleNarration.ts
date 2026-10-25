import { defineStore } from 'pinia';
import { computed, onScopeDispose, shallowRef, watch } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { MessageActionKey } from '../../../core/stores/chatHistoryStore';
import { useChatSessionStore, type ConversationKey } from '../../../core/stores/chatSessionStore';
import { useNotificationStore } from '../../../core/stores/notification';
import { useVoiceCallStore } from './voiceCallStore';

interface NarrationTask {
  key: MessageActionKey;
  id: string;
  submitted: boolean;
  notified: boolean;
}

function sameConversation(a: ConversationKey, b: ConversationKey) {
  return a.ownerId === b.ownerId && a.ownerType === b.ownerType
    && a.topicId === b.topicId && a.epoch === b.epoch;
}

function sameMessage(a: MessageActionKey, b: MessageActionKey) {
  return a.messageId === b.messageId && sameConversation(a.conversation, b.conversation);
}

/** One application-scoped owner; ordinary message component disposal has no playback semantics. */
export const useBubbleNarration = defineStore('bubbleNarration', () => {
  const session = useChatSessionStore();
  const voiceCall = useVoiceCallStore();
  const notifications = useNotificationStore();
  const activeTask = shallowRef<NarrationTask | null>(null);
  let ownedTask: NarrationTask | null = null;
  let generation = 0;
  let disposed = false;
  let pending = Promise.resolve();
  let listening: Promise<void> | null = null;
  let unlisten: UnlistenFn | undefined;

  const notify = (message: string, task?: NarrationTask) => {
    if (disposed || task?.notified) return;
    if (task) task.notified = true;
    notifications.addNotification({ type: 'error', title: '气泡朗读', message, toastOnly: true });
  };
  const enqueue = (operation: () => Promise<void>) => {
    const result = pending.then(operation);
    pending = result.catch(() => undefined);
    return result;
  };
  const release = async () => {
    const task = ownedTask;
    if (!task) return;
    // Even a terminal event may precede registry/native-slot release. Only the
    // command acknowledgement allows the next task to enter that slot.
    if (task.submitted) await invoke('stop_tts', { ttsId: task.id });
    if (ownedTask === task) ownedTask = null;
    if (activeTask.value === task) activeTask.value = null;
  };
  const ensureListening = () => {
    if (!listening) {
      listening = listen<{ ttsId: string; kind: string; message?: string }>('vcp-tts', event => {
        const task = ownedTask;
        if (!task || task.id !== event.payload.ttsId) return;
        if (event.payload.kind === 'error' || event.payload.kind === 'stopped') {
          if (activeTask.value === task) {
            if (event.payload.kind === 'error') notify(event.payload.message || '朗读失败，请重试。', task);
            activeTask.value = null;
          }
        }
      }).then(off => {
        if (disposed) off();
        else unlisten = off;
      }).catch(error => { listening = null; throw error; });
    }
    return listening;
  };

  const stopBubbleNarration = () => {
    generation += 1;
    activeTask.value = null;
    return enqueue(release);
  };
  const isNarrating = (key: MessageActionKey, ttsId?: string | null) => !!activeTask.value
    && sameMessage(activeTask.value.key, key) && (!ttsId || activeTask.value.id === ttsId);
  const stopMessage = (key: MessageActionKey, ttsId?: string | null) => {
    const task = activeTask.value ?? ownedTask;
    return task && sameMessage(task.key, key) && (!ttsId || task.id === ttsId)
      ? stopBubbleNarration() : Promise.resolve();
  };

  const startBubbleNarration = (
    key: MessageActionKey,
    text: string | (() => Promise<string>),
    isSourceCurrent: () => boolean = () => true,
  ) => {
    if (disposed || !session.isConversationCurrent(key.conversation) || !isSourceCurrent()) return Promise.resolve();
    if (voiceCall.active) {
      notify('语音通话进行中，请先结束通话再单独朗读气泡。');
      return Promise.resolve();
    }
    const intent = ++generation;
    const task: NarrationTask = {
      key: { messageId: key.messageId, conversation: { ...key.conversation } },
      id: `bubble-tts-${crypto.randomUUID()}`, submitted: false, notified: false,
    };
    // Reserve the intent before fetching raw text: a slow A read cannot replace B.
    activeTask.value = task;
    const current = () => !disposed && generation === intent && !voiceCall.active
      && session.isConversationCurrent(task.key.conversation) && isSourceCurrent();
    return enqueue(async () => {
      try {
        await release();
        if (!current()) return;
        const rawText = typeof text === 'string' ? text : await text();
        if (!current()) return;
        if (!rawText.trim()) { notify('此消息没有可朗读的正文内容。', task); return; }
        await ensureListening();
        if (!current()) return;
        ownedTask = task;
        task.submitted = true;
        const result = await invoke<{ configured: boolean }>('start_tts_session', {
          ttsId: task.id, originUserMessageId: null,
          bubbleSource: { topic: {
            ownerId: key.conversation.ownerId, ownerType: key.conversation.ownerType, topicId: key.conversation.topicId,
          }, messageId: key.messageId },
        });
        if (!current()) { await release(); return; }
        if (!result.configured) {
          notify('请先在语音设置中配置语音 API Key。', task);
          await release();
          return;
        }
        await invoke('submit_tts_text', { ttsId: task.id, text: rawText, finished: true });
        if (!current()) await release();
      } catch (error) {
        if (current()) notify(error instanceof Error ? error.message : String(error), task);
        // Preserve the resource owner if stop fails; subsequent admission must retry it.
        if (ownedTask === task) await release();
      } finally {
        if (activeTask.value === task && (ownedTask !== task || !current())) activeTask.value = null;
      }
    });
  };

  watch(() => session.currentConversationKey, () => {
    const task = activeTask.value ?? ownedTask;
    if (task && !session.isConversationCurrent(task.key.conversation)) {
      void stopBubbleNarration().catch(() => notify('停止朗读失败，请重试。'));
    }
  }, { flush: 'sync' });
  watch(() => voiceCall.active, active => {
    if (active) void stopBubbleNarration().catch(() => notify('停止朗读失败，请重试。'));
  }, { flush: 'sync' });
  onScopeDispose(() => {
    disposed = true;
    unlisten?.();
    void stopBubbleNarration().catch(() => undefined);
  });

  return {
    activeMessageKey: computed(() => activeTask.value?.key ?? null),
    activeMessageId: computed(() => activeTask.value?.key.messageId ?? null),
    activeTtsId: computed(() => activeTask.value?.id ?? null),
    isNarrating, startBubbleNarration, stopBubbleNarration, stopMessage,
  };
});

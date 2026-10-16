import { computed, ref } from 'vue';
import { defineStore } from 'pinia';
import { invoke } from '@tauri-apps/api/core';
import { useChatSessionStore } from './chatSessionStore';
import { useAssistantStore } from './assistant';
import { useTopicStore } from './topicListManager';
import { useNotificationStore } from './notification';
import type { Attachment, ChatMessage, MessageWriteResultDto } from '../types/chat';

export interface ShareIdentity { generation: number; deliveryId: string }
export interface CurrentShare extends ShareIdentity {
  state: 'receiving' | 'ready' | 'prepared' | 'draft' | 'sent' | 'discarded' | 'failed';
  error?: string;
  text: string;
  items: Array<{ itemId: string; fileName: string; sha256?: string; error?: string; attachment?: Attachment }>;
  attachments: Attachment[];
  agentId?: string;
  topicId?: string;
  revision: number;
}
export interface ShareCommit {
  message: ChatMessage;
  persisted: MessageWriteResultDto;
  alreadySent: boolean;
}

export const useShareStore = defineStore('share', () => {
  const current = ref<CurrentShare | null>(null);
  const selectorOpen = ref(false);
  const busy = ref(false);
  const sending = ref(false);
  const session = useChatSessionStore();
  const assistant = useAssistantStore();
  let readInFlight: Promise<void> | null = null;
  let readAgain = false;
  let preparingId: string | null = null;
  let androidQueue = Promise.resolve();
  let pendingEdit: { identity: ShareIdentity; text: string; attachments: Attachment[] } | null = null;
  let writing: Promise<void> | null = null;
  let saveTimer: ReturnType<typeof setTimeout> | null = null;
  const activeDraft = computed(() => current.value?.state === 'draft'
    && current.value.topicId === session.currentTopicId
    && current.value.agentId === session.currentSelectedItem?.id ? current.value : null);
  const failures = computed(() => current.value?.items.filter(item => item.error) ?? []);
  const identity = (value: ShareIdentity): ShareIdentity => ({ generation: value.generation, deliveryId: value.deliveryId });
  const isCurrent = (value: ShareIdentity) => current.value?.generation === value.generation && current.value?.deliveryId === value.deliveryId;
  const notify = (error: unknown) => useNotificationStore().addNotification({
    type: 'error', title: '分享处理失败', message: error instanceof Error ? error.message : String(error), toastOnly: false,
  });
  const apply = (value: CurrentShare | null) => {
    if (value && current.value && value.generation < current.value.generation) return;
    if (value && current.value?.deliveryId === value.deliveryId && value.revision < current.value.revision) return;
    const changed = value?.deliveryId !== current.value?.deliveryId;
    if (changed) { pendingEdit = null; selectorOpen.value = false; }
    current.value = value;
    if (value && !['sent', 'discarded', 'receiving'].includes(value.state) && changed) selectorOpen.value = true;
  };
  const prepare = async () => {
    const share = current.value;
    if (!share || !['ready', 'prepared', 'draft'].includes(share.state) || preparingId === share.deliveryId) return;
    preparingId = share.deliveryId;
    busy.value = true;
    try {
      const result = await invoke<CurrentShare>('prepare_current_share', { ...identity(share) });
      if (isCurrent(share)) { apply(result); if (!activeDraft.value) selectorOpen.value = true; }
    } catch (error) { if (isCurrent(share)) throw error; }
    finally { if (preparingId === share.deliveryId) { preparingId = null; busy.value = false; } }
  };
  const refresh = (): Promise<void> => {
    if (readInFlight) { readAgain = true; return readInFlight; }
    readInFlight = (async () => {
      const value = await invoke<CurrentShare | null>('read_current_share');
      const previous = current.value;
      // A local edit waiting for its CAS revision must not be overwritten by an older read.
      if (value && previous?.deliveryId === value.deliveryId && value.revision < previous.revision) return;
      apply(value);
      if (value && ['ready', 'draft'].includes(value.state)) void prepare().catch(notify);
      else if (value && ['draft', 'prepared', 'failed'].includes(value.state) && !activeDraft.value) selectorOpen.value = true;
    })().finally(() => {
      readInFlight = null;
      if (readAgain) { readAgain = false; void refresh().catch(notify); }
    });
    return readInFlight;
  };
  const receiveAndroid = (detail: unknown) => {
    if (!detail || typeof detail !== 'object') return;
    const source = detail as Record<string, unknown>;
    if (typeof source.intentId !== 'string' || !source.intentId) return;
    // Preserve native acceptance order even when IPC scheduling is concurrent.
    androidQueue = androidQueue.catch(() => {}).then(async () => {
      const value = await invoke<CurrentShare>('accept_android_share', {
        deliveryId: source.intentId, text: typeof source.text === 'string' ? source.text : '',
        error: typeof source.error === 'string' ? source.error : null,
      });
      apply(value);
      if (source.error) selectorOpen.value = true;
      if (source.state === 'receiving' || source.error || ['sent', 'draft', 'discarded'].includes(value.state)) return;
      // Ready processing is detached from acceptance; a later share can immediately invalidate it.
      void invoke<CurrentShare>('finish_android_share', {
        ...identity(value), text: typeof source.text === 'string' ? source.text : '', files: Array.isArray(source.files) ? source.files : [],
      }).then(async result => { if (isCurrent(value)) { apply(result); await prepare(); } }).catch(error => {
        if (isCurrent(value)) { notify(error); void refresh().catch(notify); }
      });
    }).catch(notify);
  };
  const openDraft = async (agentId: string, allowPartial = false) => {
    const share = current.value;
    if (!share || busy.value) return;
    busy.value = true;
    try {
      const result = await invoke<CurrentShare>('bind_current_share', {
        ...identity(share), agentId, allowPartial,
      });
      if (!isCurrent(share)) return;
      apply(result);
      if (!assistant.agents.length) await assistant.fetchAgentsAndGroups();
      if (!isCurrent(share) || !result.agentId || !result.topicId) return;
      await useTopicStore().loadTopicList(result.agentId, 'agent');
      if (!isCurrent(share)) return;
      await session.selectTopicById(result.agentId, 'agent', result.topicId);
      selectorOpen.value = false;
    } catch (error) { notify(error); }
    finally { busy.value = false; }
  };
  const flushDraft = (): Promise<void> => {
    if (saveTimer) { clearTimeout(saveTimer); saveTimer = null; }
    if (writing) return writing.then(() => pendingEdit ? flushDraft() : undefined);
    writing = (async () => {
      while (pendingEdit) {
        const edit = pendingEdit; pendingEdit = null;
        const share = current.value;
        if (!share || share.state !== 'draft' || !isCurrent(edit.identity)) continue;
        const result = await invoke<CurrentShare>('update_share_draft', { ...edit.identity, revision: share.revision, text: edit.text, attachments: edit.attachments });
        if (isCurrent(edit.identity)) apply(result);
      }
    })().finally(() => { writing = null; });
    return writing;
  };
  const editDraft = (text: string, attachments: Attachment[]) => {
    const share = activeDraft.value;
    if (!share || sending.value) return;
    pendingEdit = { identity: identity(share), text, attachments: attachments.filter(a => a.status === 'done').map(a => ({ ...a })) };
    if (saveTimer) clearTimeout(saveTimer);
    saveTimer = setTimeout(() => { void flushDraft().catch(notify); }, 300);
  };
  const commit = async (share: ShareIdentity, message: ChatMessage) => {
    try {
      await flushDraft();
      if (!isCurrent(share) || !current.value) throw new Error('分享已被新的分享替换');
      const result = await invoke<ShareCommit>('commit_share_message', { ...share, revision: current.value.revision, message: { ...message, blocks: undefined } });
      if (isCurrent(share)) { current.value = { ...current.value, state: 'sent', text: '', attachments: [], items: [] }; selectorOpen.value = false; }
      return result;
    } finally {
      // Only the message transaction is serialized. The normal AI stream may continue independently.
      sending.value = false;
    }
  };
  const discard = async () => {
    if (!current.value) return;
    const share = identity(current.value);
    try {
      await invoke('discard_current_share', { ...share });
      if (isCurrent(share) && current.value) { current.value = { ...current.value, state: 'discarded' }; selectorOpen.value = false; }
    } catch (error) { notify(error); }
  };
  return { current, activeDraft, failures, selectorOpen, busy, sending, refresh, prepare, receiveAndroid, openDraft, editDraft, flushDraft, commit, discard, notify };
});

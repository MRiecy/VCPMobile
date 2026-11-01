import { computed, onScopeDispose, ref } from 'vue';
import { defineStore } from 'pinia';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { useChatSessionStore } from './chatSessionStore';
import { useAssistantStore } from './assistant';
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
  let disposed = false;
  let listening: Promise<void> | null = null;
  let unlisten: (() => void) | null = null;
  let pendingEdit: { identity: ShareIdentity; text: string; attachments: Attachment[] } | null = null;
  let writing: Promise<void> | null = null;
  let saveTimer: ReturnType<typeof setTimeout> | null = null;
  let dismissedShare: ShareIdentity | null = null;
  const activeDraft = computed(() => current.value?.state === 'draft'
    && current.value.topicId === session.currentTopicId
    && session.currentSelectedItem?.type === 'agent'
    && current.value.agentId === session.currentSelectedItem?.id ? current.value : null);
  const failures = computed(() => current.value?.items.filter(item => item.error) ?? []);
  const identity = (value: ShareIdentity): ShareIdentity => ({ generation: value.generation, deliveryId: value.deliveryId });
  const isCurrent = (value: ShareIdentity) => current.value?.generation === value.generation && current.value?.deliveryId === value.deliveryId;
  const isTerminal = (value: CurrentShare) => value.state === 'sent' || value.state === 'discarded';
  const closeSelector = () => {
    dismissedShare = current.value ? identity(current.value) : null;
    selectorOpen.value = false;
  };
  const showSelector = () => {
    if (!current.value || !['ready', 'prepared', 'draft', 'failed'].includes(current.value.state)) return;
    if (!dismissedShare || !isCurrent(dismissedShare)) selectorOpen.value = true;
  };
  const notify = (error: unknown) => useNotificationStore().addNotification({
    type: 'error', title: '分享处理失败', message: error instanceof Error ? error.message : String(error), toastOnly: false,
  });
  const apply = (value: CurrentShare | null): boolean => {
    if (value && current.value && value.generation < current.value.generation) return false;
    if (value && current.value && isCurrent(value)) {
      if (value.revision < current.value.revision) return false;
      // Rust's sent/discarded states cannot return to a draft. Their revision
      // does not advance, so an older read/import can otherwise revive them.
      if (isTerminal(current.value) && !isTerminal(value)) return false;
    }
    const changed = value?.deliveryId !== current.value?.deliveryId;
    if (changed) { pendingEdit = null; selectorOpen.value = false; }
    current.value = value;
    if (value && isTerminal(value)) selectorOpen.value = false;
    if (value && !['sent', 'discarded', 'receiving'].includes(value.state) && changed) showSelector();
    return true;
  };
  const prepare = async () => {
    const share = current.value;
    if (!share || !['ready', 'prepared', 'draft'].includes(share.state) || preparingId === share.deliveryId) return;
    preparingId = share.deliveryId;
    busy.value = true;
    try {
      const result = await invoke<CurrentShare>('prepare_current_share', { ...identity(share) });
      if (isCurrent(share) && apply(result) && !activeDraft.value) showSelector();
    } catch (error) { if (isCurrent(share)) throw error; }
    finally { if (preparingId === share.deliveryId) { preparingId = null; busy.value = false; } }
  };
  const readCurrent = (): Promise<void> => {
    if (disposed) return Promise.resolve();
    if (readInFlight) { readAgain = true; return readInFlight; }
    readInFlight = (async () => {
      const observed = current.value;
      const value = await invoke<CurrentShare | null>('read_current_share');
      if (disposed) return;
      // An empty read has no generation; it cannot erase a share accepted or
      // settled while this snapshot was in flight.
      if (value === null && current.value !== observed) return;
      if (!apply(value)) return;
      if (value && ['ready', 'draft'].includes(value.state)) void prepare().catch(notify);
      else if (value && ['draft', 'prepared', 'failed'].includes(value.state) && !activeDraft.value) showSelector();
    })().finally(() => {
      readInFlight = null;
      if (readAgain && !disposed) { readAgain = false; void readCurrent().catch(notify); }
    });
    return readInFlight;
  };
  const refresh = (): Promise<void> => {
    // A fresh READY/foreground entry may resume the retained share. Continuations of
    // an older read/import keep the user's close intent until such an entry occurs.
    dismissedShare = null;
    return readCurrent();
  };
  const ensureListening = (): Promise<void> => {
    if (disposed || unlisten) return Promise.resolve();
    if (listening) return listening;
    listening = listen('vcp-share-inbox-changed', () => {
      // Admission and file publication can precede a slow handoff's completion.
      // Read the durable current share without reopening a dismissed selector.
      if (!disposed) void readCurrent().catch(notify);
    }).then(stop => {
      if (disposed) stop();
      else unlisten = stop;
    }).finally(() => { listening = null; });
    return listening;
  };
  const pollAndroid = async () => {
    await ensureListening();
    if (disposed) return;
    // Rust reads and accepts the native current slot itself; no payload or cached
    // admission result crosses this call. A later read always sees the owner now.
    await invoke<void>('receive_android_share');
    await readCurrent();
  };
  const openDraft = async (agentId: string, allowPartial = false) => {
    const share = current.value;
    if (!share || busy.value) return;
    const intent = session.beginNavigation();
    busy.value = true;
    try {
      const result = await invoke<CurrentShare>('bind_current_share', {
        ...identity(share), agentId, allowPartial,
      });
      if (!isCurrent(share) || !apply(result)) return;
      if (!result.agentId || !result.topicId || !session.isNavigationCurrent(intent)) return;
      try {
        if (!assistant.agents.length) await assistant.fetchAgentsAndGroups();
        if (!isCurrent(share) || !session.isNavigationCurrent(intent)) return;
        const status = await session.selectTopicById(result.agentId, 'agent', result.topicId, intent);
        if (!isCurrent(share) || !session.isNavigationCurrent(intent)) return;
        if (status === 'accepted') selectorOpen.value = false;
        else session.notifyNavigationResult(status);
      } catch (error) {
        if (isCurrent(share) && session.isNavigationCurrent(intent)) {
          useNotificationStore().addNotification({ type: 'error', title: '分享草稿已保存，打开失败', message: String(error), toastOnly: true });
        }
      }
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
      if (isCurrent(share) && current.value && current.value.state !== 'sent') { current.value = { ...current.value, state: 'discarded' }; selectorOpen.value = false; }
    } catch (error) { notify(error); }
  };
  onScopeDispose(() => {
    disposed = true;
    readAgain = false;
    unlisten?.();
    unlisten = null;
    if (saveTimer) clearTimeout(saveTimer);
  });
  return { current, activeDraft, failures, selectorOpen, busy, sending, refresh, prepare, pollAndroid, openDraft, editDraft, flushDraft, commit, discard, closeSelector, notify };
});

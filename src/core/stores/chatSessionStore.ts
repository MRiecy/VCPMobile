import { defineStore } from "pinia";
import { ref, computed, onScopeDispose, shallowRef } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useNotificationStore } from "./notification";
import { useAssistantStore } from "./assistant";
import { useTopicStore } from "./topicListManager";
import { parseLocalPreferenceObject } from "../preferences/localPreference";
import type {
  ConversationOwnerItem,
  OwnerType,
} from "../types/assistant";

export type ConversationOwnerType = OwnerType;

export interface ConversationKey {
  ownerId: string;
  ownerType: ConversationOwnerType;
  topicId: string;
  epoch: number;
}

type FocusTarget =
  | { kind: 'welcome' }
  | { kind: 'owner'; ownerType: OwnerType; ownerId: string }
  | { kind: 'topic'; ownerType: OwnerType; ownerId: string; topicId: string };

interface FocusSnapshot {
  target: FocusTarget;
  focusRevision: number;
  windowRevision: number;
  anchorId: string | null;
}

export type AcceptedWindow = Omit<FocusSnapshot, 'target'> & { documentId: number };
type NavigationRejection = 'call_in_progress' | 'missing' | 'stale';
export type NavigationStatus = 'accepted' | NavigationRejection;
type NavigationResult = { status: 'accepted'; focus: FocusSnapshot } | { status: NavigationRejection };

export const useChatSessionStore = defineStore("chatSession", () => {
  const projection = shallowRef<{
    item: ConversationOwnerItem | null; topicId: string | null; epoch: number;
    key: ConversationKey | null; window: AcceptedWindow | null;
  }>({ item: null, topicId: null, epoch: 0, key: null, window: null });
  const acceptedWindow = computed(() => projection.value.window);
  const currentSelectedItem = computed(() => projection.value.item);
  const currentTopicId = computed(() => projection.value.topicId);
  const sessionEpoch = computed(() => projection.value.epoch);
  const currentConversationKey = computed(() => projection.value.key);
  const navigationSequence = ref(0);
  let documentId: number | null = null;
  let connection: Promise<number> | null = null;
  let releaseListener: UnlistenFn | null = null;
  let registration: Promise<boolean> | null = null;
  let disposed = false;
  const coldHint = ref<{ currentSelectedItem: ConversationOwnerItem | null; currentTopicId: string | null }>({
    currentSelectedItem: null, currentTopicId: null,
  });
  const lastActiveTopicMap = ref<Record<string, string>>({});
  const ownerMapKey = (ownerType: ConversationOwnerType, ownerId: string) =>
    `${ownerType}:${ownerId}`;

  // All reactive readers see the target, identity and window change together.
  const setConversation = (
    item: ConversationOwnerItem | null, topicId: string | null, window?: AcceptedWindow,
  ) => {
    const previous = projection.value;
    const changed = previous.item?.id !== item?.id || previous.item?.type !== item?.type || previous.topicId !== topicId
      || Boolean(previous.window && window && previous.window.focusRevision !== window.focusRevision);
    const epoch = previous.epoch + Number(changed);
    let key = previous.key;
    if (changed) key = item && topicId ? { ownerId: item.id, ownerType: item.type, topicId, epoch } : null;
    projection.value = { item, topicId, key, epoch, window: window ?? (changed ? null : previous.window) };
    coldHint.value = { currentSelectedItem: item, currentTopicId: topicId };
  };

  const isConversationCurrent = (key: ConversationKey | null | undefined) => {
    const current = currentConversationKey.value;
    return Boolean(
      key &&
        current &&
        key.ownerId === current.ownerId &&
        key.ownerType === current.ownerType &&
        key.topicId === current.topicId &&
        key.epoch === current.epoch,
    );
  };

  // 动态检索当前活跃的话题对象
  const currentTopic = computed(() => {
    const ownerId = currentSelectedItem.value?.id;
    const ownerType = currentSelectedItem.value?.type;
    if (
      !ownerId ||
      !currentTopicId.value ||
      (ownerType !== "agent" && ownerType !== "group")
    ) return null;
    const topicStore = useTopicStore();
    return topicStore.topics.find(
      (topic) =>
        topic.id === currentTopicId.value &&
        topic.ownerId === ownerId &&
        topic.ownerType === ownerType,
    ) || null;
  });

  // 顶部显示标题（话题名，无话题则回退到智能体/群组名字）
  const headerTitle = computed(() => {
    return currentTopic.value?.name || currentSelectedItem.value?.name || "VCP Mobile";
  });

  const assistantStore = useAssistantStore();

  const targetOf = (item: ConversationOwnerItem | null, topicId: string | null): FocusTarget => {
    if (!item) return { kind: 'welcome' };
    const owner = { ownerType: item.type, ownerId: item.id };
    return topicId ? { kind: 'topic', ...owner, topicId } : { kind: 'owner', ...owner };
  };

  const ownerInfo = (target: Exclude<FocusTarget, { kind: 'welcome' }>): ConversationOwnerItem => {
    const owners = target.ownerType === 'agent' ? assistantStore.agents : assistantStore.groups;
    const owner = owners.find(candidate => candidate.id === target.ownerId);
    return { id: target.ownerId, name: owner?.name, avatarCalculatedColor: owner?.avatarCalculatedColor, type: target.ownerType };
  };

  const applyFocus = (id: number, focus: FocusSnapshot) => {
    if (disposed || id !== documentId) return;
    const previous = acceptedWindow.value;
    if (previous && previous.documentId === id && focus.windowRevision <= previous.windowRevision) return;
    const target = focus.target;
    const window = { documentId: id, focusRevision: focus.focusRevision, windowRevision: focus.windowRevision, anchorId: focus.anchorId };
    const previousHint = coldHint.value;
    if (target.kind === 'owner' && previousHint.currentSelectedItem?.id === target.ownerId
      && previousHint.currentSelectedItem.type === target.ownerType) {
      const mapKey = ownerMapKey(target.ownerType, target.ownerId);
      if (lastActiveTopicMap.value[mapKey] === previousHint.currentTopicId) delete lastActiveTopicMap.value[mapKey];
    }
    const topic = target.kind === 'topic' ? target.topicId : null;
    setConversation(target.kind === 'welcome' ? null : ownerInfo(target), topic, window);
    if (target.kind === 'topic') lastActiveTopicMap.value[ownerMapKey(target.ownerType, target.ownerId)] = target.topicId;
  };

  // Allocate immediately, before asynchronous listener installation. Older
  // documents cannot reclaim navigation when a late callback finally runs.
  const connect = () => {
    if (connection) return connection;
    let pending: FocusSnapshot | null = null;
    const attached = invoke<{ documentId: number; focus: FocusSnapshot }>('connect_chat_view');
    const listening = listen<FocusSnapshot>('vcp-chat-focus', ({ payload }) => {
      if (documentId !== null) applyFocus(documentId, payload);
      else if (!pending || pending.windowRevision < payload.windowRevision) pending = payload;
    });
    connection = Promise.all([attached, listening]).then(([result, release]) => {
      if (disposed) throw new Error('chat_view_disposed');
      releaseListener = release;
      documentId = result.documentId;
      if (pending) applyFocus(documentId, pending);
      return documentId;
    }).catch(error => {
      void listening.then(release => release(), () => {});
      connection = null;
      throw error;
    });
    return connection;
  };

  const beginNavigation = () => {
    const sequence = ++navigationSequence.value;
    registration = connect().then(async id => {
      if (!isNavigationCurrent(sequence)) return false;
      const registered = await invoke<{ focus: FocusSnapshot } | null>('register_chat_navigation_intent', { documentId: id, sequence });
      if (!registered || !isNavigationCurrent(sequence)) return false;
      // An earlier action can commit in Rust before this tap but return to JS
      // after it. Registration carries the actual starting focus of this tap.
      if (registered.focus.focusRevision > 0) applyFocus(id, registered.focus);
      return true;
    });
    // App-only notifications may supersede an old intent without navigating.
    void registration.catch(error => console.warn('[ChatSession] Intent registration failed:', error));
    return sequence;
  };

  const isNavigationCurrent = (sequence: number) => !disposed && sequence === navigationSequence.value;

  const navigate = async (
    target: FocusTarget, sequence: number,
    options: { anchorId?: string; selectRecent?: boolean; rememberedTopicId?: string; sortMode?: string } = {},
  ): Promise<NavigationStatus> => {
    if (!isNavigationCurrent(sequence) || !await registration || !isNavigationCurrent(sequence)) return 'stale';
    const id = await connect();
    if (!isNavigationCurrent(sequence)) return 'stale';
    const result = await invoke<NavigationResult>('request_chat_navigation', { documentId: id, sequence, target, ...options });
    if (!isNavigationCurrent(sequence)) return 'stale';
    if (result.status === 'accepted' && (acceptedWindow.value?.windowRevision ?? 0) > result.focus.windowRevision) return 'stale';
    if (result.status === 'accepted') applyFocus(id, result.focus);
    return result.status;
  };

  const restoreFocus = async (): Promise<void> => {
    const hint = targetOf(coldHint.value.currentSelectedItem, coldHint.value.currentTopicId);
    const id = await connect();
    const result = await invoke<NavigationResult>('restore_chat_view', { documentId: id, hint });
    if (result.status === 'accepted') applyFocus(id, result.focus);
  };

  const selectTopicById = (
    ownerId: string, ownerType: OwnerType, topicId: string, sequence = beginNavigation(), anchorId?: string,
  ) => navigate({ kind: 'topic', ownerId, ownerType, topicId }, sequence, { anchorId });

  const selectItem = (item: ConversationOwnerItem, sequence = beginNavigation()) =>
    navigate(targetOf(item, null), sequence, {
      selectRecent: true,
      rememberedTopicId: lastActiveTopicMap.value[ownerMapKey(item.type, item.id)],
      sortMode: useTopicStore().effectiveSortMode,
    });

  const selectOwnerOnly = (item: ConversationOwnerItem, sequence = beginNavigation()) =>
    navigate(targetOf(item, null), sequence);

  const clearConversation = (sequence = beginNavigation()) => navigate({ kind: 'welcome' }, sequence);

  // Entity refreshes can update labels. Only a Rust focus snapshot can
  // invalidate an owner or topic after a confirmed deletion.
  const reconcileCurrentConversation = () => {
    const target = targetOf(currentSelectedItem.value, currentTopicId.value);
    if (target.kind !== 'welcome') setConversation(ownerInfo(target), currentTopicId.value);
  };

  const notifyNavigationResult = (status: NavigationStatus) => {
    if (status === 'call_in_progress' || status === 'missing') {
      useNotificationStore().addNotification({
        type: 'info', title: status === 'call_in_progress' ? '通话中无法切换会话' : '目标已不存在',
        message: status === 'call_in_progress' ? '请先挂断当前通话。' : '会话或目标消息已被删除。', toastOnly: true,
      });
    }
  };

  onScopeDispose(() => {
    disposed = true;
    releaseListener?.();
  });

  return {
    currentSelectedItem,
    currentTopicId,
    coldHint,
    sessionEpoch,
    currentConversationKey,
    currentTopic,
    headerTitle,
    lastActiveTopicMap,
    setConversation,
    clearConversation,
    acceptedWindow,
    navigationSequence,
    beginNavigation,
    isNavigationCurrent,
    selectOwnerOnly,
    notifyNavigationResult,
    restoreFocus,
    isConversationCurrent,
    reconcileCurrentConversation,
    selectTopicById,
    selectItem,
  };
}, {
  persist: {
    pick: ['coldHint', 'lastActiveTopicMap'],
    serializer: {
      serialize: state => JSON.stringify({ ...state.coldHint, lastActiveTopicMap: state.lastActiveTopicMap }),
      deserialize(raw) {
        const value = parseLocalPreferenceObject(raw);
        const item = value.currentSelectedItem;
        let currentSelectedItem: ConversationOwnerItem | null = null;
        if (item !== null && typeof item === "object" && !Array.isArray(item)
          && "id" in item && typeof item.id === "string"
          && "type" in item && (item.type === "agent" || item.type === "group")) {
          currentSelectedItem = {
            id: item.id,
            type: item.type,
            name: "name" in item && typeof item.name === "string" ? item.name : undefined,
            avatarCalculatedColor: "avatarCalculatedColor" in item && typeof item.avatarCalculatedColor === "string"
              ? item.avatarCalculatedColor : null,
          };
        }
        const remembered = value.lastActiveTopicMap;
        const lastActiveTopicMap = remembered !== null && typeof remembered === "object" && !Array.isArray(remembered)
          ? Object.fromEntries(Object.entries(remembered).filter(([owner, topic]) =>
            /^(agent|group):/.test(owner) && typeof topic === "string"))
          : {};
        return {
          coldHint: { currentSelectedItem, currentTopicId: typeof value.currentTopicId === "string" ? value.currentTopicId : null },
          lastActiveTopicMap,
        };
      },
    },
    afterHydrate: ({ store }) => store.$persist(),
  },
});

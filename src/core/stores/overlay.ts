import { defineStore } from 'pinia';
import { ref, shallowRef, computed } from 'vue';
import { useModalHistory } from '../composables/useModalHistory';
import { LAYER_PAGE_BASE, LAYER_PAGE_MAX_OFFSET } from '../constants/layers';
import { useSyncSessionStore } from './syncSession';
import { useRebuildSessionStore } from './rebuildSession';
import type {
  ConfirmConfig,
  ConfirmOptions,
  ContextMenuConfig,
  EditorConfig,
  OverlayActionItem,
  PromptConfig,
} from '../types/overlay';

interface PageStackItem {
  type: string;
  id?: string;
  modalId: string;
}

interface ConfirmRequest {
  options: ConfirmOptions;
  resolve: (confirmed: boolean) => void;
  settled: boolean;
}

export const useOverlayStore = defineStore('overlay', () => {
  const { registerModal, unregisterModal, replaceModal } = useModalHistory();

  const promptConfig = ref<PromptConfig | null>(null);
  const confirmConfig = ref<ConfirmConfig | null>(null);
  const contextMenuConfig = shallowRef<ContextMenuConfig | null>(null);
  const editorConfig = ref<EditorConfig | null>(null);

  const confirmQueue: ConfirmRequest[] = [];
  let activeConfirm: ConfirmRequest | null = null;

  const registerOverlayModal = (modalId: string, closeHandler: () => void) => {
    if (
      modalId !== 'ContextMenu' &&
      contextMenuConfig.value &&
      replaceModal('ContextMenu', modalId, closeHandler)
    ) {
      contextMenuConfig.value = null;
      return;
    }

    registerModal(modalId, closeHandler);
  };

  // --- Page Stack (Virtual Navigation Stack) ---
  const pageStack = ref<PageStackItem[]>([]);

  const pageStackTop = computed(() => pageStack.value[pageStack.value.length - 1] || null);

  const isSettingsOpen = computed(() => pageStack.value.some(p => p.type === 'settings'));
  const isAgentSettingsOpen = computed(() => pageStack.value.some(p => p.type === 'agentSettings'));
  const isGroupSettingsOpen = computed(() => pageStack.value.some(p => p.type === 'groupSettings'));
  const isSyncSessionOpen = computed(() => pageStack.value.some(p => p.type === 'syncSession'));
  const isRebuildSessionOpen = computed(() => pageStack.value.some(p => p.type === 'rebuildSession'));
  const isTarvenSettingsOpen = computed(() => pageStack.value.some(p => p.type === 'tarvenSettings'));
  const isDistributedOpen = computed(() => pageStack.value.some(p => p.type === 'distributed'));
  const isDailyNoteOpen = computed(() => pageStack.value.some(p => p.type === 'dailyNote'));
  const isRagObserverOpen = computed(() => pageStack.value.some(p => p.type === 'ragObserver'));

  const agentSettingsId = computed(() => {
    const page = pageStack.value.find(p => p.type === 'agentSettings');
    return page?.id || '';
  });

  const groupSettingsId = computed(() => {
    const page = pageStack.value.find(p => p.type === 'groupSettings');
    return page?.id || '';
  });

  const getPageZIndex = (type: string) => {
    const index = pageStack.value.findIndex(p => p.type === type);
    if (index === -1) return LAYER_PAGE_BASE;
    return LAYER_PAGE_BASE + Math.min(index, LAYER_PAGE_MAX_OFFSET);
  };

  const pushPage = (type: string, id?: string) => {
    const modalId = `Page:${type}:${id || ''}`;
    const top = pageStack.value[pageStack.value.length - 1];
    if (top && top.type === type && top.id === id) return;

    pageStack.value.push({ type, id, modalId });
    registerModal(modalId, () => {
      popPageInternal();
    });
  };

  // Internal pop: removes from both pageStack and modalStack (used by handlePopState close callback)
  const popPageInternal = () => {
    if (pageStack.value.length === 0) return;
    const top = pageStack.value[pageStack.value.length - 1];
    unregisterModal(top.modalId);
    pageStack.value.pop();
  };

  // Public pop: removes from pageStack and syncs modal history (used by UI close buttons)
  const popPage = () => {
    if (pageStack.value.length === 0) return;
    const top = pageStack.value[pageStack.value.length - 1];
    unregisterModal(top.modalId);
    pageStack.value.pop();
  };

  const popToRoot = () => {
    while (pageStack.value.length > 0) {
      const top = pageStack.value[pageStack.value.length - 1];
      unregisterModal(top.modalId);
      pageStack.value.pop();
    }
  };

  // --- Sync Session (managed separately due to its internal state machine) ---
  const openSyncSession = () => {
    if (isSyncSessionOpen.value) return;
    const syncStore = useSyncSessionStore();
    syncStore.open();
    const modalId = 'Page:syncSession';
    pageStack.value.push({ type: 'syncSession', id: undefined, modalId });
    registerModal(modalId, () => {
      syncStore.close();
      popPageInternal();
    });
  };

  const closeSyncSession = () => {
    if (!isSyncSessionOpen.value) return;
    const syncStore = useSyncSessionStore();
    syncStore.close();
    const top = pageStack.value[pageStack.value.length - 1];
    if (top?.type === 'syncSession') {
      unregisterModal(top.modalId);
      pageStack.value.pop();
    }
  };

  // --- Rebuild Session ---
  const openRebuildSession = (taskType: import('./rebuildSession').RebuildTaskType = 'preRender') => {
    if (isRebuildSessionOpen.value) return;
    const rebuildStore = useRebuildSessionStore();
    rebuildStore.open(taskType);
    const modalId = 'Page:rebuildSession';
    pageStack.value.push({ type: 'rebuildSession', id: undefined, modalId });
    registerModal(modalId, () => {
      rebuildStore.close();
      popPageInternal();
    });
  };

  const closeRebuildSession = () => {
    if (!isRebuildSessionOpen.value) return;
    const rebuildStore = useRebuildSessionStore();
    rebuildStore.close();
    const top = pageStack.value[pageStack.value.length - 1];
    if (top?.type === 'rebuildSession') {
      unregisterModal(top.modalId);
      pageStack.value.pop();
    }
  };

  // --- Legacy API wrappers (backward compatible) ---
  const openSettings = () => {
    pushPage('settings');
  };

  const closeSettings = () => {
    popPage();
  };

  const openAgentSettings = (id: string) => {
    pushPage('agentSettings', id);
  };

  const closeAgentSettings = () => {
    popPage();
  };

  const openGroupSettings = (id: string) => {
    pushPage('groupSettings', id);
  };

  const closeGroupSettings = () => {
    popPage();
  };

  const openTarvenSettings = () => {
    pushPage('tarvenSettings');
  };

  const closeTarvenSettings = () => {
    popPage();
  };

  const openDistributed = () => {
    pushPage('distributed');
  };

  const closeDistributed = () => {
    popPage();
  };

  const openDailyNote = () => {
    pushPage('dailyNote');
  };

  const closeDailyNote = () => {
    popPage();
  };

  const openRagObserver = () => {
    pushPage('ragObserver');
  };

  const closeRagObserver = () => {
    popPage();
  };

  // --- Modal API (unchanged) ---
  const openPrompt = (config: PromptConfig) => {
    promptConfig.value = config;
    registerOverlayModal('Prompt', () => { promptConfig.value = null; });
  };

  const closePrompt = () => {
    if (promptConfig.value) {
      unregisterModal('Prompt');
      promptConfig.value = null;
    }
  };

  function activateNextConfirm() {
    if (activeConfirm || confirmQueue.length === 0) return;

    const request = confirmQueue.shift();
    if (!request) return;

    activeConfirm = request;
    confirmConfig.value = {
      title: request.options.title,
      message: request.options.message,
      confirmText: request.options.confirmText || '确认',
      cancelText: request.options.cancelText || '取消',
      isDanger: request.options.isDanger ?? false,
      onlyConfirm: request.options.onlyConfirm ?? false,
    };

    registerOverlayModal('Confirm', () => {
      settleConfirm(false, true);
    });
  }

  function settleConfirm(confirmed: boolean, fromHistory = false) {
    const request = activeConfirm;
    if (!request || request.settled) return;

    request.settled = true;
    const result = confirmConfig.value?.onlyConfirm ? true : confirmed;
    confirmConfig.value = null;

    const finalize = () => {
      if (activeConfirm !== request) return;
      activeConfirm = null;
      request.resolve(result);

      if (confirmQueue.length > 0) {
        queueMicrotask(activateNextConfirm);
      }
    };

    if (fromHistory) {
      finalize();
    } else {
      unregisterModal('Confirm', finalize);
    }
  }

  const showConfirm = (options: ConfirmOptions): Promise<boolean> => {
    return new Promise<boolean>((resolve) => {
      confirmQueue.push({ options, resolve, settled: false });
      activateNextConfirm();
    });
  };

  const resolveConfirm = (confirmed: boolean) => {
    settleConfirm(confirmed);
  };

  const closeConfirm = () => {
    settleConfirm(false);
  };

  const openContextMenu = (actions: OverlayActionItem[], title?: string) => {
    contextMenuConfig.value = {
      title: title || '',
      actions
    };
    registerModal('ContextMenu', () => { contextMenuConfig.value = null; });
  };

  const closeContextMenu = () => {
    if (contextMenuConfig.value) {
      unregisterModal('ContextMenu');
      contextMenuConfig.value = null;
    }
  };

  const openEditor = (config: EditorConfig) => {
    editorConfig.value = config;
    registerOverlayModal('FullScreenEditor', () => { editorConfig.value = null; });
  };

  const closeEditor = () => {
    if (editorConfig.value) {
      unregisterModal('FullScreenEditor');
      editorConfig.value = null;
    }
  };

  return {
    // Page stack
    pageStack,
    pageStackTop,
    getPageZIndex,
    pushPage,
    popPage,
    popToRoot,
    // Legacy visibility flags (computed)
    isSettingsOpen,
    isAgentSettingsOpen,
    agentSettingsId,
    isGroupSettingsOpen,
    groupSettingsId,
    isSyncSessionOpen,
    isRebuildSessionOpen,
    isTarvenSettingsOpen,
    isDistributedOpen,
    isDailyNoteOpen,
    isRagObserverOpen,
    // Legacy open/close (now backed by page stack)
    openSettings,
    closeSettings,
    openAgentSettings,
    closeAgentSettings,
    openGroupSettings,
    closeGroupSettings,
    openSyncSession,
    closeSyncSession,
    openRebuildSession,
    closeRebuildSession,
    openTarvenSettings,
    closeTarvenSettings,
    openDistributed,
    closeDistributed,
    openDailyNote,
    closeDailyNote,
    openRagObserver,
    closeRagObserver,
    // Modals
    promptConfig,
    confirmConfig,
    contextMenuConfig,
    editorConfig,
    openPrompt,
    closePrompt,
    showConfirm,
    resolveConfirm,
    closeConfirm,
    openContextMenu,
    closeContextMenu,
    openEditor,
    closeEditor
  };
});

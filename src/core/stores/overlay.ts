import { defineStore } from 'pinia';
import { ref, shallowRef, computed } from 'vue';
import { useModalHistory } from '../composables/useModalHistory';
import { LAYER_PAGE_BASE, LAYER_PAGE_MAX_OFFSET } from '../constants/layers';
import { useSyncSessionStore } from './syncSession';
import { useRebuildSessionStore } from './rebuildSession';
import type { OverlayActionItem, ContextMenuConfig, PromptConfig, EditorConfig, ConfirmConfig } from '../types/overlay';

export type OverlayPageType =
  | 'settings'
  | 'agentSettings'
  | 'groupSettings'
  | 'syncSession'
  | 'rebuildSession'
  | 'tarvenSettings'
  | 'distributed'
  | 'ragObserver'
  | 'diaryCenter'
  | 'cliManifest'
  | 'logCenter'
  | 'taskCenter'
  | 'agentMgr'
  | 'forum'
  | 'mail'
  | 'globalSearch';

interface DiaryOpenTarget {
  folder: string;
  file: string;
}

interface PageStackItem {
  type: OverlayPageType;
  id?: string;
  modalId: string;
}

export const useOverlayStore = defineStore('overlay', () => {
  const { registerModal, unregisterModal } = useModalHistory();

  const promptConfig = ref<PromptConfig | null>(null);
  const confirmConfig = ref<ConfirmConfig | null>(null);
  const contextMenuConfig = shallowRef<ContextMenuConfig | null>(null);
  const editorConfig = ref<EditorConfig | null>(null);
  const diaryOpenTarget = ref<DiaryOpenTarget | null>(null);

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
  const isRagObserverOpen = computed(() => pageStack.value.some(p => p.type === 'ragObserver'));
  const isDiaryCenterOpen = computed(() => pageStack.value.some(p => p.type === 'diaryCenter'));
  const isCliManifestOpen = computed(() => pageStack.value.some(p => p.type === 'cliManifest'));
  const isLogCenterOpen = computed(() => pageStack.value.some(p => p.type === 'logCenter'));
  const isTaskCenterOpen = computed(() => pageStack.value.some(p => p.type === 'taskCenter'));
  const isAgentMgrOpen = computed(() => pageStack.value.some(p => p.type === 'agentMgr'));
  const isForumOpen = computed(() => pageStack.value.some(p => p.type === 'forum'));
  const isMailOpen = computed(() => pageStack.value.some(p => p.type === 'mail'));
  const isGlobalSearchOpen = computed(() => pageStack.value.some(p => p.type === 'globalSearch'));


  const agentSettingsId = computed(() => {
    const page = pageStack.value.find(p => p.type === 'agentSettings');
    return page?.id || '';
  });

  const groupSettingsId = computed(() => {
    const page = pageStack.value.find(p => p.type === 'groupSettings');
    return page?.id || '';
  });

  const getPageZIndex = (type: OverlayPageType) => {
    const index = pageStack.value.findIndex(p => p.type === type);
    if (index === -1) return LAYER_PAGE_BASE;
    return LAYER_PAGE_BASE + Math.min(index, LAYER_PAGE_MAX_OFFSET);
  };

  const pushPage = (type: OverlayPageType, id?: string) => {
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
      // connecting / connected 期间由 sync store 持有不可卸载权；
      // registerModal 是同步回调，因此必须直接返回 canDismiss 门禁结果。
      if (!syncStore.canDismiss) return false;
      void syncStore.close();
      popPageInternal();
      return true;
    });
  };

  const closeSyncSession = async () => {
    if (!isSyncSessionOpen.value) return;
    const syncStore = useSyncSessionStore();
    if (!syncStore.canDismiss) return;
    await syncStore.close();
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

  const openRagObserver = () => {
    pushPage('ragObserver');
  };

  const closeRagObserver = () => {
    popPage();
  };

  const openDiaryCenter = (target?: DiaryOpenTarget) => {
    if (target) diaryOpenTarget.value = { ...target };
    if (isDiaryCenterOpen.value) return;
    pushPage('diaryCenter');
  };

  const clearDiaryOpenTarget = () => {
    diaryOpenTarget.value = null;
  };

  const closeDiaryCenter = () => {
    if (pageStackTop.value?.type !== 'diaryCenter') return;
    diaryOpenTarget.value = null;
    popPage();
  };

  const openCliManifest = () => {
    pushPage('cliManifest');
  };

  const closeCliManifest = () => {
    if (pageStackTop.value?.type !== 'cliManifest') return;
    popPage();
  };

  const openLogCenter = () => {
    pushPage('logCenter');
  };

  const closeLogCenter = () => {
    if (pageStackTop.value?.type !== 'logCenter') return;
    popPage();
  };

  const openTaskCenter = () => {
    pushPage('taskCenter');
  };

  const closeTaskCenter = () => {
    if (pageStackTop.value?.type !== 'taskCenter') return;
    popPage();
  };

  const openAgentMgr = () => {
    pushPage('agentMgr');
  };

  const closeAgentMgr = () => {
    if (pageStackTop.value?.type !== 'agentMgr') return;
    popPage();
  };

  const openForum = () => {
    pushPage('forum');
  };

  const closeForum = () => {
    if (pageStackTop.value?.type !== 'forum') return;
    popPage();
  };

  const openMail = () => {
    pushPage('mail');
  };

  const closeMail = () => {
    if (pageStackTop.value?.type !== 'mail') return;
    popPage();
  };

  const openGlobalSearch = () => {
    if (isGlobalSearchOpen.value) return;
    pushPage('globalSearch');
  };

  const closeGlobalSearch = () => {
    if (pageStackTop.value?.type !== 'globalSearch') return;
    popPage();
  };

  // --- Modal API (unchanged) ---
  const openPrompt = (config: PromptConfig) => {
    promptConfig.value = config;
    registerModal('Prompt', () => { promptConfig.value = null; });
  };

  const closePrompt = () => {
    if (promptConfig.value) {
      unregisterModal('Prompt');
      promptConfig.value = null;
    }
  };

  const showConfirm = (options: { title: string; message: string; isDanger?: boolean; onlyConfirm?: boolean }) => {
    return new Promise<boolean>((resolve) => {
      confirmConfig.value = {
        title: options.title,
        message: options.message,
        isDanger: options.isDanger,
        onlyConfirm: options.onlyConfirm,
        onConfirm: () => {
          unregisterModal('Confirm');
          confirmConfig.value = null;
          resolve(true);
        },
        onCancel: () => {
          unregisterModal('Confirm');
          confirmConfig.value = null;
          resolve(options.onlyConfirm ? true : false);
        }
      };
      registerModal('Confirm', () => {
        confirmConfig.value = null;
        resolve(options.onlyConfirm ? true : false);
      });
    });
  };

  const closeConfirm = () => {
    if (confirmConfig.value) {
      unregisterModal('Confirm');
      confirmConfig.value = null;
    }
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
    registerModal('FullScreenEditor', () => { editorConfig.value = null; });
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
    isRagObserverOpen,
    isDiaryCenterOpen,
    isCliManifestOpen,
    isLogCenterOpen,
    isTaskCenterOpen,
    isAgentMgrOpen,
    isForumOpen,
    isMailOpen,
    isGlobalSearchOpen,
    diaryOpenTarget,
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
    openRagObserver,
    closeRagObserver,
    openDiaryCenter,
    closeDiaryCenter,
    clearDiaryOpenTarget,
    openCliManifest,
    closeCliManifest,
    openLogCenter,
    closeLogCenter,
    openTaskCenter,
    closeTaskCenter,
    openAgentMgr,
    closeAgentMgr,
    openForum,
    closeForum,
    openMail,
    closeMail,
    openGlobalSearch,
    closeGlobalSearch,
    // Modals
    promptConfig,
    confirmConfig,
    contextMenuConfig,
    editorConfig,
    openPrompt,
    closePrompt,
    showConfirm,
    closeConfirm,
    openContextMenu,
    closeContextMenu,
    openEditor,
    closeEditor
  };
});

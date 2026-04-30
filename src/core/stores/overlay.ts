import { defineStore } from 'pinia';
import { ref, shallowRef, computed } from 'vue';
import { useModalHistory } from '../composables/useModalHistory';
import type { OverlayActionItem, ContextMenuConfig, PromptConfig, EditorConfig } from '../types/overlay';

interface PageStackItem {
  type: string;
  id?: string;
  modalId: string;
}

export const useOverlayStore = defineStore('overlay', () => {
  const { registerModal, unregisterModal } = useModalHistory();

  const promptConfig = ref<PromptConfig | null>(null);
  const contextMenuConfig = shallowRef<ContextMenuConfig | null>(null);
  const editorConfig = ref<EditorConfig | null>(null);

  // --- Page Stack (Virtual Navigation Stack) ---
  const pageStack = ref<PageStackItem[]>([]);

  const pageStackTop = computed(() => pageStack.value[pageStack.value.length - 1] || null);

  const isSettingsOpen = computed(() => pageStack.value.some(p => p.type === 'settings'));
  const isAgentSettingsOpen = computed(() => pageStack.value.some(p => p.type === 'agentSettings'));
  const isGroupSettingsOpen = computed(() => pageStack.value.some(p => p.type === 'groupSettings'));

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
    return index === -1 ? 50 : 50 + index;
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

  // Internal pop: removes from pageStack only (used by handlePopState close callback)
  const popPageInternal = () => {
    if (pageStack.value.length === 0) return;
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
    // Legacy open/close (now backed by page stack)
    openSettings,
    closeSettings,
    openAgentSettings,
    closeAgentSettings,
    openGroupSettings,
    closeGroupSettings,
    // Modals
    promptConfig,
    contextMenuConfig,
    editorConfig,
    openPrompt,
    closePrompt,
    openContextMenu,
    closeContextMenu,
    openEditor,
    closeEditor
  };
});
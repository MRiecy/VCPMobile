import { defineStore } from 'pinia';
import { ref } from 'vue';
import { useModalHistory } from '../composables/useModalHistory';
import type { OverlayActionItem, ContextMenuConfig, PromptConfig, EditorConfig } from '../types/overlay';

export const useOverlayStore = defineStore('overlay', () => {
  const { registerModal, unregisterModal } = useModalHistory();

  const promptConfig = ref<PromptConfig | null>(null);
  const contextMenuConfig = ref<ContextMenuConfig | null>(null);
  const editorConfig = ref<EditorConfig | null>(null);

  const isSettingsOpen = ref(false);
  const isSyncOpen = ref(false);

  const openSettings = () => {
    isSettingsOpen.value = true;
    registerModal('SettingsView', () => {
      isSettingsOpen.value = false;
    });
  };

  const closeSettings = () => {
    if (isSettingsOpen.value) {
      unregisterModal('SettingsView');
      isSettingsOpen.value = false;
    }
  };

  const openSync = () => {
    isSyncOpen.value = true;
    registerModal('SyncView', () => { isSyncOpen.value = false; });
  };

  const closeSync = () => {
    if (isSyncOpen.value) {
      unregisterModal('SyncView');
      isSyncOpen.value = false;
    }
  };

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
    promptConfig,
    contextMenuConfig,
    editorConfig,
    isSettingsOpen,
    isSyncOpen,
    openSettings,
    closeSettings,
    openSync,
    closeSync,
    openPrompt,
    closePrompt,
    openContextMenu,
    closeContextMenu,
    openEditor,
    closeEditor
  };
});

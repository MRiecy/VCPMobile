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
  const isAgentSettingsOpen = ref(false);
  const agentSettingsId = ref('');
  const isGroupSettingsOpen = ref(false);
  const groupSettingsId = ref('');

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

  const openAgentSettings = (id: string) => {
    agentSettingsId.value = id;
    isAgentSettingsOpen.value = true;
    registerModal('AgentSettings', () => {
      isAgentSettingsOpen.value = false;
    });
  };

  const closeAgentSettings = () => {
    if (isAgentSettingsOpen.value) {
      unregisterModal('AgentSettings');
      isAgentSettingsOpen.value = false;
    }
  };

  const openGroupSettings = (id: string) => {
    groupSettingsId.value = id;
    isGroupSettingsOpen.value = true;
    registerModal('GroupSettings', () => {
      isGroupSettingsOpen.value = false;
    });
  };

  const closeGroupSettings = () => {
    if (isGroupSettingsOpen.value) {
      unregisterModal('GroupSettings');
      isGroupSettingsOpen.value = false;
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
    isAgentSettingsOpen,
    agentSettingsId,
    isGroupSettingsOpen,
    groupSettingsId,
    openSettings,
    closeSettings,
    openAgentSettings,
    closeAgentSettings,
    openGroupSettings,
    closeGroupSettings,
    openPrompt,
    closePrompt,
    openContextMenu,
    closeContextMenu,
    openEditor,
    closeEditor
  };
});

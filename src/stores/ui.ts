import { defineStore } from 'pinia';
import { ref, watch, computed } from 'vue';
import { useModalHistory } from '../composables/useModalHistory';
import type { ActionItem } from '../components/BottomSheet.vue';

export type ModalName = 'settings' | 'sync' | null;

export interface PromptConfig {
  title: string;
  initialValue: string;
  placeholder: string;
  onConfirm: (val: string) => void;
}

export interface ContextMenuConfig {
  title: string;
  actions: ActionItem[];
}

export const useUIStore = defineStore('ui', () => {
  const { registerModal, unregisterModal } = useModalHistory();

  // --- State ---
  const leftDrawerOpen = ref(false);
  const rightDrawerOpen = ref(false);
  const activeModal = ref<ModalName>(null);
  const promptConfig = ref<PromptConfig | null>(null);
  const contextMenuConfig = ref<ContextMenuConfig | null>(null);

  // --- Actions ---
  const toggleLeftDrawer = () => {
    leftDrawerOpen.value = !leftDrawerOpen.value;
    if (leftDrawerOpen.value) rightDrawerOpen.value = false;
  };

  const toggleRightDrawer = () => {
    rightDrawerOpen.value = !rightDrawerOpen.value;
    if (rightDrawerOpen.value) leftDrawerOpen.value = false;
  };

  const setLeftDrawer = (open: boolean) => {
    leftDrawerOpen.value = open;
    if (open) rightDrawerOpen.value = false;
  };

  const setRightDrawer = (open: boolean) => {
    rightDrawerOpen.value = open;
    if (open) leftDrawerOpen.value = false;
  };

  const openModal = (modalName: ModalName) => {
    activeModal.value = modalName;
  };

  const closeModal = () => {
    activeModal.value = null;
  };

  const openPrompt = (config: PromptConfig) => {
    promptConfig.value = config;
  };

  const closePrompt = () => {
    promptConfig.value = null;
  };

  const openContextMenu = (actions: ActionItem[], title?: string) => {
    contextMenuConfig.value = {
      title: title || '',
      actions
    };
  };

  const closeContextMenu = () => {
    contextMenuConfig.value = null;
  };

  // --- History Integration ---
  // We watch the state and register/unregister with useModalHistory automatically.
  
  watch(leftDrawerOpen, (val) => {
    if (val && window.innerWidth < 768) {
      registerModal('LeftDrawer', () => { leftDrawerOpen.value = false; });
    } else if (!val) {
      unregisterModal('LeftDrawer');
    }
  });

  watch(rightDrawerOpen, (val) => {
    if (val && window.innerWidth < 768) {
      registerModal('RightDrawer', () => { rightDrawerOpen.value = false; });
    } else if (!val) {
      unregisterModal('RightDrawer');
    }
  });

  watch(activeModal, (val, oldVal) => {
    if (oldVal) {
      unregisterModal(`Modal_${oldVal}`);
    }
    if (val) {
      registerModal(`Modal_${val}`, () => { activeModal.value = null; });
    }
  });

  watch(() => promptConfig.value, (val) => {
    if (val) {
      registerModal('Prompt', () => { promptConfig.value = null; });
    } else {
      unregisterModal('Prompt');
    }
  });

  watch(() => contextMenuConfig.value, (val) => {
    if (val) {
      registerModal('ContextMenu', () => { contextMenuConfig.value = null; });
    } else {
      unregisterModal('ContextMenu');
    }
  });

  return {
    leftDrawerOpen,
    rightDrawerOpen,
    activeModal,
    promptConfig,
    contextMenuConfig,
    toggleLeftDrawer,
    toggleRightDrawer,
    setLeftDrawer,
    setRightDrawer,
    openModal,
    closeModal,
    openPrompt,
    closePrompt,
    openContextMenu,
    closeContextMenu
  };
});

import { defineStore } from 'pinia';
import { ref } from 'vue';
import { useModalHistory } from '../composables/useModalHistory';
import { useVoiceCallStore } from '../../features/chat/voice/voiceCallStore';

export const useLayoutStore = defineStore('layout', () => {
  const { registerModal, unregisterModal } = useModalHistory();
  const voiceCallStore = useVoiceCallStore();

  const leftDrawerOpen = ref(false);
  const rightDrawerOpen = ref(false);

  const toggleLeftDrawer = () => setLeftDrawer(!leftDrawerOpen.value);
  const toggleRightDrawer = () => setRightDrawer(!rightDrawerOpen.value);

  const setLeftDrawer = (open: boolean) => {
    if (open && voiceCallStore.active) return;
    if (open === leftDrawerOpen.value) return;

    if (!open) {
      leftDrawerOpen.value = false;
      unregisterModal('LeftDrawer');
      return;
    }

    setRightDrawer(false);
    leftDrawerOpen.value = true;
    registerModal('LeftDrawer', () => { leftDrawerOpen.value = false; });
  };

  const setRightDrawer = (open: boolean) => {
    if (open && voiceCallStore.active) return;
    if (open === rightDrawerOpen.value) return;

    if (!open) {
      rightDrawerOpen.value = false;
      unregisterModal('RightDrawer');
      return;
    }

    setLeftDrawer(false);
    rightDrawerOpen.value = true;
    registerModal('RightDrawer', () => { rightDrawerOpen.value = false; });
  };

  return {
    leftDrawerOpen,
    rightDrawerOpen,
    toggleLeftDrawer,
    toggleRightDrawer,
    setLeftDrawer,
    setRightDrawer
  };
});

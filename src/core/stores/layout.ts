import { defineStore } from 'pinia';
import { ref } from 'vue';
import { useModalHistory } from '../composables/useModalHistory';

export const useLayoutStore = defineStore('layout', () => {
  const { registerModal, unregisterModal } = useModalHistory();

  const leftDrawerOpen = ref(false);
  const rightDrawerOpen = ref(false);

  const toggleLeftDrawer = () => setLeftDrawer(!leftDrawerOpen.value);
  const toggleRightDrawer = () => setRightDrawer(!rightDrawerOpen.value);

  const setLeftDrawer = (open: boolean) => {
    if (open === leftDrawerOpen.value) return;
  
    if (!open) {
      leftDrawerOpen.value = false;
      unregisterModal('LeftDrawer');
      return;
    }
  
    setRightDrawer(false);
    leftDrawerOpen.value = true;
  
    if (window.innerWidth < 768) {
      registerModal('LeftDrawer', () => { leftDrawerOpen.value = false; });
    }
  };
  
  const setRightDrawer = (open: boolean) => {
    if (open === rightDrawerOpen.value) return;
  
    if (!open) {
      rightDrawerOpen.value = false;
      unregisterModal('RightDrawer');
      return;
    }
  
    setLeftDrawer(false);
    rightDrawerOpen.value = true;
  
    if (window.innerWidth < 768) {
      registerModal('RightDrawer', () => { rightDrawerOpen.value = false; });
    }
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

import { computed, onScopeDispose, ref, shallowRef, watch, type Ref } from 'vue';
import { useModalHistory } from '../../core/composables/useModalHistory';
import type { SettingsPageHandle, SettingsPageId } from './contracts';

export const SETTINGS_SUBPAGE_MODAL_ID = 'SettingsSubPage';

interface PageVisit {
  id: SettingsPageId;
  serial: number;
  setHandle: (instance: unknown) => void;
}

export function useSettingsNavigation(options: {
  isOpen: Readonly<Ref<boolean>>;
  surface: () => HTMLElement | null;
  close: () => void;
}) {
  const { registerModal, unregisterModal } = useModalHistory();
  const activeVisit = shallowRef<PageVisit | null>(null);
  const displayedVisit = shallowRef<PageVisit | null>(null);
  const pageHandle = shallowRef<SettingsPageHandle | null>(null);
  const loaded = ref(false);
  const pageError = ref('');
  const navigationError = ref('');
  const attempt = ref(0);
  const leaving = ref(false);
  let serial = 0;
  let leavePromise: Promise<void> | null = null;
  let developerOpen = false;

  const isCurrent = (visit: PageVisit) =>
    options.isOpen.value && activeVisit.value === visit;

  const closePage = () => {
    activeVisit.value = null;
    pageHandle.value = null;
    leaving.value = false;
    unregisterModal(SETTINGS_SUBPAGE_MODAL_ID);
  };

  // The return value belongs to ModalHistory's synchronous close protocol.
  // A Promise must never escape to that protocol, even for an already-clean form.
  const requestLeave = (): boolean => {
    const visit = activeVisit.value;
    if (!visit) {
      if (options.isOpen.value) options.close();
      return true;
    }
    if (developerOpen) {
      developerOpen = false;
      showPage('about');
      // The settings visit still owns this modal. Hardware back restores its
      // history entry; the next back can leave About normally.
      return false;
    }
    if (leavePromise) return false;
    const beforeLeave = pageHandle.value?.beforeLeave;
    if (!beforeLeave) {
      closePage();
      return true;
    }

    leaving.value = true;
    navigationError.value = '';
    const operation = Promise.resolve()
      .then(() => isCurrent(visit) ? beforeLeave() : false)
      .then((allowed) => {
        if (allowed && isCurrent(visit)) closePage();
      })
      .catch((error: unknown) => {
        if (isCurrent(visit)) {
          navigationError.value = `设置保存失败：${error instanceof Error ? error.message : String(error)}`;
        }
      })
      .finally(() => {
        if (leavePromise === operation) {
          leavePromise = null;
          leaving.value = false;
        }
      });
    leavePromise = operation;
    return false;
  };

  const showPage = (id: SettingsPageId) => {
    const visit: PageVisit = {
      id,
      serial: ++serial,
      setHandle(instance) {
        if (isCurrent(visit)) {
          pageHandle.value = instance as SettingsPageHandle | null;
          loaded.value = instance !== null;
        }
      },
    };
    pageHandle.value = null;
    loaded.value = false;
    pageError.value = '';
    navigationError.value = '';
    attempt.value = 0;
    activeVisit.value = visit;
    displayedVisit.value = visit;
  };

  const openPage = (id: SettingsPageId) => {
    if (!options.isOpen.value || activeVisit.value || id === 'developer') return;
    showPage(id);
    registerModal(SETTINGS_SUBPAGE_MODAL_ID, requestLeave, options.surface);
  };

  const openDeveloperPage = () => {
    if (!options.isOpen.value || activeVisit.value?.id !== 'about' || leaving.value) return;
    developerOpen = true;
    showPage('developer');
  };

  const failPage = (visitSerial: number, error: unknown) => {
    if (activeVisit.value?.serial !== visitSerial) return;
    if (pageHandle.value) {
      navigationError.value = error instanceof Error ? error.message : String(error);
      return;
    }
    pageError.value = error instanceof Error ? error.message : String(error);
    pageHandle.value = null;
    loaded.value = false;
  };

  const retryPage = () => {
    if (!activeVisit.value || leaving.value) return;
    pageError.value = '';
    pageHandle.value = null;
    loaded.value = false;
    attempt.value += 1;
  };

  const finishPageLeave = (visitSerial: number) => {
    if (displayedVisit.value?.serial === visitSerial && !activeVisit.value) {
      displayedVisit.value = null;
      loaded.value = false;
      pageError.value = '';
    }
  };

  const hide = () => {
    // Releasing a page never starts a save. Submitted work still belongs to its store.
    serial += 1;
    developerOpen = false;
    closePage();
    leavePromise = null;
  };

  watch(options.isOpen, (open) => {
    if (!open) hide();
    else {
      // An interrupted outer transition must not revive the previous page visit.
      displayedVisit.value = null;
      loaded.value = false;
      pageError.value = '';
    }
  }, { flush: 'sync' });

  onScopeDispose(hide);

  return {
    activeVisit,
    displayedVisit,
    pageError,
    navigationError,
    attempt,
    leaving,
    loading: computed(() => !!activeVisit.value && !pageHandle.value && !pageError.value),
    loaded,
    openPage,
    openDeveloperPage,
    requestLeave,
    failPage,
    retryPage,
    finishPageLeave,
  };
}

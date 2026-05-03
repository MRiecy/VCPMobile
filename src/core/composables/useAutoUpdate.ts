import { ref, watch } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { useAppLifecycleStore } from '../stores/appLifecycle';
import { useUpdateDownloader } from './useUpdateDownloader';

const LAST_CHECK_KEY = 'vcp_last_update_check';
const COOLDOWN_MS = 24 * 60 * 60 * 1000;

export interface UpdateInfo {
  hasUpdate: boolean;
  currentVersion: string;
  latestVersion: string;
  downloadUrl: string | null;
  releasePageUrl: string | null;
  releaseNotes: string | null;
  apkSize: number | null;
}

export function useAutoUpdate() {
  const lifecycleStore = useAppLifecycleStore();
  const { downloadAndInstall } = useUpdateDownloader();

  const isPromptOpen = ref(false);
  const updateInfo = ref<UpdateInfo | null>(null);
  const hasCheckedThisSession = ref(false);

  const shouldCheck = () => {
    const last = localStorage.getItem(LAST_CHECK_KEY);
    if (!last) return true;
    return Date.now() - parseInt(last, 10) > COOLDOWN_MS;
  };

  const performCheck = async () => {
    if (hasCheckedThisSession.value) return;
    hasCheckedThisSession.value = true;

    if (!shouldCheck()) {
      console.log('[AutoUpdate] Skipped: within 24h cooldown');
      return;
    }

    try {
      const info: UpdateInfo = await invoke('check_for_update');
      localStorage.setItem(LAST_CHECK_KEY, Date.now().toString());

      if (info.hasUpdate && info.downloadUrl) {
        updateInfo.value = info;
        isPromptOpen.value = true;
      }
    } catch (e) {
      console.error('[AutoUpdate] Check failed:', e);
      localStorage.setItem(LAST_CHECK_KEY, Date.now().toString());
    }
  };

  watch(
    () => lifecycleStore.state,
    (newState) => {
      if (newState === 'READY') {
        performCheck();
      }
    },
  );

  const handleConfirm = async () => {
    if (!updateInfo.value?.downloadUrl) return;
    isPromptOpen.value = false;
    try {
      await downloadAndInstall(updateInfo.value.downloadUrl);
    } catch {
      // error already handled by useUpdateDownloader
    }
  };

  const handleDismiss = () => {
    isPromptOpen.value = false;
  };

  return {
    isPromptOpen,
    updateInfo,
    handleConfirm,
    handleDismiss,
  };
}

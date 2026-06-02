import { ref, watch, computed } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { useAppLifecycleStore } from '../stores/appLifecycle';
import { useUpdateDownloader } from './useUpdateDownloader';
import { useUpdateStore } from '../stores/update';

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
  const updateStore = useUpdateStore();
  const { downloadAndInstall } = useUpdateDownloader();

  const isPromptOpen = computed({
    get: () => updateStore.isPromptOpen,
    set: (val) => { updateStore.isPromptOpen = val; }
  });

  const updateInfo = computed<UpdateInfo | null>({
    get: () => updateStore.updateInfo,
    set: (val) => { updateStore.updateInfo = val; }
  });

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
        updateStore.openPrompt(info);
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

    try {
      await downloadAndInstall(updateInfo.value.downloadUrl);
      updateStore.closePrompt();
    } catch {
      // 错误已由 useUpdateDownloader 记录并存入 store，此处不关闭弹窗
    }
  };

  const handleDismiss = () => {
    updateStore.closePrompt();
  };

  return {
    isPromptOpen,
    updateInfo,
    handleConfirm,
    handleDismiss,
  };
}

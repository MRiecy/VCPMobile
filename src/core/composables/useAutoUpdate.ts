import { watch, computed, onScopeDispose, ref } from 'vue';
import { useAppLifecycleStore } from '../stores/appLifecycle';
import { useUpdateDownloader } from './useUpdateDownloader';
import { useUpdateStore } from '../stores/update';
import { isAndroid } from '../utils/platform';
import { readLocalPreference, writeLocalPreference } from '../preferences/localPreference';

const LAST_CHECK_KEY = 'vcp_last_update_check';
const SKIPPED_VERSION_KEY = 'vcp_update_skipped_version';
const COOLDOWN_MS = 24 * 60 * 60 * 1000;

export function useAutoUpdate() {
  const lifecycleStore = useAppLifecycleStore();
  const updateStore = useUpdateStore();
  const { downloadAndInstall } = useUpdateDownloader();

  let checkTimer: ReturnType<typeof setTimeout> | null = null;

  const isPromptOpen = computed({
    get: () => updateStore.isPromptOpen,
    set: (val) => {
      if (val) updateStore.openPrompt();
      else updateStore.closePrompt();
    },
  });

  const updateInfo = computed(() => updateStore.info);

  const hasCheckedThisSession = ref(false);

  const shouldCheck = () => {
    const last = readLocalPreference(LAST_CHECK_KEY);
    if (!last.ok) {
      console.warn('[AutoUpdate] Failed to read last check:', last.error);
      return true;
    }
    if (last.raw === null) return true;
    const timestamp = Number(last.raw);
    if (!/^\d+$/.test(last.raw) || !Number.isSafeInteger(timestamp) || timestamp > Date.now()) {
      // Invalid metadata means no successful check, never a fabricated recent one.
      return true;
    }
    return Date.now() - timestamp > COOLDOWN_MS;
  };

  const performCheck = async () => {
    if (!isAndroid || lifecycleStore.state !== 'READY') return;
    if (hasCheckedThisSession.value) return;
    hasCheckedThisSession.value = true;

    if (!updateStore.autoCheckEnabled || !shouldCheck()) {
      return;
    }

    try {
      const status = await updateStore.check();
      // 冷却时间戳只在检查成功时写入，失败允许下次冷启动重试
      const saved = writeLocalPreference(LAST_CHECK_KEY, Date.now().toString());
      if (!saved.ok) console.warn('[AutoUpdate] Failed to save last check:', saved.error);

      const latest = status.info?.latestVersion;
      const hasUpdate =
        status.info?.hasUpdate &&
        (status.state === 'available' || status.state === 'readyToInstall');
      const skipped = readLocalPreference(SKIPPED_VERSION_KEY);
      if (!skipped.ok) console.warn('[AutoUpdate] Failed to read skipped version:', skipped.error);
      if (hasUpdate && latest && latest !== (skipped.ok ? skipped.raw : null)) {
        updateStore.openPrompt();
      }
    } catch (e) {
      console.error('[AutoUpdate] Check failed:', e);
    }
  };

  if (isAndroid) {
    watch(
      () => lifecycleStore.state,
      (newState) => {
        if (checkTimer) clearTimeout(checkTimer);
        checkTimer = null;
        // 仅 Android 订阅 OTA 状态；失败保留手动检查重试入口。
        void updateStore.init().catch((error) => console.warn('[AutoUpdate] Init failed:', error));
        if (newState === 'READY') {
          // 延迟 2s 执行，避开首屏渲染与聊天历史加载的 CPU 密集期
          checkTimer = setTimeout(() => {
            checkTimer = null;
            void performCheck();
          }, 2000);
        }
      },
      { immediate: true },
    );
  }

  onScopeDispose(() => {
    if (checkTimer) clearTimeout(checkTimer);
  });

  const handleConfirm = async () => {
    if (!isAndroid) return;
    if (updateStore.state === 'readyToInstall') {
      // 已下载完成：直接安装
      try {
        await updateStore.install();
        updateStore.closePrompt();
      } catch (err) {
        console.debug('[AutoUpdate] install failed:', err);
      }
      return;
    }
    try {
      await downloadAndInstall();
      updateStore.closePrompt();
    } catch (err) {
      // 错误已由状态机广播并存入 store，此处不关闭弹窗
      console.debug('[AutoUpdate] downloadAndInstall failed:', err);
    }
  };

  const handleDismiss = () => {
    updateStore.closePrompt();
  };

  /** 忽略当前版本：本次与后续自动检查都不再为该版本弹窗。 */
  const handleSkipVersion = () => {
    const latest = updateStore.info?.latestVersion;
    if (latest) {
      const saved = writeLocalPreference(SKIPPED_VERSION_KEY, latest);
      if (!saved.ok) console.warn('[AutoUpdate] Failed to save skipped version:', saved.error);
    }
    updateStore.closePrompt();
  };

  return {
    isPromptOpen,
    updateInfo,
    handleConfirm,
    handleDismiss,
    handleSkipVersion,
  };
}

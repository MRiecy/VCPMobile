import { watch, computed, ref } from 'vue';
import { useAppLifecycleStore } from '../stores/appLifecycle';
import { useUpdateDownloader } from './useUpdateDownloader';
import { useUpdateStore } from '../stores/update';

const LAST_CHECK_KEY = 'vcp_last_update_check';
const SKIPPED_VERSION_KEY = 'vcp_update_skipped_version';
const AUTO_CHECK_KEY = 'vcp_auto_update_check';
const COOLDOWN_MS = 24 * 60 * 60 * 1000;

export function useAutoUpdate() {
  const lifecycleStore = useAppLifecycleStore();
  const updateStore = useUpdateStore();
  const { downloadAndInstall } = useUpdateDownloader();

  // 幂等：订阅 Rust 状态广播并补拉当前快照
  updateStore.init();

  const isPromptOpen = computed({
    get: () => updateStore.isPromptOpen,
    set: (val) => {
      if (val) updateStore.openPrompt();
      else updateStore.closePrompt();
    },
  });

  const updateInfo = computed(() => updateStore.info);

  const hasCheckedThisSession = ref(false);

  const isAutoCheckEnabled = () => localStorage.getItem(AUTO_CHECK_KEY) !== 'off';

  const shouldCheck = () => {
    const last = localStorage.getItem(LAST_CHECK_KEY);
    if (!last) return true;
    return Date.now() - parseInt(last, 10) > COOLDOWN_MS;
  };

  const performCheck = async () => {
    if (hasCheckedThisSession.value) return;
    hasCheckedThisSession.value = true;

    if (!isAutoCheckEnabled() || !shouldCheck()) {
      return;
    }

    try {
      const status = await updateStore.check();
      // 冷却时间戳只在检查成功时写入，失败允许下次冷启动重试
      localStorage.setItem(LAST_CHECK_KEY, Date.now().toString());

      const latest = status.info?.latestVersion;
      const hasUpdate =
        status.info?.hasUpdate &&
        (status.state === 'available' || status.state === 'readyToInstall');
      if (hasUpdate && latest && latest !== localStorage.getItem(SKIPPED_VERSION_KEY)) {
        updateStore.openPrompt();
      }
    } catch (e) {
      console.error('[AutoUpdate] Check failed:', e);
    }
  };

  watch(
    () => lifecycleStore.state,
    (newState) => {
      if (newState === 'READY') {
        // 延迟 2s 执行，避开首屏渲染与聊天历史加载的 CPU 密集期
        setTimeout(() => performCheck(), 2000);
      }
    },
  );

  const handleConfirm = async () => {
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
      localStorage.setItem(SKIPPED_VERSION_KEY, latest);
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

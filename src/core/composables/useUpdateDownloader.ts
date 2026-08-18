import { computed, watch } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { useNotificationStore } from '../stores/notification';
import { useUpdateStore } from '../stores/update';

const formatBytes = (bytes: number) => {
  if (bytes === 0) return '0 B';
  const k = 1024;
  const sizes = ['B', 'KB', 'MB', 'GB'];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + ' ' + sizes[i];
};

/**
 * OTA 下载/安装编排器：消费 update store 的状态机，
 * 负责系统通知栏进度、Toast 反馈与下载完成后的安装接续。
 */
export function useUpdateDownloader() {
  const notificationStore = useNotificationStore();
  const updateStore = useUpdateStore();

  const isDownloading = computed(() =>
    ['downloading', 'verifying'].includes(updateStore.state),
  );

  const downloadAndInstall = async () => {
    if (updateStore.state === 'downloading' || updateStore.state === 'verifying') {
      console.log('[UpdateDownloader] Refused: download already in progress.');
      return;
    }

    const isAndroid = navigator.userAgent.toLowerCase().includes('android');

    notificationStore.addNotification({
      title: '发起更新下载',
      message: '已发起后台下载；允许通知后，进度也会显示在系统通知栏。',
      type: 'info',
      duration: 3500,
      toastOnly: true,
    });

    // Android 13+ 的通知权限只在用户真正开始 OTA 下载时申请。
    // 拒绝通知不阻断下载，应用内进度与系统安装器仍可正常工作。
    if (isAndroid) {
      await invoke('plugin:vcp-mobile|request_android_permission', { pType: 'notification' }).catch((e) => {
        console.warn('[UpdateDownloader] notification permission unavailable:', e);
      });
      await invoke('plugin:vcp-mobile|start_download_notification').catch((e) => {
        console.error('[UpdateDownloader] start_download_notification failed:', e);
      });
    }

    // 通知栏进度跟随 store 状态，300ms 节流避开 JNI 高频调用
    let lastNotifyTime = 0;
    const stopWatch = watch(
      () => [updateStore.status.downloaded, updateStore.status.total] as const,
      ([downloaded, total]) => {
        if (!isAndroid) return;
        const percent = total ? Math.round((downloaded / total) * 100) : 0;
        const text = total
          ? `已下载 ${percent}% (${formatBytes(downloaded)} / ${formatBytes(total)})`
          : `已下载 ${formatBytes(downloaded)}`;
        const now = Date.now();
        if (now - lastNotifyTime > 300 || percent === 100) {
          lastNotifyTime = now;
          invoke('plugin:vcp-mobile|update_download_notification', {
            progress: percent,
            text,
          }).catch((e) => {
            console.error('[UpdateDownloader] update_download_notification failed:', e);
          });
        }
      },
    );

    try {
      const downloaded = await updateStore.startDownload();

      if (downloaded.error || downloaded.state === 'failed') {
        throw new Error(downloaded.error?.message ?? '下载失败');
      }
      if (downloaded.state !== 'readyToInstall') {
        // 用户取消或其他中断：状态机已广播，静默退出
        return;
      }

      notificationStore.addNotification({
        title: '下载完成',
        message: '校验通过，正在拉起更新安装器...',
        type: 'success',
        duration: 3000,
        toastOnly: true,
      });

      const installed = await updateStore.install();
      if (installed.error) {
        throw new Error(installed.error.message);
      }

      notificationStore.addNotification({
        title: '安装器已唤起',
        message: '请在系统安装器中完成更新',
        type: 'success',
        duration: 5000,
        toastOnly: true,
      });
    } catch (e: any) {
      const errorString = String(e instanceof Error ? e.message : e);
      notificationStore.addNotification({
        title: '更新失败',
        message: errorString,
        type: 'error',
        duration: 8000,
        toastOnly: true,
      });
      throw e;
    } finally {
      stopWatch();
      if (isAndroid) {
        await invoke('plugin:vcp-mobile|cancel_download_notification').catch(() => {});
      }
    }
  };

  return {
    downloadAndInstall,
    isDownloading,
  };
}

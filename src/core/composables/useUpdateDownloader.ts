import { ref } from 'vue';
import { invoke, Channel } from '@tauri-apps/api/core';
import { useNotificationStore } from '../stores/notification';

interface DownloadProgress {
  downloaded: number;
  total: number | null;
}

const formatBytes = (bytes: number) => {
  if (bytes === 0) return '0 B';
  const k = 1024;
  const sizes = ['B', 'KB', 'MB', 'GB'];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + ' ' + sizes[i];
};

const DOWNLOAD_NOTIF_ID = 'vcp_update_download_progress';

export function useUpdateDownloader() {
  const notificationStore = useNotificationStore();
  const isDownloading = ref(false);

  const downloadAndInstall = async (url: string) => {
    if (isDownloading.value) return;
    isDownloading.value = true;

    try {
      const channel = new Channel<DownloadProgress>();
      channel.onmessage = (msg) => {
        const percent = msg.total ? Math.round((msg.downloaded / msg.total) * 100) : 0;
        notificationStore.addNotification({
          id: DOWNLOAD_NOTIF_ID,
          title: '下载更新中',
          message: msg.total
            ? `${percent}% (${formatBytes(msg.downloaded)} / ${formatBytes(msg.total)})`
            : `${formatBytes(msg.downloaded)}`,
          type: 'info',
          duration: 0,
          toastOnly: true,
        });
      };

      const apkPath = await invoke<string>('download_update', {
        url,
        onProgress: channel,
      });

      // 清除下载进度 toast
      notificationStore.activeToasts = notificationStore.activeToasts.filter(
        (t) => t.id !== DOWNLOAD_NOTIF_ID,
      );

      notificationStore.addNotification({
        title: '下载完成',
        message: '正在启动安装器...',
        type: 'success',
        duration: 3000,
        toastOnly: true,
      });

      await invoke('install_update', { apkPath });

      notificationStore.addNotification({
        title: '安装器已唤起',
        message: '请在系统安装器中完成更新',
        type: 'success',
        duration: 5000,
        toastOnly: true,
      });
    } catch (e: any) {
      notificationStore.activeToasts = notificationStore.activeToasts.filter(
        (t) => t.id !== DOWNLOAD_NOTIF_ID,
      );
      notificationStore.addNotification({
        title: '更新失败',
        message: String(e),
        type: 'error',
        duration: 8000,
        toastOnly: true,
      });
      throw e;
    } finally {
      isDownloading.value = false;
    }
  };

  return {
    downloadAndInstall,
    isDownloading,
  };
}

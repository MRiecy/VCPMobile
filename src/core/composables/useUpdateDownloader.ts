import { ref } from 'vue';
import { invoke, Channel } from '@tauri-apps/api/core';
import { useNotificationStore } from '../stores/notification';
import { useUpdateStore } from '../stores/update';

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

export function useUpdateDownloader() {
  const notificationStore = useNotificationStore();
  const updateStore = useUpdateStore();

  const downloadAndInstall = async (url: string) => {
    // 1. 单例前置硬锁定，防止多任务并发冲突
    if (updateStore.status === 'downloading') {
      console.log('[UpdateDownloader] Refused: download already in progress.');
      return;
    }

    updateStore.setStatus('downloading');
    updateStore.updateProgress(0, null);

    const isAndroid = navigator.userAgent.toLowerCase().includes('android');

    try {
      // 2. 优化：立即弹出起步 Toast，给用户明确反馈，防止物理链路延迟造成“无反应”错觉
      notificationStore.addNotification({
        title: '发起更新下载',
        message: '已发起后台下载，进度将在系统通知栏显示...',
        type: 'info',
        duration: 3500,
        toastOnly: true,
      });

      // 3. 在 Android 上拉起系统通知栏的 Ongoing 进度通知
      if (isAndroid) {
        await invoke('plugin:vcp-mobile|start_download_notification').catch((e) => {
          console.error('[UpdateDownloader] start_download_notification failed:', e);
        });
      }

      let lastNotifyTime = 0;

      const channel = new Channel<DownloadProgress>();
      channel.onmessage = (msg) => {
        // 更新全局 Pinia Store 状态，使关于页面 100% 走字同步
        updateStore.updateProgress(msg.downloaded, msg.total);

        // 计算当前下载百分比
        const percent = msg.total ? Math.round((msg.downloaded / msg.total) * 100) : 0;
        const text = msg.total
          ? `已下载 ${percent}% (${formatBytes(msg.downloaded)} / ${formatBytes(msg.total)})`
          : `已下载 ${formatBytes(msg.downloaded)}`;

        // 4. 通知栏更新的 300ms 节流策略，避开 JNI 高频调用性能损耗
        const now = Date.now();
        if (isAndroid && (now - lastNotifyTime > 300 || percent === 100)) {
          lastNotifyTime = now;
          invoke('plugin:vcp-mobile|update_download_notification', {
            progress: percent,
            text,
          }).catch((e) => {
            console.error('[UpdateDownloader] update_download_notification failed:', e);
          });
        }
      };

      const apkPath = await invoke<string>('download_update', {
        url,
        onProgress: channel,
      });

      // 5. 下载顺利完成，取消系统通知栏 Ongoing 通知
      if (isAndroid) {
        await invoke('plugin:vcp-mobile|cancel_download_notification').catch(() => {});
      }

      updateStore.setStatus('downloaded');

      notificationStore.addNotification({
        title: '下载完成',
        message: '正在拉起更新安装器...',
        type: 'success',
        duration: 3000,
        toastOnly: true,
      });

      updateStore.setStatus('installing');
      await invoke('install_update', { apkPath });

      notificationStore.addNotification({
        title: '安装器已唤起',
        message: '请在系统安装器中完成更新',
        type: 'success',
        duration: 5000,
        toastOnly: true,
      });

      updateStore.setStatus('idle');
    } catch (e: any) {
      // 出现异常，坚决销毁系统通知栏 Ongoing 通知，避免残留
      if (isAndroid) {
        await invoke('plugin:vcp-mobile|cancel_download_notification').catch(() => {});
      }

      const errorString = String(e);
      updateStore.setError(errorString);

      notificationStore.addNotification({
        title: '更新失败',
        message: errorString,
        type: 'error',
        duration: 8000,
        toastOnly: true,
      });
      throw e;
    }
  };

  return {
    downloadAndInstall,
    isDownloading: ref(updateStore.status === 'downloading'),
  };
}

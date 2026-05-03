import { defineStore } from 'pinia';
import { ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

export const useSyncSessionStore = defineStore('syncSession', () => {
  // --- 视图状态 ---
  const isOpen = ref(false);
  const canDismiss = ref(true);

  // --- 连接状态机 ---
  const status = ref<'idle' | 'connecting' | 'connected' | 'error' | 'completed'>('idle');

  // --- 面板视图 ---
  const activeTab = ref<'live' | 'history'>('live');

  // --- 同步完成后需刷新标志（once-set，不受断连等异常状态影响） ---
  const needsReload = ref(false);

  // --- 日志与进度 ---
  const logs = ref<{ level: string; message: string; time: string }[]>([]);
  const progressData = ref({ phase: 'initialization', total: 0, completed: 0, message: '' });

  // --- 监听器引用 ---
  let unlistenFns: UnlistenFn[] = [];

  const open = () => {
    isOpen.value = true;
    canDismiss.value = true;
    status.value = 'idle';
    activeTab.value = 'live';
    logs.value = [];
    progressData.value = { phase: 'initialization', total: 0, completed: 0, message: '' };
    registerListeners();
  };

  const startSync = () => {
    if (status.value !== 'idle') return;
    status.value = 'connecting';
    logs.value = [];
    progressData.value = { phase: 'initialization', total: 0, completed: 0, message: '' };
    invoke('start_manual_sync').catch((e: any) => {
      pushLog('error', `启动失败: ${e}`);
      status.value = 'error';
      canDismiss.value = true;
    });
  };

  const close = () => {
    if (!canDismiss.value) return;
    isOpen.value = false;
    activeTab.value = 'live';
    cleanupListeners();
  };

  const copyLogs = async () => {
    try {
      const { invoke } = await import('@tauri-apps/api/core');
      const files = await invoke<Array<{ filename: string }>>('list_sync_log_files');
      if (files && files.length > 0) {
        const content = await invoke<string>('read_sync_log_file', { filename: files[0].filename });
        await navigator.clipboard.writeText(content);
        pushLog('success', '完整日志已复制到剪贴板');
      } else {
        const text = logs.value.map(l => `[${l.time}] ${l.message}`).join('\n');
        await navigator.clipboard.writeText(text);
        pushLog('success', '会话日志已复制到剪贴板');
      }
    } catch (e: any) {
      pushLog('error', `复制失败: ${e}`);
    }
  };

  const registerListeners = () => {
    cleanupListeners();
    listen('vcp-log', (event: any) => {
      const { level, category, message } = event.payload;
      if (category === 'sync') pushLog(level || 'info', message);
    }).then(fn => unlistenFns.push(fn));

    listen('vcp-sync-progress', (event: any) => {
      progressData.value = event.payload;
    }).then(fn => unlistenFns.push(fn));

    listen('vcp-sync-status', (event: any) => {
      const s = event.payload.status;
      if (s === 'open') { status.value = 'connected'; canDismiss.value = false; }
      if (s === 'error') { status.value = 'error'; canDismiss.value = true; }
    }).then(fn => unlistenFns.push(fn));

    listen('vcp-sync-completed', () => {
      status.value = 'completed';
      canDismiss.value = true;
      needsReload.value = true;
      pushLog('success', '同步已全部完成，点击关闭以刷新数据');
    }).then(fn => unlistenFns.push(fn));
  };

  const cleanupListeners = () => {
    unlistenFns.forEach(fn => fn());
    unlistenFns = [];
  };

  const pushLog = (level: string, message: string) => {
    logs.value.push({ level, message, time: new Date().toLocaleTimeString() });
    if (logs.value.length > 200) logs.value.shift();
  };

  const markReloaded = () => {
    needsReload.value = false;
  };

  const switchTab = (tab: 'live' | 'history') => {
    if (status.value === 'connected') return;
    activeTab.value = tab;
  };

  return {
    isOpen,
    canDismiss,
    status,
    needsReload,
    logs,
    progressData,
    activeTab,
    open,
    close,
    startSync,
    copyLogs,
    markReloaded,
    switchTab,
  };
});

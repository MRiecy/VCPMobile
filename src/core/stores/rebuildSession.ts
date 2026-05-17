import { defineStore } from 'pinia';
import { ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

export type RebuildTaskType = 'preRender' | 'contentCompress' | 'dbPageSizeUpgrade';

export const useRebuildSessionStore = defineStore('rebuildSession', () => {
  // --- 视图状态 ---
  const isOpen = ref(false);
  const canDismiss = ref(true);

  // --- 任务类型 ---
  const taskType = ref<RebuildTaskType>('preRender');

  // --- 状态机 ---
  const status = ref<'idle' | 'running' | 'completed' | 'error'>('idle');

  // --- 进度 ---
  const progress = ref({ current: 0, total: 0 });

  // --- 完成后需刷新标志 ---
  const needsReload = ref(false);

  // --- 错误信息 ---
  const errorMessage = ref('');

  // --- 监听器引用 ---
  let unlistenFn: UnlistenFn | null = null;

  const open = (type: RebuildTaskType = 'preRender') => {
    taskType.value = type;
    isOpen.value = true;
    canDismiss.value = true;
    status.value = 'idle';
    progress.value = { current: 0, total: 0 };
    needsReload.value = false;
    errorMessage.value = '';
    registerListener();
  };

  const startRebuild = async () => {
    if (status.value !== 'idle') return;
    status.value = 'running';
    canDismiss.value = false;
    progress.value = { current: 0, total: 0 };
    errorMessage.value = '';

    try {
      if (taskType.value === 'preRender') {
        await invoke('rebuild_all_pre_renders');
        needsReload.value = true;
      } else if (taskType.value === 'contentCompress') {
        await invoke('compress_all_contents');
        needsReload.value = true;
      } else if (taskType.value === 'dbPageSizeUpgrade') {
        await invoke('upgrade_database_page_size');
        needsReload.value = false;
      }
      status.value = 'completed';
      canDismiss.value = true;
    } catch (e: any) {
      console.error(`[RebuildSession] ${taskType.value} failed:`, e);
      const msg = typeof e === 'string' ? e : (e?.message ?? String(e));
      errorMessage.value = msg;
      status.value = 'error';
      canDismiss.value = true;
    }
  };

  const close = () => {
    if (!canDismiss.value) return;
    isOpen.value = false;
    cleanupListener();
  };

  const markReloaded = () => {
    needsReload.value = false;
  };

  const registerListener = () => {
    cleanupListener();
    const eventName = taskType.value === 'preRender'
      ? 'render_rebuild_progress'
      : 'content_compress_progress';
    listen<{ current: number; total: number }>(eventName, (event) => {
      progress.value = event.payload;
    }).then((fn) => {
      unlistenFn = fn;
    });
  };

  const cleanupListener = () => {
    if (unlistenFn) {
      unlistenFn();
      unlistenFn = null;
    }
  };

  return {
    isOpen,
    canDismiss,
    taskType,
    status,
    progress,
    needsReload,
    errorMessage,
    open,
    close,
    startRebuild,
    markReloaded,
  };
});

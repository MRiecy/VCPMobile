import { defineStore } from 'pinia';
import { ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

export const useRebuildSessionStore = defineStore('rebuildSession', () => {
  // --- 视图状态 ---
  const isOpen = ref(false);
  const canDismiss = ref(true);

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

  const open = () => {
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
      await invoke('rebuild_all_pre_renders');
      status.value = 'completed';
      canDismiss.value = true;
      needsReload.value = true;
    } catch (e: any) {
      console.error('[RebuildSession] rebuild_all_pre_renders failed:', e);
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
    listen<{ current: number; total: number }>('render_rebuild_progress', (event) => {
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

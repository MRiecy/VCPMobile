import { defineStore } from 'pinia';
import { ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { withScreenKeep } from '../composables/useScreenKeeper';

export type RebuildTaskType = 'preRender';

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

    // 启动前台保活服务，显示“预渲染重建”通知
    invoke('plugin:vcp-mobile|start_streaming_service', { agentName: '[预渲染重建] VCP Mobile' }).catch(() => {});

    try {
      await withScreenKeep(() => invoke('rebuild_all_pre_renders'));
      needsReload.value = true;
      status.value = 'completed';
      canDismiss.value = true;
    } catch (e: any) {
      console.error(`[RebuildSession] ${taskType.value} failed:`, e);
      const msg = typeof e === 'string' ? e : (e?.message ?? String(e));
      errorMessage.value = msg;
      status.value = 'error';
      canDismiss.value = true;
    } finally {
      // 无论成功还是失败，均安全释放前台服务
      invoke('plugin:vcp-mobile|stop_streaming_service', { agentName: '[预渲染重建] VCP Mobile' }).catch(() => {});
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

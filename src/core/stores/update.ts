import { defineStore } from 'pinia';
import { computed, onScopeDispose, ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { isAndroid } from '../utils/platform';
import { readLocalPreference, writeLocalPreference } from '../preferences/localPreference';
import {
  UPDATE_STATUS_EVENT,
  type UpdateState,
  type UpdateStatus,
} from '../types/update';

const IDLE_STATUS: UpdateStatus = {
  state: 'idle',
  info: null,
  downloaded: 0,
  total: null,
  error: null,
};

const AUTO_CHECK_KEY = 'vcp_auto_update_check';
const AUTO_CHECK_DEFAULT = true;

export type AutoCheckPreferenceResult =
  | { ok: true; changed: boolean; enabled: boolean }
  | { ok: false; changed: false; enabled: boolean; error: string };

const readAutoCheckEnabled = (): boolean => {
  const result = readLocalPreference(AUTO_CHECK_KEY);
  if (!result.ok) {
    console.warn('[UpdateStore] Failed to read auto-check preference:', result.error);
    return AUTO_CHECK_DEFAULT;
  }
  if (result.raw === 'off') return false;
  if (result.raw === 'on' || result.raw === null) return AUTO_CHECK_DEFAULT;
  const repaired = writeLocalPreference(AUTO_CHECK_KEY, AUTO_CHECK_DEFAULT ? 'on' : 'off');
  if (!repaired.ok) console.warn('[UpdateStore] Failed to repair auto-check preference:', repaired.error);
  return AUTO_CHECK_DEFAULT;
};

/**
 * OTA 更新状态的唯一前端入口。
 *
 * OTA 状态由 Rust `UpdateSession` 持有，本 store 只做镜像：
 * - `init()` 订阅 `vcp-update://status` 事件并补拉一次当前状态；
 * - 所有命令返回最新快照并直接落库，事件广播保证其他入口同步。
 * 自动检查偏好独立保存在本地，不写入 Rust 更新状态。
 */
export const useUpdateStore = defineStore('update', () => {
  const status = ref<UpdateStatus>({ ...IDLE_STATUS });
  const autoCheckPreference = ref(readAutoCheckEnabled());
  const autoCheckEnabled = computed(() => autoCheckPreference.value);
  const isPromptOpen = ref(false);
  let initialized = false;
  let initPromise: Promise<void> | null = null;
  let unlistenStatus: (() => void) | null = null;
  let disposed = false;

  const requireUpdater = () => {
    if (!isAndroid) throw new Error('应用内 APK 更新仅适用于 Android');
    if (disposed) throw new Error('更新状态已释放');
  };

  onScopeDispose(() => {
    disposed = true;
    unlistenStatus?.();
    unlistenStatus = null;
  });

  const state = computed<UpdateState>(() => status.value.state);
  const info = computed(() => status.value.info);
  const error = computed(() => status.value.error);
  const isBusy = computed(() =>
    ['checking', 'downloading', 'verifying', 'installing'].includes(status.value.state),
  );
  const progressPercent = computed(() => {
    const { downloaded, total } = status.value;
    if (!total || total <= 0) return 0;
    return Math.min(100, Math.round((downloaded / total) * 100));
  });

  const applyStatus = (next: UpdateStatus) => {
    status.value = next;
  };

  const setAutoCheckEnabled = (enabled: boolean): AutoCheckPreferenceResult => {
    if (enabled === autoCheckPreference.value) {
      return { ok: true, changed: false, enabled };
    }
    const result = writeLocalPreference(AUTO_CHECK_KEY, enabled ? 'on' : 'off');
    if (!result.ok) {
      return { ok: false, changed: false, enabled: autoCheckPreference.value, error: result.error };
    }
    autoCheckPreference.value = enabled;
    return { ok: true, changed: true, enabled };
  };

  /** 幂等初始化：订阅 Rust 状态广播并补拉当前快照。 */
  const init = async () => {
    requireUpdater();
    if (initialized) return;
    if (initPromise) return initPromise;
    initPromise = (async () => {
      const release = await listen<UpdateStatus>(UPDATE_STATUS_EVENT, (event) => {
        if (!disposed) applyStatus(event.payload);
      });
      if (disposed) {
        release();
        throw new Error('更新状态已释放');
      }
      unlistenStatus = release;
      try {
        const snapshot = await invoke<UpdateStatus>('get_update_status');
        requireUpdater();
        applyStatus(snapshot);
        initialized = true;
      } catch (error) {
        unlistenStatus?.();
        unlistenStatus = null;
        throw error;
      }
    })();
    try {
      await initPromise;
    } finally {
      initPromise = null;
    }
  };

  const check = async (): Promise<UpdateStatus> => {
    requireUpdater();
    await init();
    const next = await invoke<UpdateStatus>('check_for_update');
    applyStatus(next);
    return next;
  };

  const startDownload = async (): Promise<UpdateStatus> => {
    requireUpdater();
    await init();
    const next = await invoke<UpdateStatus>('start_update_download');
    applyStatus(next);
    return next;
  };

  const cancelDownload = async (): Promise<UpdateStatus> => {
    requireUpdater();
    const next = await invoke<UpdateStatus>('cancel_update_download');
    applyStatus(next);
    return next;
  };

  const install = async (): Promise<UpdateStatus> => {
    requireUpdater();
    const next = await invoke<UpdateStatus>('install_update');
    applyStatus(next);
    return next;
  };

  const openPrompt = () => {
    requireUpdater();
    isPromptOpen.value = true;
  };

  const closePrompt = () => {
    isPromptOpen.value = false;
  };

  return {
    status,
    autoCheckEnabled,
    setAutoCheckEnabled,
    state,
    info,
    error,
    isBusy,
    progressPercent,
    isPromptOpen,
    init,
    check,
    startDownload,
    cancelDownload,
    install,
    openPrompt,
    closePrompt,
  };
});

import { defineStore } from 'pinia';
import { ref } from 'vue';

export type UpdateStatus = 'idle' | 'checking' | 'downloading' | 'downloaded' | 'installing' | 'error';

export const useUpdateStore = defineStore('update', () => {
  const status = ref<UpdateStatus>('idle');
  const downloadProgress = ref(0);
  const downloadTotal = ref<number | null>(null);
  const errorMsg = ref('');

  // 弹窗共享状态与真实数据
  const isPromptOpen = ref(false);
  const updateInfo = ref<any | null>(null);
  const latestVersion = ref('');

  const openPrompt = (info: any) => {
    updateInfo.value = info;
    latestVersion.value = info?.latestVersion || '';
    isPromptOpen.value = true;
  };

  const closePrompt = () => {
    isPromptOpen.value = false;
  };

  const setStatus = (newStatus: UpdateStatus) => {
    status.value = newStatus;
  };

  const updateProgress = (loaded: number, total: number | null) => {
    downloadProgress.value = loaded;
    downloadTotal.value = total;
  };

  const setError = (msg: string) => {
    status.value = 'error';
    errorMsg.value = msg;
  };

  const reset = () => {
    status.value = 'idle';
    downloadProgress.value = 0;
    downloadTotal.value = null;
    errorMsg.value = '';
    latestVersion.value = '';
  };

  return {
    status,
    downloadProgress,
    downloadTotal,
    errorMsg,
    isPromptOpen,
    updateInfo,
    latestVersion,
    openPrompt,
    closePrompt,
    setStatus,
    updateProgress,
    setError,
    reset,
  };
});

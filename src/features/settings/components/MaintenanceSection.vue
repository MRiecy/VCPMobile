<script setup lang="ts">
import { ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import SettingsActionWithStatus from '../../../components/settings/SettingsActionWithStatus.vue';

const cleanupStatus = ref<{ type: 'success' | 'error' | 'loading' | null; message: string }>({ type: null, message: '' });

const cleanupAttachments = async () => {
  cleanupStatus.value = { type: 'loading', message: '正在深度扫描孤儿附件...' };
  try {
    const result = await invoke<string>('cleanup_orphaned_attachments');
    cleanupStatus.value = { type: 'success', message: result };
    setTimeout(() => { cleanupStatus.value = { type: null, message: '' }; }, 5000);
  } catch (e: any) {
    cleanupStatus.value = { type: 'error', message: `清理失败: ${e}` };
  }
};
</script>

<template>
  <div class="space-y-2">
    <SettingsActionWithStatus
      title="附件库垃圾回收 (GC)"
      description="深度扫描并删除未被引用的孤立附件与缩略图"
      button-variant="danger"
      button-size="sm"
      button-label="立即清理"
      :button-loading="cleanupStatus.type === 'loading'"
      :status-type="cleanupStatus.type"
      :status-message="cleanupStatus.message"
      status-mono
      @action-click="cleanupAttachments"
    />
  </div>
</template>

<script setup lang="ts">
import { ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import SettingsActionWithStatus from '../../../components/settings/SettingsActionWithStatus.vue';
import { useOverlayStore } from '../../../core/stores/overlay';

const overlayStore = useOverlayStore();

const cleanupStatus = ref<{ type: 'success' | 'error' | 'loading' | null; message: string }>({ type: null, message: '' });

const cleanupAttachments = async () => {
  cleanupStatus.value = { type: 'loading', message: '正在深度扫描孤儿附件...' };
  try {
    const result = await invoke<string>('cleanup_orphaned_attachments');
    cleanupStatus.value = { type: 'success', message: result };
    setTimeout(() => { cleanupStatus.value = { type: null, message: '' }; }, 5000);
  } catch (e: any) {
    console.error('[Maintenance] cleanup_orphaned_attachments failed:', e);
    const msg = typeof e === 'string' ? e : (e?.message ?? String(e));
    cleanupStatus.value = { type: 'error', message: `清理失败: ${msg}` };
  }
};

const openRebuildSession = () => {
  overlayStore.openRebuildSession();
};
</script>

<template>
  <div class="space-y-6">
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

    <div class="pt-4 border-t border-white/5">
      <SettingsActionWithStatus
        title="全量预渲染重建"
        description="对数据库中所有历史消息进行高性能 AST 重新解析与代码高亮固化"
        button-variant="primary"
        button-size="sm"
        button-label="一键重建"
        status-mono
        @action-click="openRebuildSession"
      />
    </div>
  </div>
</template>

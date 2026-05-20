<script setup lang="ts">
import { ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import SettingsActionWithStatus from '../../../components/settings/SettingsActionWithStatus.vue';
import { useOverlayStore } from '../../../core/stores/overlay';
import { withScreenKeep } from '../../../core/composables/useScreenKeeper';

const overlayStore = useOverlayStore();

const gcStatus = ref<{ type: 'success' | 'error' | 'loading' | null; message: string }>({ type: null, message: '' });
const cacheStatus = ref<{ type: 'success' | 'error' | 'loading' | null; message: string }>({ type: null, message: '' });

const cleanupAttachments = async () => {
  gcStatus.value = { type: 'loading', message: '正在深度扫描孤儿附件...' };
  try {
    const result = await withScreenKeep(() => invoke<string>('cleanup_orphaned_attachments'));
    gcStatus.value = { type: 'success', message: result };
    setTimeout(() => { gcStatus.value = { type: null, message: '' }; }, 5000);
  } catch (e: any) {
    console.error('[Maintenance] cleanup_orphaned_attachments failed:', e);
    const msg = typeof e === 'string' ? e : (e?.message ?? String(e));
    gcStatus.value = { type: 'error', message: `清理失败: ${msg}` };
  }
};

const clearSystemCache = async () => {
  cacheStatus.value = { type: 'loading', message: '正在清理系统与 WebView 缓存...' };
  try {
    const result = await withScreenKeep(() => invoke<string>('clear_webview_cache'));
    cacheStatus.value = { type: 'success', message: result };
    setTimeout(() => { cacheStatus.value = { type: null, message: '' }; }, 5000);
  } catch (e: any) {
    console.error('[Maintenance] clear_webview_cache failed:', e);
    const msg = typeof e === 'string' ? e : (e?.message ?? String(e));
    cacheStatus.value = { type: 'error', message: `清理失败: ${msg}` };
  }
};

const openRebuildSession = () => {
  overlayStore.openRebuildSession('preRender');
};

const openCompressSession = () => {
  overlayStore.openRebuildSession('contentCompress');
};

const openDbPageSizeUpgrade = () => {
  overlayStore.openRebuildSession('dbPageSizeUpgrade');
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
      :button-loading="gcStatus.type === 'loading'"
      :status-type="gcStatus.type"
      :status-message="gcStatus.message"
      status-mono
      @action-click="cleanupAttachments"
    />

    <div class="pt-4 border-t border-white/5">
      <SettingsActionWithStatus
        title="清理系统缓存 (System Cache)"
        description="清除 WebView 内部 HTTP/图片缓存（解决磁盘空间异常占用）"
        button-variant="primary"
        button-size="sm"
        button-label="立即清理"
        :button-loading="cacheStatus.type === 'loading'"
        :status-type="cacheStatus.type"
        :status-message="cacheStatus.message"
        status-mono
        @action-click="clearSystemCache"
      />
    </div>

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

    <div class="pt-4 border-t border-white/5">
      <SettingsActionWithStatus
        title="全量消息内容压缩"
        description="将数据库中所有未压缩的历史消息文本进行 zstd 压缩，降低存储占用"
        button-variant="primary"
        button-size="sm"
        button-label="开始压缩"
        status-mono
        @action-click="openCompressSession"
      />
    </div>

    <div class="pt-4 border-t border-white/5">
      <SettingsActionWithStatus
        title="数据库 page_size 优化"
        description="将数据库存储页从 4KB 升级至 16KB，提升闪存 I/O 效率（执行 VACUUM）"
        button-variant="primary"
        button-size="sm"
        button-label="开始优化"
        status-mono
        @action-click="openDbPageSizeUpgrade"
      />
    </div>
  </div>
</template>

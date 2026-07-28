<script setup lang="ts">
import { ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import SettingsActionWithStatus from '../../../components/settings/SettingsActionWithStatus.vue';
import { useOverlayStore } from '../../../core/stores/overlay';
import { withScreenKeep } from '../../../core/composables/useScreenKeeper';

const overlayStore = useOverlayStore();

const gcStatus = ref<{ type: 'success' | 'error' | 'loading' | null; message: string }>({ type: null, message: '' });
const cacheStatus = ref<{ type: 'success' | 'error' | 'loading' | null; message: string }>({ type: null, message: '' });
const diagnosticStatus = ref<{ type: 'success' | 'error' | 'loading' | null; message: string }>({ type: null, message: '' });

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

const exportDiagnostics = async () => {
  diagnosticStatus.value = { type: 'loading', message: '正在收集崩溃、内存与运行日志...' };
  try {
    const path = await invoke<string>('export_runtime_diagnostics');
    if (/Android/i.test(navigator.userAgent)) {
      try {
        await invoke('plugin:vcp-mobile|share_file_native', {
          path,
          title: '分享 VCP Mobile 诊断包',
        });
      } catch (shareError) {
        console.warn('[Maintenance] Native diagnostic sharing failed, falling back to open_file:', shareError);
        await invoke('open_file', { path });
      }
    } else {
      await invoke('open_file', { path });
    }
    diagnosticStatus.value = { type: 'success', message: '诊断包已生成，可通过系统面板保存或分享。' };
    setTimeout(() => { diagnosticStatus.value = { type: null, message: '' }; }, 8000);
  } catch (e: any) {
    console.error('[Maintenance] export_runtime_diagnostics failed:', e);
    const msg = typeof e === 'string' ? e : (e?.message ?? String(e));
    diagnosticStatus.value = { type: 'error', message: `导出失败: ${msg}` };
  }
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
      status-multiline
      @action-click="cleanupAttachments"
    />

    <div class="pt-4 border-t border-black/5 dark:border-white/5">
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
        status-multiline
        @action-click="clearSystemCache"
      />
    </div>

    <div class="pt-4 border-t border-black/5 dark:border-white/5">
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

    <div class="pt-4 border-t border-black/5 dark:border-white/5">
      <SettingsActionWithStatus
        title="导出闪退诊断包"
        description="收集前端异常、Rust panic、Android 上次退出原因与最近运行日志；不含聊天数据库，但可能包含错误现场文本"
        button-variant="primary"
        button-size="sm"
        button-label="生成并分享"
        :button-loading="diagnosticStatus.type === 'loading'"
        :status-type="diagnosticStatus.type"
        :status-message="diagnosticStatus.message"
        status-mono
        status-multiline
        @action-click="exportDiagnostics"
      />
    </div>
  </div>
</template>

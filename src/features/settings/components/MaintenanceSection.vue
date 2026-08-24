<script setup lang="ts">
import { ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import SettingsActionWithStatus from '../../../components/settings/SettingsActionWithStatus.vue';
import { useOverlayStore } from '../../../core/stores/overlay';


const overlayStore = useOverlayStore();

const cacheStatus = ref<{ type: 'success' | 'error' | 'loading' | null; message: string }>({ type: null, message: '' });

const clearSystemCache = async () => {
  cacheStatus.value = { type: 'loading', message: '正在清理 WebView HTTP 缓存...' };
  try {
    const result = await invoke<string>('clear_webview_cache');
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
</script>

<template>
  <div class="space-y-6">
    <div>
      <SettingsActionWithStatus
        title="清理 WebView 缓存"
        description="清除 WebView HTTP 缓存（解决磁盘空间异常占用）"
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
        title="刷新现有预渲染缓存"
        description="重新解析已有缓存项的 AST 与代码高亮"
        button-variant="primary"
        button-size="sm"
        button-label="开始刷新"
        status-mono
        @action-click="openRebuildSession"
      />
    </div>
  </div>
</template>

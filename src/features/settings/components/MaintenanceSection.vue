<script setup lang="ts">
import { ref, onUnmounted, watch } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import SettingsActionWithStatus from '../../../components/settings/SettingsActionWithStatus.vue';
import { useOverlayStore } from '../../../core/stores/overlay';
import { isAndroid, isIOS } from '../../../core/utils/platform';


const props = withDefaults(defineProps<{ active?: boolean; disabled?: boolean }>(), { active: true });
const overlayStore = useOverlayStore();

const cacheStatus = ref<{ type: 'success' | 'error' | 'loading' | 'info' | null; message: string }>({ type: null, message: '' });
let disposed = false;
let clearStatusTimer: ReturnType<typeof setTimeout> | undefined;
onUnmounted(() => { disposed = true; clearTimeout(clearStatusTimer); });
watch(() => props.active, (active) => {
  if (!active) {
    disposed = true;
    clearTimeout(clearStatusTimer);
  }
}, { flush: 'sync' });

const clearSystemCache = async () => {
  if (disposed || !props.active || props.disabled || cacheStatus.value.type === 'loading') return;
  clearTimeout(clearStatusTimer);
  if (!isAndroid && !isIOS) {
    cacheStatus.value = {
      type: 'info',
      message: '当前预览环境不提供原生 WebView 缓存清理。',
    };
    return;
  }
  cacheStatus.value = { type: 'loading', message: isIOS ? '正在清理 WebKit 磁盘与内存浏览缓存...' : '正在清理 WebView HTTP 缓存...' };
  try {
    const result = await invoke<string>('clear_webview_cache');
    if (disposed) return;
    cacheStatus.value = { type: 'success', message: result };
    clearStatusTimer = setTimeout(() => { cacheStatus.value = { type: null, message: '' }; }, 5000);
  } catch (e: any) {
    if (disposed) return;
    console.error('[Maintenance] clear_webview_cache failed:', e);
    const msg = typeof e === 'string' ? e : (e?.message ?? String(e));
    cacheStatus.value = { type: 'error', message: `清理失败: ${msg}` };
  }
};

const openRebuildSession = () => {
  if (disposed || !props.active || props.disabled) return;
  overlayStore.openRebuildSession('preRender');
};
</script>

<template>
  <div class="space-y-6">
    <div>
      <SettingsActionWithStatus
        title="清理 WebView 缓存"
        :description="isAndroid ? '清除 WebView HTTP 缓存（解决磁盘空间异常占用）' : isIOS ? '清理磁盘与内存浏览缓存；聊天、附件与设置保留' : '原生 WebView 缓存清理需在移动端使用'"
        button-variant="primary"
        button-size="sm"
        :button-label="isAndroid || isIOS ? '立即清理' : '查看说明'"
        :button-loading="cacheStatus.type === 'loading'"
        :button-disabled="disabled"
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
        :button-disabled="disabled"
        status-mono
        @action-click="openRebuildSession"
      />
    </div>
  </div>
</template>

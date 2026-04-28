<script setup lang="ts">
import { useAppLifecycleStore } from '../../core/stores/appLifecycle';
import SyncProgressOverlay from './SyncProgressOverlay.vue';

const lifecycleStore = useAppLifecycleStore();

const reloadApp = () => {
  window.location.reload();
};
</script>

<template>
  <!-- 0. 全局初始化加载层 (通用) -->
  <Transition name="fade">
    <div v-if="lifecycleStore.state !== 'READY' && lifecycleStore.state !== 'ERROR' && lifecycleStore.state !== 'INITIAL_SYNCING'"
      class="fixed inset-0 z-[1000] bg-white/96 dark:bg-gray-950/96 backdrop-blur-md flex flex-col items-center justify-center gap-6 px-8 text-center">
      <div class="w-18 h-18 relative">
        <div class="absolute inset-0 rounded-full border-4 border-blue-500/15"></div>
        <div
          class="absolute inset-0 rounded-full border-4 border-transparent border-t-blue-500 border-r-cyan-400 animate-spin">
        </div>
      </div>
      <div class="flex flex-col items-center gap-2 max-w-xs">
        <p class="text-[11px] font-black tracking-[0.45em] text-blue-500/80 pl-[0.45em]">VCP MOBILE</p>
        <h2 class="text-2xl font-black tracking-tight text-primary-text">{{ lifecycleStore.statusText }}</h2>
        <p class="text-sm opacity-70 leading-6">{{ lifecycleStore.currentPhaseLabel }}</p>
        <p class="text-[10px] opacity-45 font-mono uppercase tracking-[0.3em]">{{ lifecycleStore.state }}</p>
      </div>
    </div>
  </Transition>

  <!-- 0.1 首次大数据同步专用加载层 (Neural Sync) -->
  <SyncProgressOverlay />

  <!-- 0.5 全局错误看板 -->
  <Transition name="fade">
    <div v-if="lifecycleStore.state === 'ERROR'"
      class="fixed inset-0 z-[1001] bg-white/98 dark:bg-gray-950/98 backdrop-blur-md flex flex-col items-center justify-center p-8 text-center">
      <div
        class="w-full max-w-md rounded-3xl border border-red-500/20 bg-white/80 dark:bg-white/5 shadow-2xl shadow-red-500/10 px-6 py-8 flex flex-col items-center">
        <div class="w-16 h-16 bg-red-500/10 text-red-500 rounded-2xl flex items-center justify-center mb-6">
          <svg width="32" height="32" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
            <circle cx="12" cy="12" r="10"></circle>
            <line x1="12" y1="8" x2="12" y2="12"></line>
            <line x1="12" y1="16" x2="12.01" y2="16"></line>
          </svg>
        </div>
        <p class="text-[11px] font-black tracking-[0.35em] text-red-500/80 pl-[0.35em] mb-2">LIFECYCLE ERROR</p>
        <h2 class="text-2xl font-black mb-3">核心启动失败</h2>
        <p class="text-sm opacity-70 leading-6 mb-2">生命周期入口未能完成初始化，应用已进入保护态。</p>
        <p class="text-xs opacity-60 mb-8 max-w-xs break-all">{{ lifecycleStore.errorMsg || '未知错误' }}</p>
        <button @click="reloadApp()"
          class="px-8 py-3 bg-blue-500 text-white rounded-xl font-bold shadow-lg shadow-blue-500/20 active:scale-95 transition-all">
          重试启动
        </button>
      </div>
    </div>
  </Transition>
</template>

<style scoped>
.fade-enter-active,
.fade-leave-active {
  transition: opacity 0.3s ease;
}

.fade-enter-from,
.fade-leave-to {
  opacity: 0;
}
</style>

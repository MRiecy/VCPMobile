<script setup lang="ts">
import { useAppLifecycleStore } from '../../core/stores/appLifecycle';
import { invoke } from '@tauri-apps/api/core';

const lifecycleStore = useAppLifecycleStore();

const reloadApp = () => {
  window.location.reload();
};

let isRestarting = false;
const confirmRestart = async () => {
  if (isRestarting) return;
  isRestarting = true;
  try {
    await invoke('restart_or_exit_app');
  } catch (e) {
    console.error('Failed to restart app:', e);
    isRestarting = false;
  }
};
</script>

<template>
  <!-- 0. 全局初始化加载层 (通用) -->
  <Transition name="fade">
    <div v-if="lifecycleStore.state !== 'READY' && lifecycleStore.state !== 'ERROR' && lifecycleStore.state !== 'MIGRATED' && lifecycleStore.state !== 'MIGRATING'"
      class="fixed inset-0 z-boot bg-white/96 dark:bg-gray-950/96 flex flex-col items-center justify-center gap-6 px-8 text-center">
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

  <!-- 0.3 数据库迁移进度展示层 -->
  <Transition name="fade">
    <div v-if="lifecycleStore.state === 'MIGRATING'"
      class="fixed inset-0 z-boot bg-white/96 dark:bg-gray-950/96 flex flex-col items-center justify-center gap-6 px-8 text-center">
      <div class="w-18 h-18 relative">
        <div class="absolute inset-0 rounded-full border-4 border-blue-500/15"></div>
        <div
          class="absolute inset-0 rounded-full border-4 border-transparent border-t-blue-500 border-r-cyan-400 animate-spin">
        </div>
      </div>
      <div class="flex flex-col items-center gap-2 max-w-xs">
        <p class="text-[11px] font-black tracking-[0.45em] text-blue-500/80 pl-[0.45em]">DATABASE MIGRATION</p>
        <h2 class="text-2xl font-black tracking-tight text-primary-text">升级数据库中...</h2>
        <p class="text-sm opacity-70 leading-6">{{ lifecycleStore.currentPhaseLabel }}</p>
        <p class="text-[10px] opacity-45 font-mono uppercase tracking-[0.3em]">{{ lifecycleStore.state }}</p>
      </div>
    </div>
  </Transition>

  <!-- 0.4 数据库迁移完成确认弹窗 -->
  <Transition name="fade">
    <div v-if="lifecycleStore.state === 'MIGRATED'"
      class="fixed inset-0 z-boot bg-white/95 dark:bg-gray-950/95 flex flex-col items-center justify-center p-8 text-center">
      <div
        class="w-full max-w-sm rounded-3xl border border-blue-500/20 bg-white/90 dark:bg-white/5 shadow-2xl px-6 py-8 flex flex-col items-center gap-6">
        <div class="w-16 h-16 bg-blue-500/10 text-blue-500 rounded-2xl flex items-center justify-center">
          <svg width="32" height="32" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
            <path d="M22 11.08V12a10 10 0 1 1-5.93-9.14"></path>
            <polyline points="22 4 12 14.01 9 11.01"></polyline>
          </svg>
        </div>
        <div class="flex flex-col gap-2">
          <p class="text-[10px] font-black tracking-[0.35em] text-blue-500 pl-[0.35em] uppercase">SYSTEM UPDATE</p>
          <h2 class="text-2xl font-black tracking-tight text-primary-text">数据库升级完成</h2>
          <p class="text-xs opacity-75 leading-relaxed mt-1">
            本地数据库格式已成功退化重构为高兼容性纯文本格式，并完成了 FTS5 全文检索引擎重组。
          </p>
        </div>
        <button @click="confirmRestart()"
          class="w-full py-3 bg-blue-500 text-white rounded-xl font-bold shadow-lg shadow-blue-500/25 active:scale-95 transition-all text-sm">
          确认并完全重启应用
        </button>
      </div>
    </div>
  </Transition>

  <!-- 0.5 全局错误看板 -->
  <Transition name="fade">
    <div v-if="lifecycleStore.state === 'ERROR'"
      class="fixed inset-0 z-boot bg-white/98 dark:bg-gray-950/98 flex flex-col items-center justify-center p-8 text-center">
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

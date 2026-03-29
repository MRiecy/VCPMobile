<script setup lang="ts">
import type { AppSettings } from '../../../core/stores/settings';

defineProps<{
  settings: AppSettings;
}>();
</script>

<template>
  <section class="space-y-4">
    <div class="flex items-center gap-2 px-1">
      <div class="w-1 h-4 bg-purple-500 rounded-full"></div>
      <h3 class="text-xs font-black uppercase tracking-[0.2em] opacity-50 dark:opacity-40">AI Engine Logic</h3>
    </div>
    <div class="card-modern divide-y divide-black/5 dark:divide-white/5">
      <div class="flex justify-between items-center py-3.5">
        <div class="flex flex-col">
          <span class="text-[15px] font-semibold">开启打字机流式输出 (Smooth Stream)</span>
          <span class="text-[10px] opacity-40 dark:opacity-30">以固定帧率平滑渲染接收到的文字</span>
        </div>
        <div class="relative inline-flex items-center cursor-pointer">
          <input type="checkbox" v-model="settings.enableSmoothStreaming" class="sr-only peer">
          <div class="w-11 h-6 bg-black/10 dark:bg-white/10 peer-focus:outline-none rounded-full peer peer-checked:after:translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:border-gray-300 after:border after:rounded-full after:h-5 after:w-5 after:transition-all peer-checked:bg-blue-500"></div>
        </div>
      </div>

      <div class="flex justify-between items-center py-3.5">
        <span class="text-[15px] font-semibold">VCP 动态工具路由注入</span>
        <div class="relative inline-flex items-center cursor-pointer">
          <input type="checkbox" v-model="settings.enableVcpToolInjection" class="sr-only peer">
          <div class="w-11 h-6 bg-black/10 dark:bg-white/10 peer-focus:outline-none rounded-full peer peer-checked:after:translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:border-gray-300 after:border after:rounded-full after:h-5 after:w-5 after:transition-all peer-checked:bg-blue-500"></div>
        </div>
      </div>

      <div class="flex justify-between items-center py-3.5">
        <span class="text-[15px] font-semibold">气泡主题 UI 规范注入</span>
        <div class="relative inline-flex items-center cursor-pointer">
          <input type="checkbox" v-model="settings.enableAgentBubbleTheme" class="sr-only peer">
          <div class="w-11 h-6 bg-black/10 dark:bg-white/10 peer-focus:outline-none rounded-full peer peer-checked:after:translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:border-gray-300 after:border after:rounded-full after:h-5 after:w-5 after:transition-all peer-checked:bg-blue-500"></div>
        </div>
      </div>
    </div>
  </section>
</template>

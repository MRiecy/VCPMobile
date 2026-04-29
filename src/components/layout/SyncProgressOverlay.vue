<script setup lang="ts">
import { computed, ref, onMounted, onUnmounted } from 'vue';
import { useNotificationStore } from '../../core/stores/notification';
import { useAppLifecycleStore } from '../../core/stores/appLifecycle';
import { listen } from '@tauri-apps/api/event';

const notificationStore = useNotificationStore();
const lifecycleStore = useAppLifecycleStore();

const logs = ref<string[]>([]);
const unlistenLog = ref<any>(null);
const unlistenProgress = ref<any>(null);

const progressData = ref({
  phase: 'initialization',
  total: 0,
  completed: 0,
  message: ''
});

const progressPercent = computed(() => {
  if (progressData.value.total <= 0) return 0;
  return Math.min(100, Math.round((progressData.value.completed / progressData.value.total) * 100));
});

const currentMessage = computed(() => progressData.value.message || notificationStore.vcpStatus.message);

const phaseLabel = computed(() => {
  const map: Record<string, string> = {
    'initialization': '初始化中',
    'metadata': '同步元数据',
    'topic': '同步会话主题',
    'message': '同步历史消息',
  };
  return map[progressData.value.phase] || progressData.value.phase;
});

onMounted(async () => {
  // 专门为 Overlay 收集最近的同步日志
  unlistenLog.value = await listen('vcp-log', (event: any) => {
    const { category, message } = event.payload;
    if (category === 'sync') {
      logs.value.unshift(`[${new Date().toLocaleTimeString()}] ${message}`);
      if (logs.value.length > 8) logs.value.pop();
    }
  });

  // 监听后端推送的真实进度
  unlistenProgress.value = await listen('vcp-sync-progress', (event: any) => {
    progressData.value = event.payload as any;
  });
});

onUnmounted(() => {
  if (unlistenLog.value) unlistenLog.value();
  if (unlistenProgress.value) unlistenProgress.value();
});
</script>

<template>
  <Transition name="neural-fade">
    <div v-if="lifecycleStore.state === 'INITIAL_SYNCING'"
      class="fixed inset-0 z-[2000] bg-gray-950 flex flex-col items-center justify-center p-8 overflow-hidden">
      
      <!-- 背景：扫描线效果 -->
      <div class="absolute inset-0 opacity-10 pointer-events-none overflow-hidden">
        <div class="scanline"></div>
        <div class="grid-overlay"></div>
      </div>

      <!-- 中心区域 -->
      <div class="relative z-10 flex flex-col items-center max-w-sm w-full gap-10">
        
        <!-- 进度环 (简化版，体现科技感) -->
        <div class="relative w-32 h-32 flex items-center justify-center">
          <div class="absolute inset-0 rounded-full border border-blue-500/20"></div>
          <div class="absolute inset-[-10px] rounded-full border border-blue-500/5 animate-[pulse_2s_infinite]"></div>
          <div class="absolute inset-0 rounded-full border-2 border-transparent border-t-blue-500 animate-[spin_1.5s_linear_infinite]"></div>
          
          <div class="flex flex-col items-center justify-center">
            <span class="text-[10px] font-black text-blue-500/60 tracking-[0.2em] uppercase">Syncing</span>
            <div class="flex items-baseline gap-0.5">
              <span class="text-2xl font-black font-mono text-primary-text leading-none">VCP</span>
              <span class="text-[10px] font-bold text-blue-500 animate-pulse">_</span>
            </div>
          </div>
        </div>

        <!-- 文本状态 -->
        <div class="flex flex-col items-center gap-3 text-center">
          <div class="px-3 py-1 bg-blue-500/10 border border-blue-500/20 rounded-full">
            <p class="text-[9px] font-black tracking-[0.3em] text-blue-400 uppercase">Neural Knowledge Graph</p>
          </div>
          <h2 class="text-xl font-black tracking-tight text-primary-text">正在同步大数据量...</h2>
          <p class="text-sm opacity-60 leading-relaxed max-w-[280px]">
            {{ currentMessage || '正在构建本地神经知识图谱，这可能需要一点时间。' }}
          </p>
        </div>

        <!-- 日志流模拟 -->
        <div class="w-full bg-black/40 border border-white/5 rounded-xl p-4 font-mono text-[10px] leading-relaxed overflow-hidden">
          <div class="flex items-center gap-2 mb-2 opacity-30">
            <div class="w-1.5 h-1.5 rounded-full bg-green-500"></div>
            <span class="uppercase tracking-widest">Live Sync Stream</span>
          </div>
          <div class="space-y-1 h-32 overflow-hidden relative">
            <TransitionGroup name="list">
              <div v-for="(log, i) in logs" :key="log + i" 
                class="truncate text-blue-400/80 first:text-blue-400 first:font-bold transition-all duration-500"
                :style="{ opacity: 1 - (i * 0.12) }">
                {{ log }}
              </div>
            </TransitionGroup>
            <!-- 底部渐变遮挡 -->
            <div class="absolute inset-x-0 bottom-0 h-10 bg-gradient-to-t from-black/80 to-transparent"></div>
          </div>
        </div>

        <!-- 进度条 -->
        <div class="w-full flex flex-col gap-2">
          <div class="h-1 w-full bg-white/5 rounded-full overflow-hidden">
            <div 
              class="h-full bg-blue-500 shadow-[0_0_10px_rgba(59,130,246,0.5)] transition-all duration-300"
              :style="{ width: progressPercent + '%' }"
            ></div>
          </div>
          <div class="flex justify-between items-center text-[9px] font-mono opacity-40 tracking-widest uppercase">
            <span>Stage: {{ phaseLabel }}</span>
            <span v-if="progressData.total > 0" class="animate-pulse">{{ progressPercent }}% [{{ progressData.completed }}/{{ progressData.total }}]</span>
            <span v-else class="animate-pulse">Analyzing...</span>
          </div>
        </div>
      </div>

      <div class="absolute bottom-10 text-[9px] font-black tracking-[0.5em] text-white/20 uppercase">
        VCP Mobile • High-Speed Core
      </div>
    </div>
  </Transition>
</template>

<style scoped>
.neural-fade-enter-active,
.neural-fade-leave-active {
  transition: all 0.6s cubic-bezier(0.16, 1, 0.3, 1);
}

.neural-fade-enter-from,
.neural-fade-leave-to {
  opacity: 0;
  backdrop-filter: blur(0px);
  transform: scale(1.05);
}

.scanline {
  width: 100%;
  height: 100px;
  z-index: 1;
  background: linear-gradient(0deg, rgba(0, 0, 0, 0) 0%, rgba(59, 130, 246, 0.1) 50%, rgba(0, 0, 0, 0) 100%);
  opacity: 0.1;
  position: absolute;
  bottom: 100%;
  animation: scanline 6s linear infinite;
}

.grid-overlay {
  position: absolute;
  inset: 0;
  background-image: radial-gradient(circle, rgba(255,255,255,0.05) 1px, transparent 1px);
  background-size: 30px 30px;
}

@keyframes scanline {
  0% { transform: translateY(0); }
  100% { transform: translateY(200vh); }
}

@keyframes loading-bar {
  0% { width: 0%; }
  50% { width: 70%; }
  100% { width: 95%; }
}

.list-enter-active,
.list-leave-active {
  transition: all 0.4s ease;
}
.list-enter-from {
  opacity: 0;
  transform: translateY(-10px);
}
.list-leave-to {
  opacity: 0;
  transform: translateY(10px);
}
</style>
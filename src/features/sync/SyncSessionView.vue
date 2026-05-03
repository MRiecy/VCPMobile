<script setup lang="ts">
import { computed } from 'vue';
import { X, Copy, Play } from 'lucide-vue-next';
import SlidePage from '../../components/ui/SlidePage.vue';
import SyncLogBrowserCore from '../../features/settings/components/SyncLogBrowserCore.vue';
import { useSyncSessionStore } from '../../core/stores/syncSession';
import { useAssistantStore } from '../../core/stores/assistant';
import { useTopicStore } from '../../core/stores/topicListManager';

const store = useSyncSessionStore();
const assistantStore = useAssistantStore();
const topicStore = useTopicStore();

const visibleLogs = computed(() => {
  // 只渲染最近 100 条，避免 DOM 过重；内存中保留 200 条
  const start = Math.max(0, store.logs.length - 100);
  return store.logs.slice(start);
});

const progressPercent = computed(() => {
  if (store.progressData.total <= 0) return 0;
  return Math.min(100, Math.round((store.progressData.completed / store.progressData.total) * 100));
});

const phaseLabel = computed(() => {
  const map: Record<string, string> = {
    'initialization': '初始化',
    'metadata': '元数据比对',
    'topic': '会话主题同步',
    'message': '历史消息同步',
  };
  return map[store.progressData.phase] || store.progressData.phase;
});

const statusLabel = computed(() => {
  switch (store.status) {
    case 'connecting': return '连接中';
    case 'connected': return '同步中';
    case 'completed': return '已完成';
    case 'error': return '失败';
    default: return '等待';
  }
});

const statusDotClass = computed(() => {
  switch (store.status) {
    case 'connecting': return 'bg-yellow-400 animate-pulse';
    case 'connected': return 'bg-blue-400 animate-pulse';
    case 'completed': return 'bg-green-400';
    case 'error': return 'bg-red-400';
    default: return 'bg-gray-400';
  }
});

const progressBarClass = computed(() => {
  switch (store.status) {
    case 'error': return 'bg-red-500';
    case 'completed': return 'bg-green-500';
    default: return 'bg-blue-500';
  }
});

const isSyncing = computed(() => store.status === 'connected');

const logColor = (level: string) => {
  switch (level) {
    case 'success': return 'text-green-400';
    case 'error': return 'text-red-400';
    case 'warning': return 'text-yellow-400';
    default: return 'text-blue-300';
  }
};

const handleClose = async () => {
  if (store.needsReload) {
    const confirmed = confirm('同步已完成，数据已更新。点击确认立即刷新以生效。');
    if (confirmed) {
      store.markReloaded();
      store.close();
      // 全量数据刷新
      await Promise.all([
        assistantStore.fetchAgents(),
        assistantStore.fetchGroups(),
      ]);
      topicStore.invalidateAllTopicCaches();
      return;
    }
  }
  store.close();
};
</script>

<template>
  <SlidePage :is-open="store.isOpen" :z-index="100">
    <div class="fixed inset-0 flex flex-col bg-[#0a0f14] text-white overflow-hidden"
         :class="{ 'pointer-events-none': !store.isOpen }">

      <!-- 顶部栏 -->
      <div class="flex items-center justify-between px-4 pt-[calc(env(safe-area-inset-top)+8px)] pb-3">
        <div class="flex items-center gap-3">
          <div class="flex items-center gap-2">
            <div class="w-2 h-2 rounded-full" :class="statusDotClass"></div>
            <span class="text-xs font-bold uppercase tracking-widest">{{ statusLabel }}</span>
          </div>
          <!-- Tab 切换 -->
          <div class="flex items-center gap-0.5 ml-2">
            <button
              @click="store.switchTab('live')"
              :disabled="isSyncing && store.activeTab !== 'live'"
              class="px-2 py-0.5 rounded text-[10px] font-bold tracking-wider transition-colors"
              :class="store.activeTab === 'live'
                ? 'bg-white/15 text-white'
                : 'text-white/30 hover:text-white/60 disabled:opacity-20'"
            >
              实时
            </button>
            <button
              @click="store.switchTab('history')"
              :disabled="isSyncing"
              class="px-2 py-0.5 rounded text-[10px] font-bold tracking-wider transition-colors"
              :class="store.activeTab === 'history'
                ? 'bg-white/15 text-white'
                : 'text-white/30 hover:text-white/60 disabled:opacity-20'"
            >
              历史
            </button>
          </div>
        </div>
        <button v-if="store.canDismiss" @click="handleClose()"
          class="p-2 -mr-2 text-gray-400 hover:text-white transition-colors">
          <X :size="20" />
        </button>
      </div>

      <!-- 实时视图 -->
      <div v-if="store.activeTab === 'live'" class="flex-1 flex flex-col overflow-hidden">
        <!-- idle 状态：同步启动占位 -->
        <div v-if="store.status === 'idle'" class="flex-1 flex flex-col items-center justify-center px-8">
          <div class="w-16 h-16 rounded-full bg-white/5 flex items-center justify-center mb-6">
            <Play :size="28" class="text-blue-400 ml-1" />
          </div>
          <div class="text-sm font-bold tracking-wider mb-2">全量神经同步</div>
          <div class="text-[11px] text-white/30 text-center mb-8 leading-relaxed">
            与桌面端进行数据全量比对与同步<br>
            包括会话主题、历史消息及附件
          </div>
          <button
            @click="store.startSync()"
            class="px-8 py-3 rounded-lg bg-blue-500/20 text-blue-400 text-xs font-bold tracking-widest uppercase active:bg-blue-500/30 transition-colors"
          >
            开始同步
          </button>
          <button
            @click="store.switchTab('history')"
            class="mt-4 text-[10px] text-white/20 hover:text-white/40 transition-colors"
          >
            或查看历史日志
          </button>
        </div>

        <!-- 非 idle 状态：进度 + 日志 -->
        <template v-else>
          <!-- 进度条 -->
          <div class="px-4 mb-4">
            <div class="h-1 bg-white/10 rounded-full overflow-hidden">
              <div class="h-full transition-all duration-500 rounded-full"
                   :class="progressBarClass"
                   :style="{ width: progressPercent + '%' }"></div>
            </div>
            <div class="flex justify-between text-[10px] mt-1 opacity-50">
              <span>{{ phaseLabel }}</span>
              <span v-if="store.progressData.total > 0">
                {{ store.progressData.completed }}/{{ store.progressData.total }}
              </span>
            </div>
          </div>

          <!-- 日志终端 -->
          <div class="flex-1 px-4 overflow-hidden">
            <div class="bg-black/40 rounded-lg p-3 font-mono text-[10px] leading-relaxed h-full overflow-y-auto flex flex-col-reverse">
              <div v-if="store.logs.length === 0" class="text-white/20 italic">
                等待连接...
              </div>
              <template v-else>
                <div v-for="log in visibleLogs" :key="log.time + log.message"
                     class="truncate mb-0.5"
                     :class="logColor(log.level)">
                  [{{ log.time }}] {{ log.message }}
                </div>
                <div v-if="store.logs.length > 100" class="text-white/20 text-center py-1">
                  ... {{ store.logs.length - 100 }} 条更早的日志已折叠（内存中保留最近 200 条）
                </div>
              </template>
            </div>
          </div>
        </template>
      </div>

      <!-- 历史视图 -->
      <div v-else class="flex-1 flex flex-col overflow-hidden">
        <SyncLogBrowserCore />
      </div>

      <!-- 底部工具栏 -->
      <div class="flex items-center justify-between px-4 py-2 border-t border-white/5">
        <div class="text-[9px] opacity-30 font-bold tracking-[0.2em] uppercase">
          <span v-if="store.status === 'idle'">选择上方操作以继续</span>
          <span v-else-if="store.status === 'connecting'">正在建立神经同步通道...</span>
          <span v-else-if="store.status === 'connected'">同步进行中</span>
          <span v-else-if="store.status === 'completed'">同步已完成</span>
          <span v-else-if="store.status === 'error'">同步失败，请检查配置</span>
        </div>
        <button v-if="store.logs.length > 0 && store.activeTab === 'live'" @click="store.copyLogs()"
          class="flex items-center gap-1 px-2 py-1 rounded text-[10px] text-white/50 hover:text-white hover:bg-white/10 transition-colors">
          <Copy :size="12" />
          复制日志
        </button>
      </div>

      <!-- 全局遮罩层（连接成功后激活，阻止误触） -->
      <div v-if="store.status === 'connected'"
           class="absolute inset-0 bg-black/20 z-10 flex flex-col justify-end pointer-events-auto"
           style="touch-action: none;">
        <div class="pb-8 text-center">
          <div class="inline-flex items-center gap-2 px-4 py-2 rounded-full bg-black/60 text-white/90 text-xs font-bold tracking-wider">
            <div class="w-1.5 h-1.5 rounded-full bg-blue-400 animate-pulse"></div>
            同步进行中 — 请勿退出
          </div>
        </div>
      </div>
    </div>
  </SlidePage>
</template>

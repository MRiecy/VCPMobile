<script setup lang="ts">
import { computed, ref, watch, nextTick } from 'vue';
import { X, Play, RotateCcw, Copy, Check } from 'lucide-vue-next';
import SlidePage from '../../components/ui/SlidePage.vue';
import SyncLogBrowserCore from '../../features/settings/components/SyncLogBrowserCore.vue';
import { useSyncSessionStore } from '../../core/stores/syncSession';
import { useOverlayStore } from '../../core/stores/overlay';

interface Props {
  zIndex?: number;
}

const props = defineProps<Props>();

const store = useSyncSessionStore();
const overlayStore = useOverlayStore();

const logContainer = ref<HTMLElement | null>(null);

watch([() => store.logs.length, logContainer], () => {
  nextTick(() => {
    if (logContainer.value) {
      logContainer.value.scrollTop = logContainer.value.scrollHeight;
    }
  });
});

const visibleLogs = computed(() => {
  // 只渲染最近 100 条，避免 DOM 过重；内存中保留 200 条
  const start = Math.max(0, store.logs.length - 100);
  return store.logs.slice(start);
});

const progressPercent = computed(() => {
  if (store.status === 'completed' || store.status === 'completed_with_warnings') return 100;
  if (store.progressData.total <= 0) return 0;
  return Math.min(100, Math.round((store.progressData.completed / store.progressData.total) * 100));
});

const phaseLabel = computed(() => {
  const map: Record<string, string> = {
    'initialization': '初始化',
    'owner_metadata': '元数据比对',
    'topic_metadata': '会话主题同步',
    'topic_validation': '会话校验',
    'messages': '历史消息同步',
    'finalize': '数据收尾',
  };
  return map[store.progressData.phase] || '同步处理';
});

const errorStageLabel = computed(() => {
  const map: Record<string, string> = {
    preflight: '设备预检',
    startup: '同步启动',
    connect: '建立连接',
    handshake: '版本握手',
    owner_metadata: '所有者元数据',
    topic_metadata: '话题元数据',
    topic_validation: '话题校验',
    messages: '消息同步',
    finalize: '同步收尾',
    shutdown: '同步退出',
    history: '历史续传',
  };
  return map[store.terminalError?.stage ?? ''] ?? '同步处理';
});

const errorOriginLabel = computed(() => {
  const map: Record<string, string> = {
    mobile_ui: '手机界面',
    mobile_native: '手机系统',
    mobile_sync: '手机同步核心',
    desktop_plugin: '电脑同步插件',
    desktop_cds: '电脑数据服务',
  };
  return map[store.terminalError?.origin ?? ''] ?? '同步组件';
});

const canRetry = computed(() => {
  if (store.status === 'completed_with_warnings') return true;
  return store.status === 'error' && ['manual', 'after_user_action'].includes(
    store.terminalError?.retryAction ?? 'never',
  );
});

const retryLabel = computed(() => {
  if (store.status === 'completed_with_warnings') return '处理后重新同步';
  return store.terminalError?.retryAction === 'after_user_action'
    ? '已处理，重新同步'
    : '重新同步';
});

const statusLabel = computed(() => {
  switch (store.status) {
    case 'connecting': return '连接中';
    case 'connected': return '同步中';
    case 'completed': return '已完成';
    case 'completed_with_warnings': return '有警告';
    case 'error': return '失败';
    default: return '等待';
  }
});

const statusDotClass = computed(() => {
  switch (store.status) {
    case 'connecting': return 'bg-yellow-400 animate-pulse';
    case 'connected': return 'bg-blue-400 animate-pulse';
    case 'completed': return 'bg-green-400';
    case 'completed_with_warnings': return 'bg-yellow-400';
    case 'error': return 'bg-red-400';
    default: return 'bg-gray-400';
  }
});

const progressBarClass = computed(() => {
  switch (store.status) {
    case 'error': return 'bg-red-500';
    case 'completed': return 'bg-green-500';
    case 'completed_with_warnings': return 'bg-yellow-500';
    default: return 'bg-blue-500';
  }
});

const isSyncing = computed(() => store.status === 'connecting' || store.status === 'connected');
const isProgressIndeterminate = computed(() =>
  isSyncing.value && store.progressData.total <= 0
);

const logColor = (level: string) => {
  switch (level) {
    case 'success': return 'text-green-400';
    case 'error': return 'text-red-400';
    case 'warning': return 'text-yellow-400';
    default: return 'text-blue-300';
  }
};

const handleClose = async () => {
  await overlayStore.closeSyncSession();
};

import SettingsSwitch from '../../components/settings/SettingsSwitch.vue';
import { useSettingsStore } from '../../core/stores/settings';
import { useNotificationStore } from '../../core/stores/notification';

const settingsStore = useSettingsStore();
const notificationStore = useNotificationStore();

const CDS_BUILD_CMD = 'node rust_chat_data_service/build-runtime.js';
const isCopied = ref(false);
let copiedTimer: ReturnType<typeof setTimeout> | null = null;

const requiresCdsBuild = computed(() => {
  if (!store.terminalError) return false;
  const guidance = store.terminalError.guidance;
  return guidance.includes(CDS_BUILD_CMD) || guidance.includes('build-runtime.js');
});

const copyBuildCommand = async () => {
  try {
    await navigator.clipboard.writeText(CDS_BUILD_CMD);
    isCopied.value = true;
    notificationStore.addNotification({
      type: 'success',
      title: '复制成功',
      message: '已复制 CDS 编译命令',
      toastOnly: true,
      duration: 2000,
    });
    if (copiedTimer) clearTimeout(copiedTimer);
    copiedTimer = setTimeout(() => {
      isCopied.value = false;
    }, 2000);
  } catch {
    notificationStore.addNotification({
      type: 'error',
      title: '复制失败',
      message: '请长按命令手动复制',
      toastOnly: true,
      duration: 3000,
    });
  }
};

const prerenderEnabled = computed(() =>
  settingsStore.settings?.syncPrerenderEnabled ?? false
);

const handlePrerenderToggle = async (val: boolean) => {
  if (val) {
    const ok = await overlayStore.showConfirm({
      title: '开启预渲染',
      message: '启用后将在同步时进行预渲染计算，可能导致同步耗时增加，首次同步建议关闭。确认启用？'
    });
    if (!ok) return;
  }
  settingsStore.updateSettings({ syncPrerenderEnabled: val });
};
</script>

<template>
  <SlidePage :is-open="store.isOpen" :z-index="props.zIndex">
    <div class="vcp-safe-inline fixed inset-0 flex flex-col bg-[#0a0f14] text-white overflow-hidden"
         :class="{ 'pointer-events-none': !store.isOpen }">

      <!-- 顶部栏 -->
      <header class="shrink-0 border-b border-white/8">
        <div class="flex items-center gap-2 px-4 pt-[calc(var(--vcp-safe-top,0px)+6px)] pb-2">
          <div class="flex min-w-0 flex-1 items-center gap-3">
            <div class="flex shrink-0 items-center gap-2">
              <div class="w-2 h-2 rounded-full" :class="statusDotClass"></div>
              <span class="text-xs font-bold uppercase tracking-widest">{{ statusLabel }}</span>
            </div>
            <nav
              class="flex min-w-0 items-center gap-1"
              role="tablist"
              aria-label="同步视图"
            >
              <button
                id="sync-live-tab"
                type="button"
                role="tab"
                aria-controls="sync-live-panel"
                :aria-selected="store.activeTab === 'live'"
                :disabled="isSyncing && store.activeTab !== 'live'"
                class="min-h-11 min-w-16 border-b-2 px-2 text-xs font-bold tracking-wide transition-colors active:text-white disabled:opacity-25"
                :class="store.activeTab === 'live'
                  ? 'border-blue-400/60 text-white/90'
                  : 'border-transparent text-white/40'"
                @click="store.switchTab('live')"
              >
                实时同步
              </button>
              <button
                id="sync-history-tab"
                type="button"
                role="tab"
                aria-controls="sync-history-panel"
                :aria-selected="store.activeTab === 'history'"
                :disabled="isSyncing"
                class="min-h-11 min-w-16 border-b-2 px-2 text-xs font-bold tracking-wide transition-colors active:text-white disabled:opacity-25"
                :class="store.activeTab === 'history'
                  ? 'border-blue-400/60 text-white/90'
                  : 'border-transparent text-white/40'"
                @click="store.switchTab('history')"
              >
                历史日志
              </button>
            </nav>
          </div>
          <button
            v-if="store.canDismiss"
            type="button"
            aria-label="关闭同步面板"
            class="-mr-2 flex h-11 w-11 shrink-0 items-center justify-center text-gray-400 transition-colors active:text-white"
            @click="handleClose()"
          >
            <X :size="20" />
          </button>
        </div>
      </header>

      <!-- 内容区域：同一时刻仅挂载当前视图 -->
      <div class="flex-1 min-h-0 overflow-hidden">
        <!-- 实时视图 -->
        <div
          v-if="store.activeTab === 'live'"
          id="sync-live-panel"
          role="tabpanel"
          aria-labelledby="sync-live-tab"
          class="h-full flex flex-col overflow-hidden"
        >
          <!-- idle 状态：同步启动占位 -->
          <div v-if="store.status === 'idle'" class="flex-1 flex flex-col items-center justify-center px-8">
            <div class="w-16 h-16 rounded-full bg-white/5 flex items-center justify-center mb-6">
              <Play :size="28" class="text-blue-400 ml-1" />
            </div>
            <div class="text-sm font-bold tracking-wider mb-2">全量神经同步</div>
            <div class="text-[11px] text-white/30 text-center mb-8 leading-relaxed">
              双向比对并合并智能体、群组、话题、头像与历史消息<br>
              较新的修改和删除会同步；附件仅同步信息，不传输文件
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

            <!-- 高级设置区 -->
            <div class="w-full max-w-xs mt-8 border-t border-white/5 pt-4">
              <div class="text-[9px] font-bold uppercase tracking-widest text-white/20 mb-3 text-left">
                高级设置
              </div>
              <div class="flex items-center justify-between">
                <div class="flex flex-col text-left">
                  <span class="text-[12px] font-semibold text-white/70">预渲染同步</span>
                  <span class="text-[9px] text-white/25 mt-0.5">
                    同步时预编译渲染缓存，增加耗时，首次同步时建议关闭
                  </span>
                </div>
                <SettingsSwitch
                  :model-value="prerenderEnabled"
                  @update:model-value="handlePrerenderToggle"
                />
              </div>
            </div>
          </div>

          <!-- 非 idle 状态：进度 + 日志 -->
          <template v-else>
            <!-- 进度条 -->
            <div class="px-4 mb-4">
              <div class="h-1 bg-white/10 rounded-full overflow-hidden">
                <div
                  v-if="isProgressIndeterminate"
                  class="sync-progress-indeterminate h-full rounded-full"
                  :class="progressBarClass"
                ></div>
                <div
                  v-else
                  class="h-full transition-all duration-500 rounded-full"
                  :class="progressBarClass"
                  :style="{ width: progressPercent + '%' }"
                ></div>
              </div>
              <div class="flex justify-between text-[10px] mt-1 opacity-50">
                <span>{{ phaseLabel }}</span>
                <span v-if="store.progressData.total > 0">
                  {{ store.progressData.completed }}/{{ store.progressData.total }} · {{ progressPercent }}%
                </span>
              </div>

              <div
                v-if="store.desktopInfo"
                class="mt-2 flex items-center gap-2 font-mono text-[9px] text-white/35"
              >
                <span>桌面后端 {{ store.desktopInfo.backendMode === 'cds' ? 'CDS' : 'Legacy' }}</span>
                <span>·</span>
                <span>同步插件 v{{ store.desktopInfo.packageVersion }}</span>
              </div>

              <!-- 统计条：messages 阶段起随进度事件实时跳动；终态即使全零也展示 -->
              <div
                v-if="store.summary.totalTopics > 0 || store.status === 'completed' || store.status === 'completed_with_warnings' || store.status === 'error'"
                class="grid grid-cols-4 mt-3 border-y border-white/8 py-2 font-mono text-center"
              >
                <div>
                  <div class="text-[9px] text-white/30">成功</div>
                  <div class="text-xs text-green-400">{{ store.summary.successfulTopics }}</div>
                </div>
                <div>
                  <div class="text-[9px] text-white/30">总数</div>
                  <div class="text-xs text-white/70">{{ store.summary.totalTopics }}</div>
                </div>
                <div>
                  <div class="text-[9px] text-white/30">失败</div>
                  <div class="text-xs" :class="store.summary.failedTopics > 0 ? 'text-red-400' : 'text-white/50'">
                    {{ store.summary.failedTopics }}
                  </div>
                </div>
                <div>
                  <div class="text-[9px] text-white/30">旧附件</div>
                  <div class="text-xs" :class="store.summary.legacyAttachmentWarnings > 0 ? 'text-yellow-400' : 'text-white/50'">
                    {{ store.summary.legacyAttachmentWarnings }}
                  </div>
                </div>
              </div>

              <div
                v-if="store.terminalError"
                class="mt-3 border-l-2 border-red-500 bg-red-500/6 px-3 py-2 text-left"
              >
                <div class="text-[11px] font-semibold leading-relaxed text-red-300 break-words">
                  {{ store.terminalError.message }}
                </div>
                <div class="mt-1 text-[10px] leading-relaxed text-white/55 break-words">
                  {{ store.terminalError.guidance }}
                </div>
                <!-- 针对需要重新编译 CDS 的错误码，提供显式命令框与一键复制 -->
                <div
                  v-if="requiresCdsBuild"
                  class="mt-2.5 rounded border border-white/10 bg-black/40 p-2 text-left"
                >
                  <div class="mb-1 flex items-center justify-between text-[9px] text-white/40">
                    <span class="font-mono">电脑端 (VCPChat 根目录) 重新编译命令:</span>
                    <button
                      type="button"
                      @click="copyBuildCommand"
                      class="flex items-center gap-1 text-blue-400 hover:text-blue-300 active:opacity-70 transition-colors"
                      title="复制命令"
                    >
                      <Check v-if="isCopied" :size="11" class="text-green-400" />
                      <Copy v-else :size="11" />
                      <span :class="isCopied ? 'text-green-400' : 'text-blue-400'">
                        {{ isCopied ? '已复制' : '复制命令' }}
                      </span>
                    </button>
                  </div>
                  <div class="font-mono text-[10px] text-yellow-300/90 break-all select-all py-0.5">
                    {{ CDS_BUILD_CMD }}
                  </div>
                </div>
                <div class="mt-2 font-mono text-[9px] leading-relaxed text-white/35 break-all">
                  {{ errorStageLabel }} · {{ errorOriginLabel }} · {{ store.terminalError.code }}
                </div>
                <div
                  v-if="store.terminalError.failedTopicIds.length > 0"
                  class="mt-2 font-mono text-[9px] leading-relaxed text-red-300/70 break-all"
                >
                  失败 Topic · {{ store.terminalError.failedTopicIds.join(' · ') }}
                </div>
                <div class="mt-2 text-[9px] text-white/30">
                  <template v-if="store.terminalError.logFile">
                    日志 · <span class="font-mono break-all">{{ store.terminalError.logFile }}</span>
                  </template>
                  <template v-else>本次未生成诊断日志。</template>
                </div>
              </div>

              <div
                v-else-if="store.status === 'completed_with_warnings'"
                class="mt-3 border-l-2 border-yellow-500 bg-yellow-500/6 px-3 py-2 text-left"
              >
                <div class="text-[11px] font-semibold leading-relaxed text-yellow-300">
                  消息已同步，{{ store.summary.legacyAttachmentWarnings }} 项旧附件信息无法安全识别，已跳过
                </div>
                <div class="mt-1 text-[10px] leading-relaxed text-white/55">
                  请在电脑端重新发送这些附件后，再重新同步。
                </div>
              </div>
            </div>

            <!-- 日志终端 -->
            <div class="flex-1 px-4 overflow-hidden flex flex-col min-h-0">
              <div ref="logContainer" class="bg-black/40 rounded-lg p-3 font-mono text-[10px] leading-relaxed flex-1 overflow-y-auto no-rubber-band flex flex-col min-h-0">
                <div v-if="store.logs.length === 0" class="text-white/20 italic">
                  等待连接...
                </div>
                <template v-else>
                  <div v-for="log in visibleLogs" :key="log.id"
                       class="break-words mb-0.5"
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
        <div
          v-else
          id="sync-history-panel"
          role="tabpanel"
          aria-labelledby="sync-history-tab"
          class="h-full flex flex-col overflow-hidden"
        >
          <SyncLogBrowserCore />
        </div>
      </div>

      <!-- 底部工具栏 -->
      <div class="flex items-center justify-between px-4 py-2 border-t border-white/5 pb-[calc(var(--vcp-safe-bottom,48px)+4px)]">
        <div class="text-[9px] opacity-30 font-bold tracking-[0.2em] uppercase">
          <span v-if="store.status === 'idle'">选择上方操作以继续</span>
          <span v-else-if="store.status === 'connecting'">正在建立神经同步通道...</span>
          <span v-else-if="store.status === 'connected'">同步进行中</span>
          <span v-else-if="store.status === 'completed'">同步已完成</span>
          <span v-else-if="store.status === 'completed_with_warnings'">同步完成，部分附件信息需处理</span>
          <span v-else-if="store.status === 'error'">同步未完成</span>
        </div>
        <div v-if="store.activeTab === 'live'" class="flex items-center gap-2">
          <button
            v-if="canRetry"
            :disabled="store.retryInFlight"
            @click="store.retrySync()"
            class="flex items-center gap-1 border-l-2 border-blue-400 px-2 py-1 text-[10px] text-blue-300 disabled:opacity-30"
          >
            <RotateCcw :size="12" :class="{ 'animate-spin': store.retryInFlight }" />
            {{ retryLabel }}
          </button>
        </div>
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

<style scoped>
.sync-progress-indeterminate {
  width: 36%;
  animation: sync-progress-slide 1.2s ease-in-out infinite;
}

@keyframes sync-progress-slide {
  from { transform: translateX(-110%); }
  to { transform: translateX(290%); }
}
</style>

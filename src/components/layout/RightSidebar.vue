<script setup lang="ts">
import { watch, ref } from 'vue';
import { X, Trash2, Bug } from 'lucide-vue-next';
import { useNotificationStore } from '../../core/stores/notification';
import { useNotificationProcessor } from '../../core/composables/useNotificationProcessor';
import { useSidebarSwipe } from '../../core/composables/useSidebarSwipe';
import NotificationStatusBar from '../../features/notification/NotificationStatusBar.vue';
import NotificationList from '../../features/notification/NotificationList.vue';
import { useOverlayStore } from '../../core/stores/overlay';

const props = defineProps<{ isOpen: boolean }>();

const emit = defineEmits<{
  close: [];
}>();

const store = useNotificationStore();
const { processPayload } = useNotificationProcessor();
const overlayStore = useOverlayStore();

const openDistributedView = () => {
  overlayStore.openDistributed();
};

const sidebarRef = ref<HTMLElement | null>(null);
useSidebarSwipe(sidebarRef, { type: 'right' });

const triggerDebugNotifications = () => {
  const randomSuffix = () => Math.random().toString(36).substring(2, 5);

  // 调试 payload 必须与后端真实消息结构一致，统一走 processPayload 引擎
  const debugPayloads = [
    // 1. DailyNote 成功 (vcp_log)
    {
      type: 'vcp_log',
      data: {
        tool_name: 'DailyNote',
        status: 'success',
        content: JSON.stringify({
          MaidName: '[Nova]Nova',
          timestamp: '2026-05-26T21:49:09.295+08:00'
        })
      }
    },
    // 2. 普通工具成功 (vcp_log)
    {
      type: 'vcp_log',
      data: {
        tool_name: 'PowerShellExecutor',
        status: 'success',
        source: 'VCPLog',
        content: JSON.stringify({
          MaidName: '艾米莉亚',
          timestamp: '2026-05-26T21:38:00',
          original_plugin_output: {
            status: 'success',
            stdout: 'G:\\VCPMobile\\src\\components\\ui> ls\n\n    Directory: G:\\VCPMobile\\src\\components\\ui\n\nMode                 LastWriteTime         Length Name\n----                 -------------         ------ ----\n-a----        2026/05/26     21:38           1520 ToastItem.vue\n'
          }
        })
      }
    },
    // 3. 工具错误 (vcp_log)
    {
      type: 'vcp_log',
      data: {
        tool_name: 'AdbBridge',
        status: 'error',
        source: 'VCPLog',
        content: '执行错误: {"plugin_error": "device \'emulator-5554\' not found."}'
      }
    },
    // 4. DistPluginManager 消息 (vcp_log)
    {
      type: 'vcp_log',
      data: {
        source: 'DistPluginManager',
        content: '已成功同步 3 个分布式计算节点状态，物理核心 CPU 综合占用率 14%。'
      }
    },
    // 5. 视频生成状态
    {
      type: 'video_generation_status',
      data: {
        status: 'Succeed',
        timestamp: '2026-05-26T21:38:00',
        original_plugin_output: {
          message: '视频已生成，URL: https://cdn.vcpchat.com/generations/vid_77189b.mp4'
        }
      }
    },
    // 6. 工具审核请求（duration=0，含 actions）
    {
      type: 'tool_approval_request',
      data: {
        requestId: 'debug_req_' + randomSuffix(),
        toolName: 'PowerShellExecutor',
        maid: '艾米莉亚',
        args: { command: 'cargo check --workspace' },
        timestamp: '2026-05-26 21:38:00'
      }
    },
    // 7. 连接确认（默认回退逻辑）
    {
      type: 'connection_ack',
      message: 'VCPLog 连接成功！'
    }
  ];

  debugPayloads.forEach((payload) => {
    const processed = processPayload(payload);
    if (processed && !processed.silent) {
      store.addNotification(processed);
    }
  });
};

watch(
  () => props.isOpen,
  (isOpen) => {
    store.isDrawerOpen = isOpen;

    if (isOpen) {
      store.markAllRead();
    }
  },
  { immediate: true }
);
</script>

<template>
  <aside ref="sidebarRef" class="vcp-drawer vcp-drawer-right pt-safe flex flex-col" :class="{ 'is-open': props.isOpen }">
    <div class="px-5 py-4 border-b border-black/5 dark:border-white/5 flex justify-between items-center shrink-0">
      <div class="flex items-center gap-2">
        <h3 class="font-black text-[11px] uppercase tracking-[0.2em] opacity-70 text-primary-text">Notifications</h3>
        <span v-if="store.unreadCount > 0"
          class="px-1.5 py-0.5 bg-blue-500 text-[9px] font-black rounded-full text-white animate-pulse">
          {{ store.unreadCount }}
        </span>
      </div>
      <div class="flex items-center -mr-2">
        <button @click="triggerDebugNotifications"
          class="w-10 h-10 flex items-center justify-center opacity-40 hover:opacity-100 hover:text-amber-500 transition-all text-primary-text active:scale-90"
          title="Push debug notifications">
          <Bug :size="16" />
        </button>
        <button @click="store.clearHistory"
          class="w-10 h-10 flex items-center justify-center opacity-40 hover:opacity-100 hover:text-red-400 transition-all text-primary-text active:scale-90"
          title="Clear all">
          <Trash2 :size="16" />
        </button>
        <button @click="emit('close')" 
          class="w-10 h-10 flex items-center justify-center opacity-40 hover:opacity-100 transition-opacity text-primary-text active:scale-90"
          title="Close">
          <X :size="20" />
        </button>
      </div>
    </div>

    <NotificationStatusBar />
    
    <div class="flex-1 overflow-y-auto">
      <NotificationList :items="store.historyList" />
    </div>

    <!-- 底部：工具按钮 2x2 网格区 -->
    <div class="p-4 border-t border-black/5 dark:border-white/5 glass-panel shrink-0 pb-[calc(var(--vcp-safe-bottom,16px)+8px)]">
      <div class="grid grid-cols-2 gap-2">
        <button
          class="col-span-1 py-3 px-4 rounded-full transition-all text-white flex items-center justify-center gap-2 hover:opacity-90 active:scale-95 shadow-md border border-black/5 dark:border-white/5"
          style="background-color: var(--highlight-text)"
          @click="openDistributedView"
        >
          <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <polygon points="12 2 2 7 12 12 22 7 12 2"></polygon>
            <polyline points="2 17 12 22 22 17"></polyline>
            <polyline points="2 12 12 17 22 12"></polyline>
          </svg>
          <span class="font-bold text-[11px] leading-none">插件中心</span>
        </button>
        
        <div class="col-span-1 border border-dashed border-black/10 dark:border-white/10 rounded-full flex items-center justify-center text-[10px] opacity-25 text-primary-text py-3">
          <span>待开发</span>
        </div>
        <div class="col-span-1 border border-dashed border-black/10 dark:border-white/10 rounded-full flex items-center justify-center text-[10px] opacity-25 text-primary-text py-3">
          <span>待开发</span>
        </div>
        <div class="col-span-1 border border-dashed border-black/10 dark:border-white/10 rounded-full flex items-center justify-center text-[10px] opacity-25 text-primary-text py-3">
          <span>待开发</span>
        </div>
      </div>
    </div>
  </aside>
</template>

<style scoped>
.vcp-drawer {
  position: absolute;
  top: 0;
  bottom: 0;
  width: 82vw;
  max-width: 340px;
  background-color: color-mix(in srgb, var(--secondary-bg) 97%, transparent);
  transition: transform 0.4s cubic-bezier(0.16, 1, 0.3, 1);
  z-index: var(--layer-drawer);
}

.vcp-drawer-right {
  right: 0;
  transform: translateX(100%);
  border-left: 1px solid transparent;
}

.vcp-drawer-right.is-open {
  transform: translateX(0);
}

@media (min-width: 768px) {
  .vcp-drawer {
    position: relative;
    transform: translateX(0) !important;
    width: 300px;
    max-width: 300px;
  }

  .vcp-drawer-right {
    transition: none;
  }
}

@keyframes vcp-shimmer {
  0% {
    background-position: 250% 0;
  }

  100% {
    background-position: -250% 0;
  }
}

@media (hover: none) and (pointer: coarse) {
}
</style>

<script setup lang="ts">
import { watch, ref } from 'vue';
import { onClickOutside, onKeyStroke } from '@vueuse/core';
import {
  X,
  Trash2,
  Bug,
  Sparkles,
  BookOpenText,
  Ellipsis,
  RefreshCw,
  Settings,
  SquareTerminal,
  ScrollText,
  CalendarClock,
  Bot,
  MessageSquareText,
  Mail,
} from 'lucide-vue-next';
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

const openDiaryCenter = () => {
  overlayStore.openDiaryCenter();
};

const sidebarRef = ref<HTMLElement | null>(null);
const moreTrayRef = ref<HTMLElement | null>(null);
const isMoreOpen = ref(false);

const closeMoreMenu = () => {
  isMoreOpen.value = false;
};

const toggleMoreMenu = () => {
  isMoreOpen.value = !isMoreOpen.value;
};

const openMoreTool = (open: () => void) => {
  closeMoreMenu();
  open();
};

const moreTools = [
  {
    id: 'log-center',
    label: '日志中心',
    icon: ScrollText,
    open: () => overlayStore.openLogCenter(),
  },
  {
    id: 'task-center',
    label: '任务调度',
    icon: CalendarClock,
    open: () => overlayStore.openTaskCenter(),
  },
  {
    id: 'agent-mgr',
    label: 'Agent 管理',
    icon: Bot,
    open: () => overlayStore.openAgentMgr(),
  },
  {
    id: 'forum',
    label: 'VCP 论坛',
    icon: MessageSquareText,
    open: () => overlayStore.openForum(),
  },
  {
    id: 'mail',
    label: '邮箱',
    icon: Mail,
    open: () => overlayStore.openMail(),
  },
  {
    id: 'vcp-cli',
    label: 'VCP CLI',
    icon: SquareTerminal,
    open: () => overlayStore.openCliManifest(),
  },
  {
    id: 'sync',
    label: '同步中心',
    icon: RefreshCw,
    open: () => overlayStore.openSyncSession(),
  },
  {
    id: 'settings',
    label: '全局设置',
    icon: Settings,
    open: () => overlayStore.openSettings(),
  },
] as const;

useSidebarSwipe(sidebarRef, { type: 'right' });
onClickOutside(moreTrayRef, closeMoreMenu);
onKeyStroke('Escape', closeMoreMenu);

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
    } else {
      closeMoreMenu();
    }
  },
  { immediate: true }
);
</script>

<template>
  <aside
    id="notification-sidebar"
    ref="sidebarRef"
    class="vcp-drawer vcp-drawer-right flex flex-col min-w-0 min-h-0 overflow-hidden"
    :class="{ 'is-open': props.isOpen }"
    aria-label="通知与工具侧栏"
  >
    <div class="vcp-drawer-header px-5 pb-4 border-b border-black/5 dark:border-white/5 flex justify-between items-center shrink-0">
      <div class="flex items-center gap-2">
        <h3 class="font-black text-[11px] uppercase tracking-[0.2em] opacity-70 text-primary-text">Notifications</h3>
        <span v-if="store.unreadCount > 0"
          class="px-1.5 py-0.5 bg-blue-500 text-[9px] font-black rounded-full text-white">
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
          class="vcp-drawer-close w-10 h-10 flex items-center justify-center opacity-40 hover:opacity-100 transition-opacity text-primary-text active:scale-90"
          title="关闭通知栏"
          aria-label="关闭通知栏">
          <X :size="20" />
        </button>
      </div>
    </div>

    <NotificationStatusBar />
    
    <NotificationList :items="store.historyList" />

    <!-- 底部：常用工具与可扩展的更多工具浮窗 -->
    <div
      ref="moreTrayRef"
      class="right-tool-tray p-4 border-t border-black/5 dark:border-white/5 glass-panel shrink-0 pb-[calc(var(--vcp-safe-bottom,48px)+8px)]"
    >
      <Transition name="right-tool-popover">
        <section
          v-if="isMoreOpen"
          id="right-sidebar-more-tools"
          class="right-tool-popover"
          role="dialog"
          aria-label="更多工具"
        >
          <header class="px-1 pb-3">
            <span class="block text-[9px] font-black tracking-[0.18em] opacity-45 text-primary-text">TOOL TRAY</span>
            <strong class="block mt-0.5 text-[13px] text-primary-text">更多功能</strong>
          </header>

          <div class="right-tool-grid grid grid-cols-2 gap-2">
            <button
              v-for="tool in moreTools"
              :key="tool.id"
              type="button"
              class="min-w-0 min-h-11 px-3 rounded-full border border-black/10 dark:border-white/10 bg-[var(--secondary-bg)] text-[var(--primary-text)] text-[11px] font-bold flex items-center justify-center gap-2 transition-all hover:border-[var(--highlight-text)] active:opacity-80 active:scale-[0.98]"
              @click="openMoreTool(tool.open)"
            >
              <component :is="tool.icon" :size="15" />
              <span>{{ tool.label }}</span>
            </button>
          </div>
        </section>
      </Transition>

      <div class="grid grid-cols-2 gap-2">
        <button
          type="button"
          class="col-span-1 min-h-12 px-4 rounded-full transition-all text-white flex items-center justify-center gap-2 hover:opacity-90 active:scale-95 shadow-md border border-black/5 dark:border-white/5"
          style="background-color: var(--highlight-text)"
          @click="openMoreTool(openDistributedView)"
        >
          <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <polygon points="12 2 2 7 12 12 22 7 12 2"></polygon>
            <polyline points="2 17 12 22 22 17"></polyline>
            <polyline points="2 12 12 17 22 12"></polyline>
          </svg>
          <span class="font-bold text-[11px] leading-none">插件中心</span>
        </button>
        
        <button
          type="button"
          class="col-span-1 min-h-12 px-4 rounded-full transition-all text-white flex items-center justify-center gap-2 hover:opacity-90 active:scale-95 shadow-md border border-black/5 dark:border-white/5"
          style="background-color: #2c3e50"
          @click="openMoreTool(() => overlayStore.openRagObserver())"
        >
          <Sparkles :size="14" class="text-blue-400" />
          <span class="font-bold text-[11px] leading-none">灵视中心</span>
        </button>
        <button
          type="button"
          class="col-span-1 min-h-12 px-4 rounded-full transition-all flex items-center justify-center gap-2 text-[var(--primary-text)] bg-[var(--secondary-bg)] hover:opacity-90 active:scale-95 shadow-sm border border-black/10 dark:border-white/10"
          aria-label="打开日记中心"
          @click="openMoreTool(openDiaryCenter)"
        >
          <BookOpenText :size="15" class="text-[var(--highlight-text)]" />
          <span class="font-bold text-[11px] leading-none">日记中心</span>
        </button>
        <button
          type="button"
          class="col-span-1 min-h-12 px-4 rounded-full transition-all flex items-center justify-center gap-2 text-[var(--primary-text)] bg-[var(--secondary-bg)] hover:opacity-90 active:scale-95 shadow-sm border border-black/10 dark:border-white/10"
          aria-label="打开更多工具"
          aria-controls="right-sidebar-more-tools"
          aria-haspopup="dialog"
          :aria-expanded="isMoreOpen"
          @click="toggleMoreMenu"
        >
          <Ellipsis :size="17" class="text-[var(--highlight-text)]" />
          <span class="font-bold text-[11px] leading-none">更多</span>
        </button>
      </div>
    </div>
  </aside>
</template>

<style scoped>
.vcp-drawer {
  position: absolute;
  top: 0;
  bottom: 0;
  box-sizing: border-box;
  height: 100%;
  min-height: 0;
  width: 82vw;
  max-width: 340px;
  visibility: hidden;
  pointer-events: none;
  background-color: var(--vcp-panel-bg-97, var(--secondary-bg));
  transition:
    transform 0.4s cubic-bezier(0.16, 1, 0.3, 1),
    visibility 0s linear 0.4s;
  z-index: var(--layer-drawer);
}

.vcp-drawer-right {
  right: 0;
  padding-right: var(--vcp-workspace-safe-right, 0px);
  transform: translateX(calc(100% + var(--vcp-drawer-shadow-clearance, 64px)));
  border-left: 1px solid transparent;
}

.vcp-drawer-right.is-open {
  visibility: visible;
  pointer-events: auto;
  transform: translateX(0);
  transition-delay: 0s;
}

.vcp-drawer-header {
  padding-top: calc(var(--vcp-safe-top, 24px) + 1rem);
}

.right-tool-tray {
  position: relative;
}

.right-tool-popover {
  position: absolute;
  right: 1rem;
  bottom: calc(100% + 0.625rem);
  left: 1rem;
  z-index: 10;
  padding: 0.75rem;
  border: 1px solid var(--vcp-border-subtle, var(--border-color));
  border-radius: 1rem;
  background-color: var(--vcp-panel-bg-97, var(--secondary-bg));
  box-shadow: 0 12px 28px rgba(0, 0, 0, 0.24);
  transform-origin: 75% 100%;
}

/* 平板/宽屏下 popover 变宽，网格升为三列；基础形态保持两列长胶囊 */
@media (min-width: 768px) {
  .right-tool-grid {
    grid-template-columns: repeat(3, minmax(0, 1fr));
  }
}

.right-tool-popover::after {
  position: absolute;
  right: calc(25% - 0.3125rem);
  bottom: -0.35rem;
  width: 0.625rem;
  height: 0.625rem;
  content: '';
  border-right: 1px solid var(--vcp-border-subtle, var(--border-color));
  border-bottom: 1px solid var(--vcp-border-subtle, var(--border-color));
  background-color: var(--vcp-panel-bg-97, var(--secondary-bg));
  transform: rotate(45deg);
}

.right-tool-popover-enter-active,
.right-tool-popover-leave-active {
  transition: opacity 0.16s ease, transform 0.16s ease;
}

.right-tool-popover-enter-from,
.right-tool-popover-leave-to {
  opacity: 0;
  transform: translateY(0.375rem) scale(0.98);
}

@media (min-width: 1280px) {
  .vcp-drawer {
    position: relative;
    top: auto;
    right: auto;
    bottom: auto;
    flex: 0 0 300px;
    transform: translateX(0) !important;
    width: 300px;
    max-width: 300px;
    visibility: visible;
    pointer-events: auto;
    z-index: var(--layer-local);
    transition: none;
  }

  .vcp-drawer-close {
    display: none;
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

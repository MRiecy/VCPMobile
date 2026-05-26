<script setup lang="ts">
import { watch } from 'vue';
import { X, Trash2, Bug } from 'lucide-vue-next';
import { useNotificationStore } from '../../core/stores/notification';
import NotificationStatusBar from '../../features/notification/NotificationStatusBar.vue';
import NotificationList from '../../features/notification/NotificationList.vue';

const props = defineProps<{ isOpen: boolean }>();

const emit = defineEmits<{
  close: [];
}>();

const store = useNotificationStore();

const triggerDebugNotifications = () => {
  // 1. 日常日记
  store.addNotification({
    id: 'debug_daily_note_' + Math.random().toString(36).substring(2, 5),
    type: 'success',
    title: '日记: 艾米莉亚 (2026-05-26)',
    message: '✅ 成功记录本日思考与 3 项待办事项到本地 SQLite 知识库。',
    isPreformatted: false
  });

  // 2. 工具审核请求
  store.addNotification({
    id: 'debug_tool_approval_' + Math.random().toString(36).substring(2, 5),
    type: 'warning',
    title: '🛠️ 审核请求: RunCommand',
    message: '助手: 艾米莉亚\n命令: pnpm check\n时间: 2026-05-26 21:38:00',
    isPreformatted: true,
    duration: 0,
    actions: [
      { label: '允许', value: true, color: 'bg-green-600' },
      { label: '拒绝', value: false, color: 'bg-red-600' }
    ],
    rawPayload: {
      type: 'tool_approval_request',
      data: {
        requestId: 'debug_req_' + Math.random().toString(36).substring(2, 5),
        toolName: 'RunCommand',
        maid: '艾米莉亚',
        args: { command: 'pnpm check' },
        timestamp: '2026-05-26 21:38:00'
      }
    }
  });

  // 3. VCP 日志错误 (JSON 格式化代码块)
  store.addNotification({
    id: 'debug_vcp_log_error_' + Math.random().toString(36).substring(2, 5),
    type: 'error',
    title: 'sync_retry.rs error',
    message: JSON.stringify({
      error: "ConnectionReset",
      attempt: 3,
      url: "wss://api.vcpchat.com/v2/sync",
      message: "远程服务器强行关闭了一个现有的连接。经过 3 次指数退避重试后，同步管道断开。"
    }, null, 2),
    isPreformatted: true
  });

  // 4. 视频生成状态
  store.addNotification({
    id: 'debug_video_status_' + Math.random().toString(36).substring(2, 5),
    type: 'info',
    title: '视频生成状态: (05-26 21:38)',
    message: '🎬 正在编译第 12 帧渲染缓存，当前总进度 42.5%，预计剩余时间 14 秒。',
    isPreformatted: false
  });

  // 5. 连接成功
  store.addNotification({
    id: 'debug_conn_ack_' + Math.random().toString(36).substring(2, 5),
    type: 'success',
    title: 'VCP 连接成功',
    message: '连接已建立。双轨同步会话已准备就绪，当前延迟 14ms。',
    isPreformatted: false
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
  <aside class="vcp-drawer vcp-drawer-right pt-safe flex flex-col" :class="{ 'is-open': props.isOpen }">
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
    <NotificationList :items="store.historyList" />
  </aside>
</template>

<style scoped>
.vcp-drawer {
  position: absolute;
  top: 0;
  bottom: 0;
  width: 82vw;
  max-width: 340px;
  background-color: color-mix(in srgb, var(--secondary-bg) 95%, transparent);
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

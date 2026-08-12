<script setup lang="ts">
import { computed } from 'vue';
import { useNotificationStore } from '../../core/stores/notification';

const notificationStore = useNotificationStore();

const statusConfig = computed(() => {
  // P0 优先级：VCPLog 未连接时，覆盖 Core 状态显示红灯
  const logStatus = notificationStore.vcpStatus.status;
  if (logStatus !== 'connected') {
    return {
      color: 'bg-red-500',
      shadow: 'shadow-red-500/50',
      text: 'VCPLog 未连接'
    };
  }

  // 正常模式：显示 Core 引擎状态
  const s = notificationStore.vcpCoreStatus.status;
  switch (s) {
    case 'ready':
      return {
        color: 'bg-green-500',
        shadow: 'shadow-green-500/50',
        text: 'Core Active'
      };
    case 'initializing':
    case 'connecting':
      return {
        color: 'bg-yellow-500',
        shadow: 'shadow-yellow-500/50',
        text: 'Booting...'
      };
    case 'error':
      return {
        color: 'bg-red-500',
        shadow: 'shadow-red-500/50',
        text: 'Core Error'
      };
    default:
      return {
        color: 'bg-gray-400',
        shadow: 'shadow-gray-400/20',
        text: 'Unknown'
      };
  }
});
</script>

<template>
  <div
    class="flex items-center gap-1.5 transition-all duration-300 select-none"
    :title="notificationStore.vcpCoreStatus.message"
    :aria-label="statusConfig.text"
  >
    <!-- 所有状态使用静态颜色与文字，避免页头常驻刷新。 -->
    <div
      data-testid="core-status-dot"
      aria-hidden="true"
      class="w-1.5 h-1.5 rounded-full transition-colors duration-500"
      :class="[statusConfig.color, statusConfig.shadow]"
    ></div>
    
    <!-- 状态文字 -->
    <span class="text-[9px] opacity-40 uppercase font-mono tracking-tighter">
      {{ statusConfig.text }}
    </span>
  </div>
</template>

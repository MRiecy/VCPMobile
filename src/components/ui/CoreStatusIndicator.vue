<script setup lang="ts">
import { computed } from 'vue';
import { useNotificationStore } from '../../core/stores/notification';

const notificationStore = useNotificationStore();

const statusConfig = computed(() => {
  const s = notificationStore.vcpCoreStatus.status;
  switch (s) {
    case 'ready':
      return {
        color: 'bg-green-500',
        shadow: 'shadow-green-500/50',
        animate: 'vcp-core-pulse',
        text: 'Core Active'
      };
    case 'initializing':
    case 'connecting':
      return {
        color: 'bg-yellow-500',
        shadow: 'shadow-yellow-500/50',
        animate: 'animate-pulse',
        text: 'Booting...'
      };
    case 'error':
      return {
        color: 'bg-red-500',
        shadow: 'shadow-red-500/50',
        animate: 'animate-bounce',
        text: 'Core Error'
      };
    default:
      return {
        color: 'bg-gray-400',
        shadow: 'shadow-gray-400/20',
        animate: '',
        text: 'Unknown'
      };
  }
});
</script>

<template>
  <div 
    class="flex items-center gap-1.5 transition-all duration-300 select-none"
    :title="notificationStore.vcpCoreStatus.message"
  >
    <!-- 呼吸灯 -->
    <div 
      class="w-1.5 h-1.5 rounded-full transition-colors duration-500"
      :class="[statusConfig.color, statusConfig.shadow, statusConfig.animate]"
    ></div>
    
    <!-- 状态文字 -->
    <span class="text-[9px] opacity-40 uppercase font-mono tracking-tighter">
      {{ statusConfig.text }}
    </span>
  </div>
</template>

<style scoped>
.vcp-core-pulse {
  animation: vcpCorePulse 2s cubic-bezier(0.4, 0, 0.2, 1) infinite;
}

@keyframes vcpCorePulse {
  0%, 100% {
    opacity: 1;
    transform: scale(1.1);
  }
  50% {
    opacity: 0.6;
    transform: scale(0.9);
  }
}
</style>

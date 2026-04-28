<script setup lang="ts">
import { computed } from 'vue';
import { useNotificationStore } from '../../core/stores/notification';

const store = useNotificationStore();

const statusClass = computed(() => {
  switch (store.vcpStatus.status) {
    case 'connected':
    case 'open':
      return 'bg-[#2e7d32] text-white';
    case 'disconnected':
    case 'closed':
      return 'bg-[#c62828] text-white';
    case 'error':
      return 'bg-[#b71c1c] text-white';
    case 'connecting':
      return 'bg-[#f9a825] text-black';
    default:
      return 'bg-black/20 text-primary-text';
  }
});
</script>

<template>
  <div class="w-full text-center py-1.5 text-[10px] font-black uppercase tracking-[0.2em] transition-colors duration-300 shadow-sm relative z-20"
    :class="statusClass">
    {{ store.vcpStatus.source || 'VCPLog' }}: {{ store.vcpStatus.message || 'IDLE' }}
  </div>
</template>

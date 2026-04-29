<script setup lang="ts">
import { useNotificationStore, type VcpNotification } from '../../core/stores/notification';
import { Info, CheckCircle, AlertTriangle, X, Cpu, User } from 'lucide-vue-next';
import { invoke } from '@tauri-apps/api/core';

const store = useNotificationStore();

const getIcon = (type: string) => {
  switch (type) {
    case 'success': return CheckCircle;
    case 'warning': return AlertTriangle;
    case 'error': return X;
    case 'tool': return Cpu;
    case 'agent': return User;
    default: return Info;
  }
};

const handleAction = async (item: VcpNotification, action: any) => {
  if (item.type === 'warning' && item.rawPayload?.type === 'tool_approval_request') {
    const response = {
      type: 'tool_approval_response',
      data: {
        requestId: item.rawPayload.data.requestId,
        approved: action.value
      }
    };

    try {
      await invoke('send_vcp_log_message', { payload: response });
      // 处理后关闭 Toast
      item.actions = [];
      store.activeToasts = store.activeToasts.filter((t: VcpNotification) => t.id !== item.id);
    } catch (e) {
      console.error('Action failed from toast', e);
    }
  }
};
</script>

<template>
  <div class="fixed top-safe left-0 right-0 z-[200] pointer-events-none px-4 pt-4 flex flex-col items-center gap-2">
    <TransitionGroup name="toast">
      <div v-for="toast in store.activeToasts" :key="toast.id"
        class="pointer-events-auto flex items-center justify-between gap-3 px-3 py-2 rounded-xl bg-black/85 backdrop-blur-md border border-white/10 shadow-lg max-w-sm w-full overflow-hidden">
        
        <div class="flex items-center gap-2.5 min-w-0 flex-1">
          <component :is="getIcon(toast.type)" :size="14"
            :class="toast.type === 'error' ? 'text-red-400' : toast.type === 'success' ? 'text-green-400' : 'text-blue-400'" class="shrink-0" />
          <div class="flex-col flex min-w-0">
             <span class="text-[11px] font-bold text-white truncate">{{ toast.title }}</span>
             <span v-if="toast.type !== 'error' && !toast.isPreformatted" class="text-[10px] text-white/60 truncate">{{ toast.message }}</span>
          </div>
        </div>

        <div v-if="toast.actions && toast.actions.length > 0" class="flex gap-1.5 shrink-0">
          <button v-for="action in toast.actions" :key="action.label" @click="handleAction(toast, action)" :class="[
            action.label === 'Approve' || action.color?.includes('green') ? 'text-green-400 bg-green-500/10' :
              action.label === 'Deny' || action.color?.includes('red') ? 'text-red-400 bg-red-500/10' : 'text-blue-400 bg-blue-500/10',
            'px-2 py-1 transition-all duration-200 font-bold text-[10px] rounded-md'
          ]">
            {{ action.label }}
          </button>
        </div>
        <button v-else @click="store.activeToasts = store.activeToasts.filter((t: VcpNotification) => t.id !== toast.id)"
          class="p-1 opacity-40 hover:opacity-100 text-white transition-opacity shrink-0">
          <X :size="14" />
        </button>
      </div>
    </TransitionGroup>
  </div>
</template>

<style scoped>
.toast-enter-active {
  transition: all 0.5s cubic-bezier(0.18, 0.89, 0.32, 1.28);
}

.toast-leave-active {
  transition: all 0.4s ease-in;
}

.toast-enter-from {
  opacity: 0;
  transform: translateY(-40px) scale(0.8);
}

.toast-leave-to {
  opacity: 0;
  transform: translateY(-20px) scale(0.9);
}

.toast-move {
  transition: transform 0.4s ease;
}
</style>

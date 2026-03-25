<script setup lang="ts">
import { useNotificationStore, VcpNotification } from '../stores/notification';
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
      store.activeToasts = store.activeToasts.filter(t => t.id !== item.id);
    } catch (e) {
      console.error('Action failed from toast', e);
    }
  }
};
</script>

<template>
  <div class="fixed top-safe left-0 right-0 z-[200] pointer-events-none px-6 pt-4 flex flex-col items-center gap-3">
    <TransitionGroup name="toast">
      <div v-for="toast in store.activeToasts" :key="toast.id"
           class="pointer-events-auto flex flex-col gap-3 px-4 py-3 rounded-2xl bg-black/85 backdrop-blur-[10px] border border-white/10 shadow-2xl max-w-md w-full overflow-hidden bg-[linear-gradient(110deg,transparent_0%,transparent_35%,rgba(255,255,255,0.1)_50%,transparent_65%,transparent_100%)] bg-[length:300%_100%] animate-[vcp-shimmer_15s_linear_infinite]">
        
        <div class="flex items-center gap-3">
          <div class="shrink-0 p-1.5 rounded-xl bg-white/5">
            <component :is="getIcon(toast.type)" :size="14" 
                      :class="toast.type === 'error' ? 'text-red-400' : 'text-blue-400'" />
          </div>
          <div class="flex-1 min-w-0 pr-2">
            <div class="text-[11px] font-black uppercase tracking-wider opacity-90 mb-0.5 text-white">{{ toast.title }}</div>
            <div v-if="toast.isPreformatted" 
                 class="bg-black/20 p-1.5 rounded text-[0.85em] mt-1.5 max-h-[100px] overflow-y-auto whitespace-pre-wrap break-all font-mono text-white/90">
              {{ toast.message }}
            </div>
            <div v-else class="text-[12px] text-white/80 truncate">
              {{ toast.message }}
            </div>
          </div>
          <button @click="store.activeToasts = store.activeToasts.filter(t => t.id !== toast.id)" 
                  class="p-1 opacity-40 hover:opacity-100 text-white transition-opacity">
            <X :size="14" />
          </button>
        </div>

        <!-- 对标桌面端的按钮逻辑 -->
        <div v-if="toast.actions && toast.actions.length > 0" class="flex gap-2 pb-1">
          <button v-for="action in toast.actions" :key="action.label"
                  @click="handleAction(toast, action)"
                  :class="[
                    action.label === 'Approve' || action.color?.includes('green') ? 'bg-green-600' :
                    action.label === 'Deny' || action.color?.includes('red') ? 'bg-red-600' : action.color,
                    'flex-1 py-1.5 shadow-sm hover:shadow-md hover:-translate-y-0.5 active:translate-y-0 transition-all duration-200 font-medium text-[11px] rounded-lg text-white'
                  ]">
            {{ action.label }}
          </button>
        </div>
      </div>
    </TransitionGroup>
  </div>
</template>

<style scoped>
.toast-enter-active { transition: all 0.5s cubic-bezier(0.18, 0.89, 0.32, 1.28); }
.toast-leave-active { transition: all 0.4s ease-in; }
.toast-enter-from { opacity: 0; transform: translateY(-40px) scale(0.8); }
.toast-leave-to { opacity: 0; transform: translateY(-20px) scale(0.9); }
.toast-move { transition: transform 0.4s ease; }

@keyframes vcp-shimmer {
  0% { background-position: 100% 0; }
  100% { background-position: -200% 0; }
}
</style>

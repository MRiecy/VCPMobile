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
      await invoke('sendToVCP', { payload: JSON.stringify(response) });
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
           class="pointer-events-auto flex flex-col gap-3 px-4 py-3 rounded-2xl bg-black/80 dark:bg-white/10 backdrop-blur-xl border border-white/10 shadow-2xl max-w-md w-full overflow-hidden">
        
        <div class="flex items-center gap-3">
          <div class="shrink-0 p-1.5 rounded-xl bg-white/5">
            <component :is="getIcon(toast.type)" :size="14" 
                      :class="toast.type === 'error' ? 'text-red-400' : 'text-blue-400'" />
          </div>
          <div class="flex-1 min-w-0 pr-2">
            <div class="text-[11px] font-black uppercase tracking-wider opacity-90 mb-0.5 text-white">{{ toast.title }}</div>
            <div :class="['text-[12px] text-white/80', toast.isPreformatted ? 'font-mono opacity-100' : 'truncate']">
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
                  :class="action.color"
                  class="flex-1 py-2 rounded-xl text-[10px] font-black text-white active:scale-95 transition-all uppercase tracking-wider">
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
</style>

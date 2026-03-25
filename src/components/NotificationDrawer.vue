<script setup lang="ts">
import { useNotificationStore, VcpNotification } from '../stores/notification';
import { X, Trash2, Bell, Info, AlertTriangle, CheckCircle, Cpu, User, Copy, Check } from 'lucide-vue-next';
import { format } from 'date-fns';
import { ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';

defineProps<{ isOpen: boolean }>();
const emit = defineEmits(['close']);
const store = useNotificationStore();

const copiedId = ref<string | null>(null);

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

const getTypeColor = (type: string) => {
  switch (type) {
    case 'success': return 'text-green-500';
    case 'warning': return 'text-amber-500';
    case 'error': return 'text-red-500';
    case 'tool': return 'text-purple-500';
    case 'agent': return 'text-blue-500';
    default: return 'text-blue-400';
  }
};

const copyContent = (item: VcpNotification) => {
  const text = `${item.title}\n${item.message}`;
  navigator.clipboard.writeText(text);
  copiedId.value = item.id;
  setTimeout(() => copiedId.value = null, 2000);
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
      // 通过 vcp_client 接口回传
      await invoke('sendToVCP', { payload: JSON.stringify(response) });
      
      // 处理后 UI 反馈：清空按钮并从 Toast 移除
      item.actions = [];
      item.message = `[已处理] 操作: ${action.label}`;
      store.activeToasts = store.activeToasts.filter(t => t.id !== item.id);
    } catch (e) {
      console.error('Action failed', e);
    }
  }
};
</script>

<template>
  <aside class="vcp-drawer vcp-drawer-right pt-safe flex flex-col z-[100]" :class="{ 'is-open': isOpen }">
    <div class="p-6 border-b border-white/10 flex justify-between items-center shrink-0">
      <div class="flex items-center gap-2">
        <h3 class="font-bold text-xs uppercase tracking-[0.2em] opacity-80 text-primary-text">Notification Center</h3>
        <span v-if="store.unreadCount > 0" class="px-1.5 py-0.5 bg-blue-500 text-[9px] font-black rounded-full text-white">
          {{ store.unreadCount }}
        </span>
      </div>
      <div class="flex items-center gap-1">
        <button @click="store.clearHistory" class="p-2 opacity-40 hover:opacity-100 hover:text-red-400 transition-all text-primary-text">
          <Trash2 :size="16" />
        </button>
        <button @click="emit('close')" class="p-2 opacity-40 hover:opacity-100 transition-opacity text-primary-text">
          <X :size="20" />
        </button>
      </div>
    </div>

    <div class="flex-1 overflow-y-auto custom-scrollbar">
      <TransitionGroup name="list" tag="div" class="p-4 space-y-4">
        <div v-for="item in store.historyList" :key="item.id" 
             class="group relative p-4 rounded-2xl bg-white/5 border border-white/5 hover:bg-white/10 transition-all">
          
          <div class="flex gap-3">
            <component :is="getIcon(item.type)" :size="16" :class="getTypeColor(item.type)" class="mt-0.5 shrink-0" />
            <div class="flex-1 min-w-0">
              <div class="flex justify-between items-start mb-1">
                <span class="text-[11px] font-bold opacity-90 truncate pr-2 text-primary-text">{{ item.title }}</span>
                <span class="text-[9px] font-mono opacity-30 whitespace-nowrap text-primary-text">{{ format(item.timestamp, 'HH:mm:ss') }}</span>
              </div>
              
              <!-- 消息体：支持 Preformatted 模式 -->
              <div :class="[
                'text-[12px] leading-relaxed break-words text-primary-text',
                item.isPreformatted ? 'font-mono bg-black/10 p-2 rounded-lg mt-2 opacity-80' : 'opacity-60'
              ]" style="white-space: pre-wrap;">{{ item.message }}</div>

              <!-- 动作按钮区 -->
              <div v-if="item.actions && item.actions.length > 0" class="mt-4 flex gap-2">
                <button v-for="action in item.actions" :key="action.label"
                        @click="handleAction(item, action)"
                        :class="action.color"
                        class="px-4 py-2 rounded-xl text-[10px] font-black text-white active:scale-95 transition-all uppercase tracking-wider">
                  {{ action.label }}
                </button>
              </div>
            </div>

            <!-- 复制按钮 -->
            <button @click="copyContent(item)" class="opacity-0 group-hover:opacity-40 hover:!opacity-100 transition-opacity p-1 text-primary-text">
              <component :is="copiedId === item.id ? Check : Copy" :size="14" />
            </button>
          </div>
        </div>
      </TransitionGroup>

      <!-- 空状态 -->
      <div v-if="store.historyList.length === 0" class="h-full flex flex-col items-center justify-center opacity-20 text-center p-8">
        <Bell :size="48" stroke-width="1" class="mb-4 text-primary-text" />
        <div class="text-[10px] uppercase tracking-[0.2em] font-light text-primary-text">No notifications yet</div>
      </div>
    </div>
  </aside>
</template>

<style scoped>
.list-enter-active, .list-leave-active { transition: all 0.4s cubic-bezier(0.3, 0, 0.2, 1); }
.list-enter-from { opacity: 0; transform: translateX(30px); }
.list-leave-to { opacity: 0; transform: scale(0.9); }
</style>

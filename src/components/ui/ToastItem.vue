<script setup lang="ts">
import { computed, ref } from 'vue';
import { useSwipe } from '@vueuse/core';
import { Info, CheckCircle, AlertTriangle, X, Cpu, User } from 'lucide-vue-next';
import { invoke } from '@tauri-apps/api/core';
import { useNotificationStore, type VcpNotification } from '../../core/stores/notification';

const props = defineProps<{
  toast: VcpNotification;
}>();

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

const dismissToast = (id: string) => {
  store.activeToasts = store.activeToasts.filter((t: VcpNotification) => t.id !== id);
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
      item.actions = [];
      dismissToast(item.id);
    } catch (e) {
      console.error('Action failed from toast', e);
    }
  }
};

const el = ref<HTMLElement | null>(null);
const { isSwiping, lengthX } = useSwipe(el, {
  onSwipeEnd(_, direction) {
    if ((direction === 'left' || direction === 'right') && Math.abs(lengthX.value) > 60) {
      dismissToast(props.toast.id);
    }
  },
});

const swipeStyle = computed(() => {
  if (!isSwiping.value) return {};
  const opacity = Math.max(0, 1 - Math.abs(lengthX.value) / 200);
  return {
    transform: `translateX(${-lengthX.value}px)`,
    opacity
  };
});

const handleClick = () => {
  if (!props.toast.actions || props.toast.actions.length === 0) {
    dismissToast(props.toast.id);
  }
};
</script>

<template>
  <div 
    ref="el"
    class="pointer-events-auto flex items-center justify-between gap-3 px-3 py-2.5 rounded-2xl bg-white/95 dark:bg-[#1e1e1e]/95 backdrop-blur-md border border-black/5 dark:border-white/10 shadow-[0_8px_32px_rgba(0,0,0,0.12)] max-w-sm w-full overflow-hidden transition-all duration-200 active:scale-[0.98]"
    :style="swipeStyle"
    @click="handleClick"
  >
    <div class="flex items-center gap-3 min-w-0 flex-1">
      <component :is="getIcon(toast.type)" :size="15"
        :class="toast.type === 'error' ? 'text-red-500' : toast.type === 'success' ? 'text-green-500' : 'text-blue-500'" class="shrink-0" />
      <div class="flex-col flex min-w-0">
         <span class="text-[12px] font-bold text-black/90 dark:text-white/90 truncate leading-tight">{{ toast.title }}</span>
         <span v-if="toast.type !== 'error' && !toast.isPreformatted" class="text-[10px] text-black/50 dark:text-white/50 truncate mt-0.5">{{ toast.message }}</span>
      </div>
    </div>

    <div v-if="toast.actions && toast.actions.length > 0" class="flex gap-2 shrink-0 ml-1">
      <button v-for="action in toast.actions" :key="action.label" @click.stop="handleAction(toast, action)" :class="[
        action.label === 'Approve' || action.color?.includes('green') ? 'text-green-600 bg-green-500/10' :
          action.label === 'Deny' || action.color?.includes('red') ? 'text-red-600 bg-red-500/10' : 'text-blue-600 bg-blue-500/10',
        'px-2.5 py-1.5 transition-all duration-200 font-black text-[10px] rounded-lg uppercase tracking-wider'
      ]">
        {{ action.label }}
      </button>
    </div>
    <button v-else @click.stop="dismissToast(toast.id)"
      class="p-1.5 opacity-20 hover:opacity-100 text-black dark:text-white transition-opacity shrink-0 ml-1">
      <X :size="14" />
    </button>
  </div>
</template>

<script setup lang="ts">
import { computed, ref } from 'vue';
import { useSwipe } from '@vueuse/core';
import { Info, CheckCircle, AlertTriangle, X, Cpu, User } from 'lucide-vue-next';
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

const getIconColor = (type: string) => {
  switch (type) {
    case 'success': return 'text-green-500';
    case 'warning': return 'text-amber-500';
    case 'error': return 'text-red-500';
    case 'tool': return 'text-purple-500';
    case 'agent': return 'text-blue-500';
    default: return 'text-blue-400';
  }
};

const dismissToast = (id: string) => {
  store.activeToasts = store.activeToasts.filter((t: VcpNotification) => t.id !== id);
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

const displayMessage = computed(() => {
  if (!props.toast.message) return '';
  // 对于预格式化或者带有换行的消息，在极简 Toast 中仅展示为单行空格分隔的纯文本并裁剪
  const cleanMsg = props.toast.message.replace(/[\r\n]+/g, ' ').replace(/\s+/g, ' ').trim();
  return cleanMsg.length > 50 ? cleanMsg.substring(0, 50) + '...' : cleanMsg;
});

const handleClick = () => {
  dismissToast(props.toast.id);
};
</script>

<template>
  <div 
    ref="el"
    class="pointer-events-auto flex items-center justify-between gap-3 px-3.5 py-2.5 rounded-xl bg-white/90 dark:bg-zinc-900/90 backdrop-blur-md border border-black/5 dark:border-white/10 shadow-[0_8px_30px_rgba(0,0,0,0.12)] max-w-[90vw] w-[320px] overflow-hidden transition-all duration-200 active:scale-[0.98] cursor-pointer"
    :style="swipeStyle"
    @click="handleClick"
  >
    <div class="flex items-center gap-3 min-w-0 flex-1">
      <component :is="getIcon(toast.type)" :size="14" :class="getIconColor(toast.type)" class="shrink-0 opacity-80" />
      <div class="flex flex-col min-w-0 flex-1">
         <span class="text-[11px] font-bold text-primary-text leading-tight tracking-wide truncate">{{ toast.title }}</span>
         <span v-if="displayMessage" class="text-[9px] text-primary-text opacity-50 truncate mt-0.5 font-sans leading-none pr-1">
           {{ displayMessage }}
         </span>
      </div>
    </div>

    <button @click.stop="dismissToast(toast.id)"
      class="p-1 opacity-20 hover:opacity-100 text-primary-text transition-opacity shrink-0 ml-1">
      <X :size="12" />
    </button>
  </div>
</template>


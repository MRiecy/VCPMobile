<script setup lang="ts">
import { watch } from 'vue';
import { X, Trash2 } from 'lucide-vue-next';
import { useNotificationStore } from '../../core/stores/notification';
import NotificationStatusBar from '../../features/notification/NotificationStatusBar.vue';
import NotificationList from '../../features/notification/NotificationList.vue';

const props = defineProps<{ isOpen: boolean }>();

const emit = defineEmits<{
  close: [];
}>();

const store = useNotificationStore();

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
        <h3 class="font-black text-[11px] uppercase tracking-[0.2em] opacity-50 text-primary-text">Notifications</h3>
        <span v-if="store.unreadCount > 0"
          class="px-1.5 py-0.5 bg-blue-500 text-[9px] font-black rounded-full text-white animate-pulse">
          {{ store.unreadCount }}
        </span>
      </div>
      <div class="flex items-center -mr-2">
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
  background-color: color-mix(in srgb, var(--secondary-bg) 85%, transparent);
  backdrop-filter: blur(18px) saturate(165%);
  -webkit-backdrop-filter: blur(18px) saturate(165%);
  transition: transform 0.4s cubic-bezier(0.16, 1, 0.3, 1);
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
  .vcp-drawer {
    backdrop-filter: blur(4px) saturate(165%);
    -webkit-backdrop-filter: blur(4px) saturate(165%);
  }
}
</style>

<script setup lang="ts">
import type { VcpNotification } from '../../core/stores/notification';
import NotificationCard from './NotificationCard.vue';
import NotificationEmptyState from './NotificationEmptyState.vue';
import { useNotificationClipboard } from './composables/useNotificationClipboard';
import { useNotificationStore } from '../../core/stores/notification';

const props = defineProps<{
  items: VcpNotification[];
}>();

const { copyContent, getCopyIcon } = useNotificationClipboard();
const store = useNotificationStore();
</script>

<template>
  <div class="flex-1 overflow-y-auto vcp-scrollable">
    <TransitionGroup name="list" tag="div" class="flex flex-col">
      <NotificationCard v-for="item in props.items" :key="item.id" :item="item" :copy-icon="getCopyIcon(item.id)"
        @copy="copyContent(item)" @remove="store.removeHistoryItem(item.id)" />
    </TransitionGroup>

    <div v-if="props.items.length === 0"
      class="h-full flex flex-col items-center justify-center opacity-20 text-center p-8">
      <NotificationEmptyState />
    </div>
  </div>
</template>

<style scoped>
.list-enter-active,
.list-leave-active {
  transition: all 0.4s cubic-bezier(0.3, 0, 0.2, 1);
}

.list-enter-from {
  opacity: 0;
  transform: translateX(30px);
}

.list-leave-to {
  opacity: 0;
  transform: translateX(100%) scale(0.9);
}
</style>

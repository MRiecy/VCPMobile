<script setup lang="ts">
import type { VcpNotification } from '../../core/stores/notification';
import NotificationCard from './NotificationCard.vue';
import NotificationEmptyState from './NotificationEmptyState.vue';
import { useNotificationClipboard } from './composables/useNotificationClipboard';
import { useNotificationStore } from '../../core/stores/notification';
import { isIOS } from '../../core/utils/platform';
import { useSystemNotificationPermission } from './composables/useSystemNotificationPermission';

const props = defineProps<{
  items: VcpNotification[];
}>();

const { copyContent, getCopyIcon } = useNotificationClipboard();
const store = useNotificationStore();
const systemPermission = useSystemNotificationPermission();
</script>

<template>
  <div class="flex-1 overflow-y-auto overflow-x-hidden vcp-scrollable no-rubber-band">
    <div v-if="isIOS" class="flex items-center gap-3 border-b border-[var(--border-color)] px-4 py-3">
      <div class="min-w-0 flex-1">
        <p class="text-xs font-semibold">系统通知</p>
        <p class="mt-1 text-xs opacity-60 leading-relaxed" role="status">{{ systemPermission.description.value }}</p>
      </div>
      <button class="shrink-0 px-3 py-2 text-xs border border-[var(--border-color)] rounded-lg disabled:opacity-50"
        :inert="systemPermission.busy.value || systemPermission.openingSettings.value"
        :disabled="systemPermission.actionBusy.value"
        @click="systemPermission.act">
        {{ systemPermission.actionLabel.value }}
      </button>
    </div>
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

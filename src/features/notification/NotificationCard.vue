<script setup lang="ts">
import type { VcpNotification } from '../../core/stores/notification';
import { format } from 'date-fns';
import { useNotificationStore } from '../../core/stores/notification';
import { useNotificationPresentation } from './composables/useNotificationPresentation';
import { ref } from 'vue';

const props = defineProps<{
  item: VcpNotification;
  copyIcon: any;
}>();

const emit = defineEmits<{
  copy: [];
  remove: [];
}>();

const store = useNotificationStore();
const { getIcon, getTypeColor, getActionButtonClass } = useNotificationPresentation();

const handleAction = (action: { label: string; value: boolean; color: string }) => {
  store.executeAction(props.item.id, action);
};

// --- Swipe to Dismiss ---
const swipeX = ref(0);
const isDragging = ref(false);
let startX = 0;
let startY = 0;
let touchStarted = false;
let isVerticalScroll = false;
let hasDeterminedDirection = false;
const SWIPE_THRESHOLD = 120;
const MAX_SWIPE = 200;

const onTouchStart = (e: TouchEvent) => {
  startX = e.touches[0].clientX;
  startY = e.touches[0].clientY;
  touchStarted = true;
  isDragging.value = false;
  isVerticalScroll = false;
  hasDeterminedDirection = false;
};

const onTouchMove = (e: TouchEvent) => {
  if (!touchStarted || isVerticalScroll) return;

  const currentX = e.touches[0].clientX;
  const currentY = e.touches[0].clientY;
  const deltaX = currentX - startX;
  const deltaY = currentY - startY;

  if (!hasDeterminedDirection) {
    const absX = Math.abs(deltaX);
    const absY = Math.abs(deltaY);
    if (absX > 5 || absY > 5) {
      hasDeterminedDirection = true;
      if (absY / absX > 0.577) {
        isVerticalScroll = true;
        touchStarted = false;
        return;
      }
    } else {
      return;
    }
  }

  if (deltaX > 0) {
    isDragging.value = true;
    swipeX.value = Math.min(deltaX, MAX_SWIPE);
  }
};

const onTouchEnd = () => {
  if (!touchStarted) return;
  touchStarted = false;
  isDragging.value = false;

  if (swipeX.value > SWIPE_THRESHOLD) {
    if (navigator.vibrate) navigator.vibrate(40);
    emit('remove');
  } else {
    swipeX.value = 0;
  }
};
</script>

<template>
  <div
    @touchstart="onTouchStart"
    @touchmove="onTouchMove"
    @touchend="onTouchEnd"
    class="group relative px-3.5 py-2.5 border-b border-black/5 dark:border-white/5 hover:bg-black/[0.02] dark:hover:bg-white/[0.02] transition-colors"
    :class="isDragging ? 'transition-none' : 'transition-transform duration-200 ease-out'"
    :style="{ transform: `translateX(${swipeX}px)` }">
    <div class="flex items-start gap-2.5">
      <component :is="getIcon(props.item.type)" :size="13" :class="getTypeColor(props.item.type)"
        class="mt-0.5 shrink-0 opacity-75" />
      
      <div class="flex-1 min-w-0 flex flex-col">
        <div class="flex justify-between items-start gap-2">
          <span class="text-[10.5px] font-black tracking-wide opacity-80 pr-2 text-highlight-text leading-tight">{{ props.item.title }}</span>
          <span class="text-[9px] font-mono opacity-30 whitespace-nowrap text-secondary-text leading-none mt-0.5">{{ format(props.item.timestamp, 'HH:mm:ss') }}</span>
        </div>

        <div v-if="props.item.isPreformatted"
          class="mt-1.5 p-1.5 bg-black/10 dark:bg-black/20 rounded text-[9px] max-h-[100px] overflow-y-auto whitespace-pre-wrap break-all font-mono opacity-70 text-primary-text leading-normal select-text">
          {{ props.item.message }}
        </div>
        <div v-else class="text-[10.5px] leading-relaxed break-words text-primary-text opacity-65 mt-0.5 select-text">
          {{ props.item.message }}
        </div>

        <div v-if="props.item.actions && props.item.actions.length > 0" class="mt-2 flex gap-1.5">
          <button v-for="action in props.item.actions" :key="action.label" @click="handleAction(action)"
            :class="getActionButtonClass(action)" class="!py-1 !px-2.5 !text-[9.5px] !rounded-md active:scale-95 transition-transform duration-100">
            {{ action.label }}
          </button>
        </div>
      </div>

      <button @click="$emit('copy')"
        class="self-center opacity-0 group-hover:opacity-30 hover:!opacity-80 transition-opacity p-1 shrink-0 text-primary-text active:scale-90 duration-100">
        <component :is="props.copyIcon" :size="13" />
      </button>
    </div>
  </div>
</template>

<style scoped>
@keyframes vcp-shimmer {
  0% {
    background-position: 250% 0;
  }

  100% {
    background-position: -250% 0;
  }
}
</style>

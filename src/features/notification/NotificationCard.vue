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
    class="group relative p-4 rounded-2xl border border-white/5 hover:bg-white/10 bg-[linear-gradient(110deg,rgba(255,255,255,0.05)_0%,rgba(255,255,255,0.05)_40%,rgba(255,255,255,0.1)_50%,rgba(255,255,255,0.05)_60%,rgba(255,255,255,0.05)_100%)] bg-[length:250%_100%] animate-[vcp-shimmer_15s_linear_infinite]"
    :class="isDragging ? 'transition-none' : 'transition-transform duration-200 ease-out'"
    :style="{ transform: `translateX(${swipeX}px)` }">
    <div class="flex gap-3">
      <component :is="getIcon(props.item.type)" :size="16" :class="getTypeColor(props.item.type)"
        class="mt-0.5 shrink-0" />
      <div class="flex-1 min-w-0">
        <div class="flex justify-between items-start mb-1">
          <span class="text-[11px] font-bold opacity-90 truncate pr-2 text-primary-text">{{ props.item.title }}</span>
          <span class="text-[9px] font-mono opacity-30 whitespace-nowrap text-primary-text">{{
            format(props.item.timestamp, 'HH:mm:ss') }}</span>
        </div>

        <div v-if="props.item.isPreformatted"
          class="bg-black/20 p-1.5 rounded text-[0.85em] mt-1.5 max-h-[100px] overflow-y-auto whitespace-pre-wrap break-all font-mono opacity-90 text-primary-text">
          {{ props.item.message }}
        </div>
        <div v-else class="text-[12px] leading-relaxed break-words text-primary-text opacity-60">
          {{ props.item.message }}
        </div>

        <div v-if="props.item.actions && props.item.actions.length > 0" class="mt-4 flex gap-2">
          <button v-for="action in props.item.actions" :key="action.label" @click="handleAction(action)"
            :class="getActionButtonClass(action)">
            {{ action.label }}
          </button>
        </div>
      </div>

      <button @click="$emit('copy')"
        class="opacity-0 group-hover:opacity-40 hover:!opacity-100 transition-opacity p-1 text-primary-text">
        <component :is="props.copyIcon" :size="14" />
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

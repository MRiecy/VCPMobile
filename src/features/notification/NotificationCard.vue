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

const reasonText = ref('');

const handleAction = (action: { label: string; value: boolean; color: string }) => {
  store.executeAction(props.item.id, action, reasonText.value);
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
    if (absX > 15 || absY > 15) { // 提高手势判定阈值，防止在滑动浏览历史时出现过敏性误触移位
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
    @touchstart.stop="onTouchStart"
    @touchmove.stop="onTouchMove"
    @touchend.stop="onTouchEnd"
    class="group relative px-3.5 py-2.5 border-b border-black/5 dark:border-white/5 hover:bg-black/[0.02] dark:hover:bg-white/[0.02] select-none touch-pan-y"
    :style="{ 
      transform: `translateX(${swipeX}px)`,
      transition: isDragging ? 'none' : 'transform 0.25s cubic-bezier(0.16, 1, 0.3, 1)' 
    }">
    <div class="flex items-start gap-2.5">
      <component :is="getIcon(props.item.type)" :size="13" :class="getTypeColor(props.item.type)"
        class="mt-0.5 shrink-0 opacity-75" />

      <div class="flex-1 min-w-0 flex flex-col">
        <span class="text-[10.5px] font-black tracking-wide opacity-80 leading-tight text-[var(--highlight-text)]">{{ props.item.title }}</span>

        <div 
          :class="[
            props.item.isPreformatted ? 'font-mono text-[9.5px] opacity-70 leading-normal bg-black/5 dark:bg-white/5 px-1.5 py-0.5 rounded mt-1.5' : 'text-[10.5px] opacity-65 leading-relaxed mt-0.5',
            'break-words text-primary-text select-text'
          ]"
        >
          {{ props.item.message }}
        </div>

        <div v-if="props.item.actions && props.item.actions.length > 0" class="mt-2 flex flex-col gap-2">
          <!-- 审核理由输入框 -->
          <div v-if="props.item.type === 'warning' && props.item.rawPayload?.type === 'tool_approval_request'" class="flex flex-col gap-1 w-full mt-0.5">
            <textarea
              v-model="reasonText"
              placeholder="可选：告诉 AI 为什么通过或拒绝"
              maxlength="1000"
              class="w-full text-[10px] p-1.5 rounded border border-black/10 dark:border-white/10 bg-black/[0.02] dark:bg-white/[0.02] text-primary-text placeholder:opacity-45 focus:outline-none focus:border-amber-500/50 resize-none font-sans"
              style="line-height: 1.4; height: calc(3.5 * 1.4em + 12px);"
              @touchstart.stop
              @touchmove.stop
              @touchend.stop
              @keydown.stop
            />
            <span class="text-[9px] opacity-40 leading-snug">拒绝时建议填写可执行的修正建议，最多 1000 字。</span>
          </div>

          <div class="flex gap-3.5">
            <button v-for="action in props.item.actions" :key="action.label" @click="handleAction(action)"
              :class="getActionButtonClass(action)">
              {{ action.label }}
            </button>
          </div>
        </div>

        <div class="flex justify-end mt-1">
          <span class="text-[9px] font-mono opacity-30 whitespace-nowrap leading-none text-[var(--secondary-text)]">{{ format(props.item.timestamp, 'HH:mm:ss') }}</span>
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

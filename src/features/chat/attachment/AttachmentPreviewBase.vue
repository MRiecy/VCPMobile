<template>
  <div class="attachment-preview-base relative flex items-center bg-[var(--secondary-bg)] border border-[var(--border-color)] rounded-xl"
       :class="[sizeClass]">
    <!-- Default slot for content -->
    <slot />
    
    <!-- Delete Button -->
    <button
      @click.stop="emit('remove', index)"
      class="absolute -top-1.5 -right-1.5 w-5 h-5 bg-red-500/90 backdrop-blur-md rounded-full flex items-center justify-center text-white shadow-md active:scale-90 transition-transform border border-white/20"
    >
      <svg
        width="10"
        height="10"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="3"
        stroke-linecap="round"
        stroke-linejoin="round"
      >
        <line x1="18" y1="6" x2="6" y2="18"></line>
        <line x1="6" y1="6" x2="18" y2="18"></line>
      </svg>
    </button>

    <!-- Loading Overlay -->
    <div
      v-if="isLoading"
      class="absolute inset-0 bg-black/60 backdrop-blur-[2px] rounded-xl flex flex-col items-center justify-center z-10 gap-1"
    >
      <div
        class="w-5 h-5 border-2 border-white/30 border-t-white rounded-full animate-spin"
      ></div>
      <span
        v-if="file.progress"
        class="text-[9px] text-white font-bold tabular-nums"
      >{{ file.progress }}%</span>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed } from "vue";
import type { Attachment } from "../../../core/stores/chatManager";

interface Props {
  file: Attachment;
  index: number;
  size?: 'small' | 'medium' | 'large';
}

const props = withDefaults(defineProps<Props>(), {
  size: 'medium'
});

const emit = defineEmits<{ (e: "remove", index: number): void }>();

const isLoading = computed(() => props.file.status === "loading");

const sizeClass = computed(() => {
  switch (props.size) {
    case 'small':
      return 'w-10 h-10';
    case 'large':
      return 'w-20 h-20';
    case 'medium':
    default:
      return 'w-14 h-14';
  }
});
</script>

<style scoped>
.list-enter-active,
.list-leave-active {
  transition: all 0.3s cubic-bezier(0.4, 0, 0.2, 1);
}

.list-enter-from {
  opacity: 0;
  transform: translateX(20px) scale(0.8);
}

.list-leave-to {
  opacity: 0;
  transform: translateY(-20px) scale(0.8);
}
</style>

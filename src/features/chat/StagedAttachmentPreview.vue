<script setup lang="ts">
import { computed } from "vue";
import { convertFileSrc } from "@tauri-apps/api/core";
import type { Attachment } from "../../core/stores/chatManager";

const props = defineProps<{ file: Attachment; index: number }>();
const emit = defineEmits<{ (e: "remove", index: number): void }>();

const isImage = computed(() => props.file?.type?.startsWith("image/") || false);
const isVideo = computed(() => props.file?.type?.startsWith("video/") || false);

const formattedSize = computed(
  () => (props.file.size / 1024).toFixed(1) + " KB",
);
const isLoading = computed(() => props.file.status === "loading");

const safeSrc = computed(() => {
  if (!props.file.src) return "";
  if (
    props.file.src.startsWith("http") ||
    props.file.src.startsWith("data:") ||
    props.file.src.startsWith("blob:")
  ) {
    return props.file.src;
  }
  // 此时 src 可能是绝对路径
  try {
    return convertFileSrc(props.file.src.replace("file://", ""));
  } catch (e) {
    return "";
  }
});
</script>

<template>
  <div
    class="relative shrink-0 flex items-center bg-[var(--secondary-bg)] border border-[var(--border-color)] rounded-xl"
    :class="[isImage ? 'w-14 h-14' : 'px-2 py-1.5 max-w-[180px] h-12 gap-2']"
  >
    <!-- Image Card -->
    <template v-if="isImage">
      <div
        class="w-full h-full rounded-xl overflow-hidden bg-black/5 dark:bg-white/5"
      >
        <img :src="safeSrc" class="w-full h-full object-cover" />
      </div>
    </template>

    <!-- Video Card -->
    <template v-else-if="isVideo">
      <div
        class="w-8 h-8 rounded-lg bg-blue-500/10 flex items-center justify-center shrink-0 border border-blue-500/20"
      >
        <svg
          width="14"
          height="14"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2.5"
          stroke-linecap="round"
          stroke-linejoin="round"
          class="text-blue-500"
        >
          <polygon points="23 7 16 12 23 17 23 7"></polygon>
          <rect x="1" y="5" width="15" height="14" rx="2" ry="2"></rect>
        </svg>
      </div>
      <div class="flex flex-col overflow-hidden min-w-0">
        <span
          class="text-xs text-[var(--primary-text)] truncate font-semibold"
          >{{ file.name }}</span
        >
        <span class="text-[9px] opacity-50 uppercase truncate tracking-tight"
          >Video • {{ formattedSize }}</span
        >
      </div>
    </template>

    <!-- Document Card -->
    <template v-else>
      <div
        class="w-8 h-8 rounded-lg bg-orange-500/10 flex items-center justify-center shrink-0 border border-orange-500/20"
      >
        <svg
          width="14"
          height="14"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2.5"
          stroke-linecap="round"
          stroke-linejoin="round"
          class="text-orange-500"
        >
          <path
            d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"
          ></path>
          <polyline points="14 2 14 8 20 8"></polyline>
          <line x1="16" y1="13" x2="8" y2="13"></line>
          <line x1="16" y1="17" x2="8" y2="17"></line>
          <polyline points="10 9 9 9 8 9"></polyline>
        </svg>
      </div>
      <div class="flex flex-col overflow-hidden min-w-0">
        <span
          class="text-xs text-[var(--primary-text)] truncate font-semibold"
          >{{ file.name }}</span
        >
        <span class="text-[9px] opacity-50 uppercase truncate tracking-tight"
          >{{ file.name.includes(".") ? file.name.split(".").pop() : "FILE" }} •
          {{ formattedSize }}</span
        >
      </div>
    </template>

    <!-- Delete Button (always visible on mobile) -->
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
        >{{ file.progress }}%</span
      >
    </div>
  </div>
</template>

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

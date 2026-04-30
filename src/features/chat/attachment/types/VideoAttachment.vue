<template>
  <AttachmentPreviewBase 
    :file="file" 
    :index="index" 
    :show-remove="showRemove"
    size="auto"
    @remove="emit('remove', index)"
  >
    <div class="flex items-center gap-3 px-3 py-2 min-w-[140px] max-w-[200px]">
      <div class="relative w-9 h-9 shrink-0 rounded-lg overflow-hidden bg-black/5 dark:bg-white/5 border border-black/10 dark:border-white/10">
        <img 
          v-if="thumbnailSrc"
          :src="thumbnailSrc" 
          class="w-full h-full object-cover opacity-60"
        />
        <!-- Video Play Icon Overlay -->
        <div class="absolute inset-0 flex items-center justify-center">
          <div class="w-5 h-5 rounded-full bg-blue-500/20 backdrop-blur-sm flex items-center justify-center border border-blue-500/30">
            <svg width="8" height="8" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" class="text-blue-500 translate-x-[0.5px]">
              <polygon points="5 3 19 12 5 21 5 3"></polygon>
            </svg>
          </div>
        </div>
      </div>

      <!-- File Info -->
      <div class="flex flex-col min-w-0">
        <div class="text-[11px] font-bold truncate text-[var(--primary-text)] mb-0.5">
          {{ file.name || 'Video' }}
        </div>
        <div class="text-[9px] opacity-40 font-mono tracking-tighter uppercase">
          {{ formatSize(file.size) }} • VIDEO
        </div>
      </div>
    </div>
  </AttachmentPreviewBase>
</template>

<script setup lang="ts">
import { computed } from "vue";
import { convertFileSrc } from "@tauri-apps/api/core";
import AttachmentPreviewBase from "../AttachmentPreviewBase.vue";
import type { Attachment } from "../../../../core/stores/chatManager";

const props = withDefaults(defineProps<{
  file: Attachment;
  index: number;
  showRemove?: boolean;
}>(), {
  showRemove: false
});

const emit = defineEmits<{ (e: "remove", index: number): void }>();

const thumbnailSrc = computed(() => {
  const src = props.file.thumbnailPath || props.file.src;
  if (!src) return "";
  if (src.startsWith("http") || src.startsWith("data:") || src.startsWith("blob:")) return src;
  try { return convertFileSrc(src.replace("file://", "")); } catch (e) { return ""; }
});

const formatSize = (bytes: number) => {
  if (!bytes) return '0 B';
  const k = 1024;
  const sizes = ['B', 'KB', 'MB', 'GB'];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + ' ' + sizes[i];
};
</script>
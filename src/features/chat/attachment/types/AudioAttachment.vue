<template>
  <AttachmentPreviewBase 
    :file="file" 
    :index="index" 
    :show-remove="showRemove"
    size="auto"
    @remove="emit('remove', index)"
  >
    <div class="flex items-center gap-3 px-3 py-2 min-w-[140px] max-w-[180px]">
      <div class="w-9 h-9 shrink-0 rounded-lg bg-purple-500/10 flex items-center justify-center border border-purple-500/20">
        <svg
          width="18"
          height="18"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2.5"
          stroke-linecap="round"
          stroke-linejoin="round"
          class="text-purple-500"
        >
          <path d="M9 18V5l12-2v13"></path>
          <circle cx="6" cy="18" r="3"></circle>
          <circle cx="18" cy="16" r="3"></circle>
        </svg>
      </div>

      <!-- File Info -->
      <div class="flex flex-col min-w-0">
        <div class="text-[11px] font-bold truncate text-[var(--primary-text)] mb-0.5">
          {{ displayName }}
        </div>
        <div class="text-[9px] opacity-40 font-mono tracking-tighter uppercase">
          {{ formatSize(file.size) }} • AUDIO
        </div>
      </div>
    </div>
  </AttachmentPreviewBase>
</template>

<script setup lang="ts">
import { computed } from "vue";
import AttachmentPreviewBase from "../AttachmentPreviewBase.vue";
import { truncateFileName } from "../utils/truncateFileName";
import type { Attachment } from "../../../../core/types/chat";

const props = withDefaults(defineProps<{
  file: Attachment;
  index: number;
  showRemove?: boolean;
}>(), {
  showRemove: false
});

const emit = defineEmits<{ (e: "remove", index: number): void }>();

const displayName = computed(() => truncateFileName(props.file.name || 'Audio'));

const formatSize = (bytes: number) => {
  if (!bytes) return '0 B';
  const k = 1024;
  const sizes = ['B', 'KB', 'MB', 'GB'];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + ' ' + sizes[i];
};
</script>

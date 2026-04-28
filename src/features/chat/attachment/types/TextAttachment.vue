<template>
  <AttachmentPreviewBase 
    :file="file" 
    :index="index" 
    :show-remove="showRemove"
    size="auto"
    @remove="emit('remove', index)"
  >
    <div class="flex items-center gap-3 px-3 py-2 min-w-[140px] max-w-[200px]">
      <!-- Text Icon -->
      <div class="w-9 h-9 shrink-0 rounded-lg bg-blue-500/10 flex items-center justify-center border border-blue-500/20">
        <svg
          width="18"
          height="18"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2.5"
          stroke-linecap="round"
          stroke-linejoin="round"
          class="text-blue-500"
        >
          <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"></path>
          <polyline points="14 2 14 8 20 8"></polyline>
          <line x1="16" y1="13" x2="8" y2="13"></line>
          <line x1="16" y1="17" x2="8" y2="17"></line>
          <line x1="10" y1="9" x2="9" y2="9"></line>
        </svg>
      </div>
      
      <!-- File Info -->
      <div class="flex flex-col min-w-0">
        <div class="text-[11px] font-bold truncate text-[var(--primary-text)] mb-0.5">
          {{ file.name || 'Untitled' }}
        </div>
        <div class="text-[9px] opacity-40 font-mono tracking-tighter uppercase">
          {{ formatSize(file.size) }} • TXT
        </div>
      </div>
    </div>
  </AttachmentPreviewBase>
</template>

<script setup lang="ts">
import AttachmentPreviewBase from "../AttachmentPreviewBase.vue";
import type { Attachment } from "../../../../core/stores/chatManager";

withDefaults(defineProps<{
  file: Attachment;
  index: number;
  showRemove?: boolean;
}>(), {
  showRemove: false
});

const emit = defineEmits<{ (e: "remove", index: number): void }>();

const formatSize = (bytes: number) => {
  if (!bytes) return '0 B';
  const k = 1024;
  const sizes = ['B', 'KB', 'MB', 'GB'];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + ' ' + sizes[i];
};
</script>
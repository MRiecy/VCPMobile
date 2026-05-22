<template>
  <AttachmentPreviewBase 
    :file="file" 
    :index="index" 
    :show-remove="showRemove"
    size="auto"
    @remove="emit('remove', index)"
  >
    <div class="flex items-center gap-2.5 px-2.5 py-2 min-w-[120px] max-w-[160px]">
      <!-- Pure SVG Icon, no background/border box -->
      <svg
        width="20"
        height="20"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="2"
        stroke-linecap="round"
        stroke-linejoin="round"
        class="text-gray-500 shrink-0"
      >
        <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"></path>
        <polyline points="14 2 14 8 20 8"></polyline>
      </svg>

      <!-- File Info -->
      <div class="flex flex-col min-w-0">
        <div class="text-[12px] font-medium truncate text-[var(--primary-text)] leading-tight mb-0.5">
          {{ displayName }}
        </div>
        <div class="text-[10px] opacity-50 truncate leading-none">
          FILE • {{ formatSize(file.size) }}
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

const displayName = computed(() => truncateFileName(props.file.name || 'File'));

const formatSize = (bytes: number) => {
  if (!bytes) return '0 B';
  const k = 1024;
  const sizes = ['B', 'KB', 'MB', 'GB'];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + ' ' + sizes[i];
};
</script>

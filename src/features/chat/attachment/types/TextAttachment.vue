<template>
  <AttachmentPreviewBase 
    :file="file" 
    :index="index" 
    size="medium"
    @remove="emit('remove', index)"
  >
    <div class="w-full h-full flex flex-col items-center justify-center p-2">
      <!-- Text Icon -->
      <div class="w-6 h-6 rounded bg-blue-500/10 flex items-center justify-center mb-1">
        <svg
          width="12"
          height="12"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
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
      <!-- Text Content Preview -->
      <div 
        v-if="previewText" 
        class="text-[8px] text-[var(--primary-text)] opacity-60 font-mono text-center leading-tight"
        :title="previewText"
      >
        {{ previewText }}
      </div>
      <div 
        v-else 
        class="text-[8px] text-[var(--primary-text)] opacity-60 font-mono"
      >
        TXT
      </div>
    </div>
  </AttachmentPreviewBase>
</template>

<script setup lang="ts">
import { computed } from "vue";
import AttachmentPreviewBase from "../AttachmentPreviewBase.vue";
import type { Attachment } from "../../../../core/stores/chatManager";

const props = defineProps<{
  file: Attachment;
  index: number;
}>();

const emit = defineEmits<{ (e: "remove", index: number): void }>();

const previewText = computed(() => {
  if (!props.file.extractedText) return "";
  
  // Extract first line or truncate to fit
  const firstLine = props.file.extractedText.split('\n')[0];
  return firstLine.length > 20 ? firstLine.substring(0, 20) + '...' : firstLine;
});
</script>
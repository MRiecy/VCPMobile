<template>
  <AttachmentPreviewBase 
    :file="file" 
    :index="index" 
    size="medium"
    @remove="emit('remove', index)"
  >
    <div class="w-full h-full flex flex-col items-center justify-center p-2">
      <!-- Code Icon -->
      <div class="w-6 h-6 rounded bg-green-500/10 flex items-center justify-center mb-1">
        <svg
          width="12"
          height="12"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          stroke-linejoin="round"
          class="text-green-500"
        >
          <polyline points="16 18 22 12 16 6"></polyline>
          <polyline points="8 6 2 12 8 18"></polyline>
        </svg>
      </div>
      <!-- Code Language Indicator -->
      <div class="text-[8px] text-[var(--primary-text)] opacity-60 font-mono">
        {{ file.name.split('.').pop()?.toUpperCase() || 'CODE' }}
      </div>
    </div>
  </AttachmentPreviewBase>
</template>

<script setup lang="ts">
import AttachmentPreviewBase from "../AttachmentPreviewBase.vue";
import type { Attachment } from "../../../../core/stores/chatManager";

defineProps<{
  file: Attachment;
  index: number;
}>();

const emit = defineEmits<{ (e: "remove", index: number): void }>();
</script>
<template>
  <AttachmentPreviewBase 
    :file="file" 
    :index="index" 
    size="medium"
    @remove="emit('remove', index)"
  >
    <div class="w-full h-full flex items-center justify-center">
      <div class="w-8 h-8 rounded-lg bg-purple-500/10 flex items-center justify-center border border-purple-500/20">
        <svg
          width="14"
          height="14"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          stroke-linejoin="round"
          class="text-purple-500"
        >
          <polygon points="11 5 6 9 2 9 2 15 6 15 11 19 11 5"></polygon>
          <path d="M19.07 4.93a10 10 0 0 1 0 14.14M15.54 8.46a5 5 0 0 1 0 7.07"></path>
        </svg>
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

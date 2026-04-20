<template>
  <AttachmentPreviewBase 
    :file="file" 
    :index="index" 
    size="medium"
    @remove="emit('remove', index)"
  >
    <div class="w-full h-full flex items-center justify-center">
      <div class="w-8 h-8 rounded-lg bg-gray-500/10 flex items-center justify-center border border-gray-500/20">
        <svg
          width="14"
          height="14"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2.5"
          stroke-linecap="round"
          stroke-linejoin="round"
          class="text-gray-500"
        >
          <path
            d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"
          ></path>
          <polyline points="14 2 14 8 20 8"></polyline>
          <line x1="16" y1="13" x2="8" y2="13"></line>
          <line x1="16" y1="17" x2="8" y2="17"></line>
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

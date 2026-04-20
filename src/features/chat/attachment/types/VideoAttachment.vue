<template>
  <AttachmentPreviewBase 
    :file="file" 
    :index="index" 
    @remove="emit('remove', index)"
  >
    <div class="relative w-full h-full">
      <!-- Video Thumbnail -->
      <div class="w-full h-full rounded-xl overflow-hidden bg-black/5 dark:bg-white/5">
        <img 
          :src="thumbnailSrc" 
          :alt="file.name"
          class="w-full h-full object-cover"
          loading="lazy"
        />
        <!-- Video Play Button -->
        <div class="absolute inset-0 flex items-center justify-center">
          <div class="w-8 h-8 rounded-full bg-blue-500/20 backdrop-blur-md flex items-center justify-center border border-blue-500/30">
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

const props = defineProps<{
  file: Attachment;
  index: number;
}>();

const emit = defineEmits<{ (e: "remove", index: number): void }>();

const thumbnailSrc = computed(() => {
  // Priority use thumbnailPath if available
  const src = props.file.thumbnailPath || props.file.src;
  if (!src) return "";
  
  if (
    src.startsWith("http") ||
    src.startsWith("data:") ||
    src.startsWith("blob:")
  ) {
    return src;
  }
  try {
    return convertFileSrc(src.replace("file://", ""));
  } catch (e) {
    return "";
  }
});
</script>
<template>
  <AttachmentPreviewBase 
    :file="file" 
    :index="index" 
    :size="size"
    :show-remove="showRemove"
    @remove="emit('remove', index)"
  >
    <!-- Image Card -->
    <div class="w-full h-full rounded-xl overflow-hidden bg-black/5 dark:bg-white/5">
      <img 
        :src="safeSrc" 
        :alt="file.name"
        class="w-full h-full object-cover"
        loading="lazy"
      />
    </div>
  </AttachmentPreviewBase>
</template>

<script setup lang="ts">
import { computed } from "vue";
import { convertFileSrc } from "@tauri-apps/api/core";
import AttachmentPreviewBase from "../AttachmentPreviewBase.vue";
import type { Attachment } from "../../../../core/types/chat";

interface Props {
  file: Attachment;
  index: number;
  size?: 'small' | 'medium' | 'large';
  showRemove?: boolean;
}

const props = withDefaults(defineProps<Props>(), {
  size: 'medium',
  showRemove: false
});

const emit = defineEmits<{ (e: "remove", index: number): void }>();

const safeSrc = computed(() => {
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
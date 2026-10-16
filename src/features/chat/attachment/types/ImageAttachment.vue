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
        v-if="safeSrc"
        :src="safeSrc" 
        :alt="file.name"
        class="w-full h-full object-cover"
        loading="lazy"
      />
      <span v-else class="flex h-full items-center justify-center text-[10px] opacity-50">图片</span>
    </div>
  </AttachmentPreviewBase>
</template>

<script setup lang="ts">
import { ref, watch } from "vue";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
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

const safeSrc = ref("");
watch([() => props.file.hash, () => props.file.thumbnailPath], async ([hash, thumbnail], _old, onCleanup) => {
  let active = true;
  let ownedPath: string | undefined;
  onCleanup(() => {
    active = false;
    if (ownedPath) void invoke("release_attachment_image", { path: ownedPath }).catch(console.warn);
  });
  safeSrc.value = "";
  try {
    if (thumbnail) {
      safeSrc.value = convertFileSrc(thumbnail.replace("file://", ""));
    } else if (hash) {
      const image = await invoke<{ path: string }>("get_attachment_image", {
        hash, purpose: "display", maxDimension: 512,
      });
      if (!active) {
        await invoke("release_attachment_image", { path: image.path }).catch(console.warn);
        return;
      }
      ownedPath = image.path;
      safeSrc.value = convertFileSrc(image.path);
    }
  } catch (error) {
    console.warn("[ImageAttachment] Preview unavailable:", error);
  }
}, { immediate: true });
</script>

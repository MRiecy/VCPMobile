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
        :key="safeSrc"
        :src="safeSrc" 
        :alt="file.name"
        class="w-full h-full object-cover"
        loading="lazy"
        @error="handleImageError"
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
const handleImageError = ref<(event: Event) => void>(() => {});
watch([() => props.file.hash, () => props.file.thumbnailPath], ([hash, thumbnail], _old, onCleanup) => {
  let active = true;
  let requestedFallback = false;
  let ownedPath: string | undefined;
  const releaseOwnedImage = () => {
    const path = ownedPath;
    ownedPath = undefined;
    if (path) void invoke("release_attachment_image", { path }).catch(console.warn);
  };
  onCleanup(() => {
    active = false;
    releaseOwnedImage();
  });
  safeSrc.value = "";
  const loadFallback = async () => {
    if (!active || requestedFallback || !hash) return;
    requestedFallback = true;
    try {
      const image = await invoke<{ path: string }>("get_attachment_image", {
        hash, purpose: "display", maxDimension: 512,
      });
      if (!active) {
        await invoke("release_attachment_image", { path: image.path }).catch(console.warn);
        return;
      }
      ownedPath = image.path;
      safeSrc.value = convertFileSrc(image.path);
    } catch (error) {
      if (active) {
        releaseOwnedImage();
        safeSrc.value = "";
        console.warn("[ImageAttachment] Preview unavailable:", error);
      }
    }
  };
  handleImageError.value = (event) => {
    const image = event.target as HTMLImageElement | null;
    if (!active || image?.getAttribute("src") !== safeSrc.value) return;
    safeSrc.value = "";
    releaseOwnedImage();
    // A cache miss gets one bounded derivative per resource identity. A failed
    // derivative stays a placeholder instead of starting an image-error loop.
    void loadFallback();
  };
  if (thumbnail) {
    try {
      safeSrc.value = convertFileSrc(thumbnail.replace("file://", ""));
    } catch {
      void loadFallback();
    }
  } else {
    void loadFallback();
  }
}, { immediate: true });
</script>

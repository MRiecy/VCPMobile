<script setup lang="ts">
import { ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { ExternalLink } from "lucide-vue-next";
import AttachmentViewer from "./AttachmentViewer.vue";
import AttachmentRenderer from '../../features/chat/attachment/AttachmentRenderer.vue';

interface Attachment {
  type: string;
  src: string;
  resolvedSrc?: string;
  name: string;
  size: number;
  hash?: string;
  extractedText?: string;
  thumbnailPath?: string;
  id?: string;
  progress?: number;
  status?: string;
  internalPath?: string;
  imageFrames?: string[];
  createdAt?: number;
}

defineProps<{
  attachments: Attachment[];
}>();

const emit = defineEmits<{ (e: "remove", index: number): void }>();

const isViewerOpen = ref(false);
const activeFile = ref<Attachment | null>(null);

const openViewer = (att: Attachment) => {
  activeFile.value = att;
  isViewerOpen.value = true;
};

const openExternal = async (path: string) => {
  try {
    await invoke("open_file", { path });
  } catch (e) {
    console.error("[AttachmentPreview] Open failed:", e);
  }
};

const handleRemove = (index: number) => {
  emit("remove", index);
};
</script>

<template>
  <div
    class="vcp-attachment-preview flex flex-col gap-2 mt-2 w-full max-w-full overflow-hidden"
  >
    <div
      v-for="(att, index) in attachments"
      :key="index"
      class="attachment-item relative"
      @click="openViewer(att)"
    >
      <AttachmentRenderer
        :file="att"
        :index="index"
        @remove="handleRemove(index)"
      />
      
      <button
        v-if="!att.type.startsWith('image/') && !att.type.startsWith('audio/') && !att.type.startsWith('video/')"
        @click.stop="openExternal(att.src)"
        class="absolute top-2 right-2 p-2 opacity-30 hover:opacity-100 transition-opacity z-10"
      >
        <ExternalLink :size="16" />
      </button>
    </div>

    <AttachmentViewer
      :file="activeFile"
      :is-open="isViewerOpen"
      @close="isViewerOpen = false"
      @open-external="openExternal"
    />
  </div>
</template>

<style scoped>
audio::-webkit-media-controls-enclosure {
  background-color: rgba(255, 255, 255, 0.05);
}
</style>

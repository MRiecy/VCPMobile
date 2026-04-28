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
</script>

<template>
  <div
    class="vcp-attachment-preview flex flex-wrap gap-3 mt-3 w-full max-w-full overflow-hidden"
  >
    <div
      v-for="(att, index) in attachments"
      :key="index"
      class="attachment-item relative group"
      @click="openViewer(att)"
    >
      <AttachmentRenderer
        :file="att"
        :index="index"
        :show-remove="false"
      />
      
      <!-- 外部打开按钮 (仅针对非媒体文件) -->
      <button
        v-if="!att.type.startsWith('image/') && !att.type.startsWith('audio/') && !att.type.startsWith('video/')"
        @click.stop="openExternal(att.src)"
        class="absolute top-1 right-1 p-1 bg-black/20 dark:bg-white/10 rounded-lg opacity-0 group-hover:opacity-100 hover:bg-black/40 dark:hover:bg-white/20 transition-all z-10 backdrop-blur-sm"
      >
        <ExternalLink :size="12" class="text-white/70" />
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

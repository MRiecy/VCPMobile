<script setup lang="ts">
import { computed, watch } from "vue";
import { useModalHistory } from "../../../core/composables/useModalHistory";
import { convertFileSrc } from "@tauri-apps/api/core";
import { X, ExternalLink, Download } from "lucide-vue-next";
import MarkdownBlock from "../blocks/MarkdownBlock.vue";

interface Attachment {
  type: string;
  src: string;
  name: string;
  size: number;
  extractedText?: string;
}

const props = defineProps<{
  file: Attachment | null;
  isOpen: boolean;
}>();

const emit = defineEmits(["close", "open-external"]);

const { registerModal, unregisterModal } = useModalHistory();
const modalId = 'AttachmentViewer';

watch(() => props.isOpen, (newVal) => {
  if (newVal) {
    registerModal(modalId, close);
  } else {
    unregisterModal(modalId);
  }
});

const isImage = computed(() => props.file?.type.startsWith("image/"));
const isText = computed(() => !!props.file?.extractedText);

const renderSrc = computed(() => {
  if (!props.file?.src) return "";
  if (
    props.file.src.startsWith("http") ||
    props.file.src.startsWith("data:") ||
    props.file.src.startsWith("blob:")
  )
    return props.file.src;
  try {
    return convertFileSrc(props.file.src.replace("file://", ""));
  } catch (e) {
    return "";
  }
});

const close = () => emit("close");
</script>

<template>
  <Transition name="viewer-fade">
    <div
      v-if="isOpen && file"
      class="vcp-attachment-viewer fixed inset-0 z-[1000] flex flex-col bg-[#f0f4f8] dark:bg-[#121e23] pointer-events-auto"
      @click.self="close"
    >
      <!-- Toolbar -->
      <div
        class="flex items-center justify-between px-4 pt-[calc(env(safe-area-inset-top,24px)+8px)] pb-3 bg-white/80 dark:bg-gray-900/80 backdrop-blur-md border-b border-black/5 dark:border-white/5 shrink-0 shadow-sm z-10"
      >
        <div class="flex flex-col overflow-hidden mr-4 min-w-0">
          <span class="text-sm font-bold text-gray-800 dark:text-gray-200 truncate">{{
            file.name
          }}</span>
          <span class="text-[10px] text-gray-400 dark:text-gray-500 uppercase tracking-widest">{{
            file.type
          }}</span>
        </div>
        <div class="flex items-center gap-1">
          <button
            @click="$emit('open-external', file.src)"
            class="p-2 -mr-1 rounded-full text-gray-500 hover:text-gray-800 dark:text-gray-400 dark:hover:text-gray-200 transition-colors active:bg-black/5 dark:active:bg-white/5"
          >
            <ExternalLink :size="20" />
          </button>
          <button
            @click="close"
            class="p-2 -mr-2 rounded-full text-gray-500 hover:text-gray-800 dark:text-gray-400 dark:hover:text-gray-200 transition-colors active:bg-black/5 dark:active:bg-white/5"
          >
            <X :size="24" />
          </button>
        </div>
      </div>

      <!-- Main Content -->
      <div
        class="flex-1 overflow-auto vcp-scrollable pb-[env(safe-area-inset-bottom)]"
      >
        <!-- Text/Code/MD Viewer -->
        <div
          v-if="isText"
          class="w-full px-5 py-6 text-[15px] leading-relaxed"
        >
          <MarkdownBlock :content="file.extractedText!" :is-streaming="false" />
        </div>

        <!-- Image Viewer -->
        <div
          v-else-if="isImage"
          class="h-full w-full flex items-center justify-center p-4"
        >
          <img
            :src="renderSrc"
            class="max-w-full max-h-full object-contain rounded-lg shadow-2xl animate-zoom-in"
            @click.stop
          />
        </div>

        <!-- Unsupported Format -->
        <div v-else class="h-full flex flex-col items-center justify-center gap-6 p-4">
          <div
            class="w-20 h-20 rounded-3xl bg-gray-100 dark:bg-gray-800 flex items-center justify-center border border-black/5 dark:border-white/10"
          >
            <Download :size="40" class="text-gray-400 dark:text-gray-500" />
          </div>
          <div class="text-center">
            <p class="text-gray-800 dark:text-gray-200 font-bold">暂不支持在线预览该格式</p>
            <p class="text-xs text-gray-400 dark:text-gray-500 mt-1">
              请点击右上角按钮使用系统应用打开
            </p>
          </div>
        </div>
      </div>
    </div>
  </Transition>
</template>

<style scoped>
.viewer-fade-enter-active,
.viewer-fade-leave-active {
  transition: all 0.3s cubic-bezier(0.16, 1, 0.3, 1);
}

.viewer-fade-enter-from,
.viewer-fade-leave-to {
  opacity: 0;
  transform: scale(1.05);
}

.animate-zoom-in {
  animation: zoomIn 0.4s cubic-bezier(0.16, 1, 0.3, 1);
}

@keyframes zoomIn {
  from {
    opacity: 0;
    transform: scale(0.9);
  }

  to {
    opacity: 1;
    transform: scale(1);
  }
}
</style>

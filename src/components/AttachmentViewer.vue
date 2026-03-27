<script setup lang="ts">
import { computed } from 'vue';
import { convertFileSrc } from '@tauri-apps/api/core';
import { X, ExternalLink, Download } from 'lucide-vue-next';
import MarkdownBlock from './blocks/MarkdownBlock.vue';

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

const emit = defineEmits(['close', 'open-external']);

const isImage = computed(() => props.file?.type.startsWith('image/'));
const isText = computed(() => !!props.file?.extractedText);

const renderSrc = computed(() => {
  if (!props.file?.src) return '';
  if (props.file.src.startsWith('http')) return props.file.src;
  try {
    return convertFileSrc(props.file.src.replace('file://', ''));
  } catch (e) {
    return '';
  }
});

const close = () => emit('close');
</script>

<template>
  <Transition name="viewer-fade">
    <div v-if="isOpen && file" class="vcp-attachment-viewer fixed inset-0 z-[1000] flex flex-col bg-black/90 backdrop-blur-2xl">
      <!-- Toolbar -->
      <div class="flex items-center justify-between px-4 py-4 border-b border-white/10 shrink-0">
        <div class="flex flex-col overflow-hidden mr-4">
          <span class="text-sm font-bold text-white truncate">{{ file.name }}</span>
          <span class="text-[10px] text-white/40 uppercase tracking-widest">{{ file.type }}</span>
        </div>
        <div class="flex items-center gap-2">
          <button @click="$emit('open-external', file.src)" class="p-2 hover:bg-white/10 rounded-full text-white/70 transition-colors">
            <ExternalLink :size="20" />
          </button>
          <button @click="close" class="p-2 hover:bg-white/10 rounded-full text-white transition-colors">
            <X :size="24" />
          </button>
        </div>
      </div>

      <!-- Main Content -->
      <div class="flex-1 overflow-auto custom-scrollbar p-4 flex flex-col items-center justify-center">
        
        <!-- Text/Code/MD Viewer -->
        <div v-if="isText" class="w-full max-w-4xl bg-white/5 rounded-2xl p-6 border border-white/10 shadow-2xl">
          <MarkdownBlock :content="file.extractedText!" :is-streaming="false" />
        </div>

        <!-- Image Viewer -->
        <div v-else-if="isImage" class="relative group max-w-full max-h-full flex items-center justify-center">
          <img 
            :src="renderSrc" 
            class="max-w-full max-h-[80vh] object-contain rounded-lg shadow-2xl animate-zoom-in"
            @click.stop
          />
        </div>

        <!-- Unsupported Format -->
        <div v-else class="flex flex-col items-center gap-6 opacity-50">
          <div class="w-20 h-20 rounded-3xl bg-white/5 flex items-center justify-center border border-white/10">
            <Download :size="40" />
          </div>
          <div class="text-center">
            <p class="text-white font-bold">暂不支持在线预览该格式</p>
            <p class="text-xs text-white/40 mt-1">请点击右上角按钮使用系统应用打开</p>
          </div>
        </div>

      </div>
    </div>
  </Transition>
</template>

<style scoped>
.viewer-fade-enter-active, .viewer-fade-leave-active {
  transition: all 0.3s cubic-bezier(0.16, 1, 0.3, 1);
}
.viewer-fade-enter-from, .viewer-fade-leave-to {
  opacity: 0;
  transform: scale(1.05);
}

.animate-zoom-in {
  animation: zoomIn 0.4s cubic-bezier(0.16, 1, 0.3, 1);
}

@keyframes zoomIn {
  from { opacity: 0; transform: scale(0.9); }
  to { opacity: 1; transform: scale(1); }
}

.custom-scrollbar::-webkit-scrollbar {
  width: 4px;
}
.custom-scrollbar::-webkit-scrollbar-thumb {
  background: rgba(255, 255, 255, 0.1);
  border-radius: 4px;
}
</style>

<script setup lang="ts">
import { ref } from 'vue';
import { convertFileSrc, invoke } from '@tauri-apps/api/core';
import { FileIcon, FileText, Music, Video, ExternalLink } from 'lucide-vue-next';
import AttachmentViewer from './AttachmentViewer.vue';

interface Attachment {
  type: string;
  src: string;
  resolvedSrc?: string;
  name: string;
  size: number;
  hash?: string;
  extractedText?: string;
  thumbnailPath?: string;
}

defineProps<{
  attachments: Attachment[];
}>();

const isViewerOpen = ref(false);
const activeFile = ref<Attachment | null>(null);

const formatSize = (bytes: number) => {
  if (bytes === 0) return '0 B';
  const k = 1024;
  const sizes = ['B', 'KB', 'MB', 'GB'];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + ' ' + sizes[i];
};

const getFileIcon = (type: string) => {
  if (type.startsWith('image/')) return null; // 图片直接显示预览
  if (type.startsWith('audio/')) return Music;
  if (type.startsWith('video/')) return Video;
  if (type.startsWith('text/') || type.includes('json') || type.includes('javascript')) return FileText;
  return FileIcon;
};

const getAttachmentRenderSrc = (att: Attachment) => {
  // 优先使用缩略图，后端已自动解析
  const path = att.thumbnailPath || att.src;
  if (!path) return '';
  if (path.startsWith('http')) return path;
  try {
    return convertFileSrc(path.replace('file://', ''));
  } catch (e) {
    return '';
  }
};

const openViewer = (att: Attachment) => {
  activeFile.value = att;
  isViewerOpen.value = true;
};

const openExternal = async (path: string) => {
  try {
    await invoke('open_file', { path });
  } catch (e) {
    console.error('[AttachmentPreview] Open failed:', e);
  }
};
</script>

<template>
  <div class="vcp-attachment-preview flex flex-col gap-2 mt-2 w-full max-w-full overflow-hidden">
    <div v-for="(att, index) in attachments" :key="index" class="attachment-item">
      
      <!-- 图片类型 -->
      <div v-if="att.type.startsWith('image/')" class="image-preview relative group" @click="openViewer(att)">
        <img 
          :src="getAttachmentRenderSrc(att)" 
          :alt="att.name"
          class="max-w-[200px] max-h-[300px] rounded-xl border border-white/10 shadow-sm object-cover cursor-zoom-in active:scale-95 transition-all"
          loading="lazy"
        />
        <div class="absolute bottom-2 left-2 px-2 py-0.5 bg-black/40 backdrop-blur-md rounded-md text-[9px] text-white opacity-0 group-hover:opacity-100 transition-opacity">
          {{ formatSize(att.size) }}
        </div>
      </div>

      <!-- 音频类型 -->
      <div v-else-if="att.type.startsWith('audio/')" class="audio-container bg-white/5 border border-white/10 rounded-xl p-3 flex flex-col gap-2 w-full max-w-[280px]">
        <div class="flex items-center gap-3" @click="openViewer(att)">
          <div class="w-10 h-10 rounded-full bg-blue-500/20 flex items-center justify-center text-blue-400">
            <Music :size="20" />
          </div>
          <div class="flex flex-col overflow-hidden">
            <span class="text-xs font-bold truncate text-[var(--primary-text)]">{{ att.name }}</span>
            <span class="text-[10px] opacity-40 uppercase">{{ formatSize(att.size) }}</span>
          </div>
        </div>
        <audio controls class="w-full h-8 opacity-80 mt-1 scale-90 -translate-x-[5%] origin-left">
          <source :src="getAttachmentRenderSrc(att)" :type="att.type">
        </audio>
      </div>

      <!-- 视频类型 -->
      <div v-else-if="att.type.startsWith('video/')" class="video-container relative max-w-[280px] rounded-xl overflow-hidden border border-white/10 bg-black/20" @click="openViewer(att)">
        <video class="w-full aspect-video object-cover" preload="metadata">
          <source :src="getAttachmentRenderSrc(att)" :type="att.type">
        </video>
        <div class="absolute inset-0 flex items-center justify-center bg-black/20 group cursor-pointer active:scale-95 transition-all">
          <div class="w-12 h-12 rounded-full bg-white/10 backdrop-blur-md flex items-center justify-center border border-white/20">
            <Video :size="24" class="text-white fill-current" />
          </div>
        </div>
        <div class="absolute bottom-0 inset-x-0 p-2 bg-gradient-to-t from-black/60 to-transparent text-[10px] text-white truncate">
          {{ att.name }} ({{ formatSize(att.size) }})
        </div>
      </div>

      <!-- 通用文件类型 -->
      <div v-else class="file-card bg-white/5 hover:bg-white/10 border border-white/10 rounded-xl p-3 flex items-center gap-3 w-full max-w-[300px] transition-all active:scale-[0.98]" @click="openViewer(att)">
        <div class="w-10 h-10 rounded-lg bg-white/5 flex items-center justify-center shrink-0">
          <component :is="getFileIcon(att.type)" :size="20" class="opacity-60" />
        </div>
        <div class="flex flex-col overflow-hidden flex-1">
          <span class="text-xs font-bold truncate text-[var(--primary-text)]">{{ att.name }}</span>
          <div class="flex items-center gap-2 text-[10px] opacity-40">
            <span class="uppercase">{{ formatSize(att.size) }}</span>
            <span v-if="att.extractedText" class="px-1.5 py-0.5 bg-blue-500/20 text-blue-400 rounded-sm scale-90">TEXT_EXTRACTED</span>
          </div>
        </div>
        <button @click.stop="openExternal(att.src)" class="p-2 opacity-30 hover:opacity-100 transition-opacity">
          <ExternalLink :size="16" />
        </button>
      </div>

    </div>

    <!-- 内置轻量预览器 -->
    <AttachmentViewer 
      :file="activeFile" 
      :is-open="isViewerOpen" 
      @close="isViewerOpen = false" 
      @open-external="openExternal"
    />
  </div>
</template>

<style scoped>
/* 针对不同音频控制器的样式适配 (Webkit) */
audio::-webkit-media-controls-enclosure {
  background-color: rgba(255, 255, 255, 0.05);
}
</style>

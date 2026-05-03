<script setup lang="ts">
import { ref, onMounted } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { FileText, Trash2, Copy, ChevronLeft, ChevronRight } from 'lucide-vue-next';

interface LogFile {
  filename: string;
  created_at: number;
  size_bytes: number;
}

const files = ref<LogFile[]>([]);
const loading = ref(false);
const currentFile = ref<string | null>(null);
const fileContent = ref<string>('');
const currentPage = ref(0);
const linesPerPage = 500;
const totalPages = ref(0);
const lines = ref<string[]>([]);

const formatBytes = (bytes: number) => {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
};

const formatTime = (ts: number) => {
  const d = new Date(ts * 1000);
  return d.toLocaleString('zh-CN', { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' });
};

const loadFiles = async () => {
  loading.value = true;
  try {
    files.value = await invoke<LogFile[]>('list_sync_log_files');
  } catch (e) {
    console.error('[SyncLogBrowser] Failed to list files:', e);
  } finally {
    loading.value = false;
  }
};

const openFile = async (filename: string) => {
  loading.value = true;
  try {
    const content = await invoke<string>('read_sync_log_file', { filename });
    fileContent.value = content;
    lines.value = content.split('\n').filter(l => l.trim());
    totalPages.value = Math.max(1, Math.ceil(lines.value.length / linesPerPage));
    currentPage.value = 0;
    currentFile.value = filename;
  } catch (e) {
    console.error('[SyncLogBrowser] Failed to read file:', e);
  } finally {
    loading.value = false;
  }
};

const closeFile = () => {
  currentFile.value = null;
  fileContent.value = '';
  lines.value = [];
};

const copyCurrentFile = async () => {
  if (!currentFile.value) return;
  try {
    await navigator.clipboard.writeText(fileContent.value);
  } catch (e) {
    console.error('[SyncLogBrowser] Copy failed:', e);
  }
};

const clearOldLogs = async () => {
  if (!confirm('确定要清理 7 天前的同步日志吗？')) return;
  try {
    const removed = await invoke<number>('clear_old_sync_logs', { keepDays: 7 });
    await loadFiles();
    alert(`已清理 ${removed} 个旧日志文件`);
  } catch (e) {
    console.error('[SyncLogBrowser] Clear failed:', e);
  }
};

const visibleLines = () => {
  const start = currentPage.value * linesPerPage;
  return lines.value.slice(start, start + linesPerPage);
};

const prevPage = () => {
  if (currentPage.value > 0) currentPage.value--;
};

const nextPage = () => {
  if (currentPage.value < totalPages.value - 1) currentPage.value++;
};

onMounted(() => {
  loadFiles();
});
</script>

<template>
  <div class="h-full flex flex-col overflow-hidden">
    <!-- File List -->
    <div v-if="!currentFile" class="flex-1 overflow-y-auto">
      <div v-if="loading" class="flex items-center justify-center h-32 text-white/30 text-xs">
        加载中...
      </div>
      <div v-else-if="files.length === 0" class="flex flex-col items-center justify-center h-64 text-white/20 text-xs">
        <FileText :size="32" class="mb-3 opacity-30" />
        <p>暂无同步日志</p>
      </div>
      <div v-else class="divide-y divide-white/5">
        <div v-for="file in files" :key="file.filename"
          @click="openFile(file.filename)"
          class="flex items-center justify-between px-4 py-3 active:bg-white/5 cursor-pointer">
          <div class="flex items-center gap-3 min-w-0">
            <FileText :size="16" class="text-blue-400 shrink-0" />
            <div class="min-w-0">
              <div class="text-xs font-mono truncate">{{ file.filename }}</div>
              <div class="text-[10px] text-white/30 mt-0.5">{{ formatTime(file.created_at) }} · {{ formatBytes(file.size_bytes) }}</div>
            </div>
          </div>
          <ChevronLeft :size="14" class="text-white/20 rotate-180 shrink-0" />
        </div>
      </div>

      <!-- Clear button -->
      <div v-if="files.length > 0" class="px-4 py-6">
        <button @click="clearOldLogs"
          class="flex items-center justify-center gap-2 w-full py-2.5 rounded-lg bg-red-500/10 text-red-400 text-xs font-bold tracking-wider active:bg-red-500/20">
          <Trash2 :size="12" />
          清理 7 天前的日志
        </button>
      </div>
    </div>

    <!-- File Content -->
    <div v-else class="flex-1 flex flex-col overflow-hidden">
      <!-- Content toolbar -->
      <div class="flex items-center justify-between px-4 py-2 border-b border-white/10">
        <button @click="closeFile" class="flex items-center gap-1 text-[10px] text-white/50 hover:text-white">
          <ChevronLeft :size="14" />
          返回列表
        </button>
        <button @click="copyCurrentFile"
          class="flex items-center gap-1 px-2 py-1 rounded text-[10px] text-white/50 hover:text-white hover:bg-white/10 transition-colors">
          <Copy :size="12" />
          复制
        </button>
      </div>

      <div class="flex-1 overflow-y-auto px-4 py-3 font-mono text-[10px] leading-relaxed">
        <div v-for="(line, i) in visibleLines()" :key="i"
          class="truncate text-white/70"
          :class="{
            'text-green-400': line.includes('[INFO]') && line.includes('success'),
            'text-red-400': line.includes('[Error]') || line.includes('failed') || line.includes('error'),
            'text-yellow-400': line.includes('WARN'),
          }">
          {{ line }}
        </div>
      </div>

      <!-- Pagination -->
      <div v-if="totalPages > 1" class="flex items-center justify-between px-4 py-2 border-t border-white/10 text-[10px]">
        <button @click="prevPage" :disabled="currentPage === 0"
          class="p-1 text-white/50 disabled:opacity-20">
          <ChevronLeft :size="14" />
        </button>
        <span class="text-white/40 font-mono">{{ currentPage + 1 }} / {{ totalPages }}</span>
        <button @click="nextPage" :disabled="currentPage >= totalPages - 1"
          class="p-1 text-white/50 disabled:opacity-20">
          <ChevronRight :size="14" />
        </button>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, onMounted } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { shareFileNative } from 'tauri-plugin-vcp-mobile';
import { FileText, Trash2, Copy, Share2, ChevronLeft, ChevronRight } from 'lucide-vue-next';
import { useOverlayStore } from '../../../core/stores/overlay';
import { useNotificationStore } from '../../../core/stores/notification';

interface LogFile {
  filename: string;
  created_at: number;
  size_bytes: number;
}

interface LogCleanupResult {
  removed: number;
  failed: number;
}

const files = ref<LogFile[]>([]);
const loading = ref(false);
const errorText = ref('');

const overlayStore = useOverlayStore();
const notificationStore = useNotificationStore();
const currentFile = ref<string | null>(null);
const fileContent = ref<string>('');
const shareBusy = ref(false);
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
  errorText.value = '';
  try {
    files.value = await invoke<LogFile[]>('list_sync_log_files');
  } catch (e) {
    console.error('[SyncLogBrowser] Failed to list files:', e);
    files.value = [];
    errorText.value = '无法加载同步日志，请稍后再试。';
  } finally {
    loading.value = false;
  }
};

const openFile = async (filename: string) => {
  loading.value = true;
  errorText.value = '';
  try {
    const content = await invoke<string>('read_sync_log_file', { filename });
    fileContent.value = content;
    lines.value = content.split('\n').filter(l => l.trim());
    totalPages.value = Math.max(1, Math.ceil(lines.value.length / linesPerPage));
    currentPage.value = 0;
    currentFile.value = filename;
  } catch (e) {
    console.error('[SyncLogBrowser] Failed to read file:', e);
    notificationStore.addNotification({
      type: 'error',
      message: '无法打开此同步日志，请稍后再试',
      toastOnly: true
    });
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
    notificationStore.addNotification({
      type: 'success',
      message: '同步日志已复制',
      toastOnly: true
    });
  } catch (e) {
    console.error('[SyncLogBrowser] Copy failed:', e);
    notificationStore.addNotification({
      type: 'error',
      message: '复制同步日志失败，请稍后再试',
      toastOnly: true
    });
  }
};

const shareCurrentFile = async () => {
  if (!currentFile.value || shareBusy.value) return;
  shareBusy.value = true;
  try {
    const path = await invoke<string>('prepare_sync_log_share_file', {
      filename: currentFile.value
    });
    await shareFileNative(path);
    notificationStore.addNotification({
      type: 'success',
      message: '已打开系统分享面板',
      toastOnly: true
    });
  } catch (e) {
    console.error('[SyncLogBrowser] Share failed:', e);
    notificationStore.addNotification({
      type: 'error',
      message: '分享同步日志失败，请稍后再试',
      toastOnly: true
    });
  } finally {
    shareBusy.value = false;
  }
};

const clearOldLogs = async () => {
  const confirmed = await overlayStore.showConfirm({
    title: '清理日志',
    message: '确定要清理 7 天前的同步日志吗？',
    isDanger: true
  });
  if (!confirmed) return;
  try {
    const result = await invoke<LogCleanupResult>('clear_old_sync_logs', { keepDays: 7 });
    await loadFiles();
    notificationStore.addNotification({
      type: result.failed > 0 ? 'warning' : 'success',
      message: result.failed > 0
        ? `已清理 ${result.removed} 个日志，${result.failed} 个未能删除`
        : `已清理 ${result.removed} 个旧日志文件`,
      toastOnly: true
    });
  } catch (e) {
    console.error('[SyncLogBrowser] Clear failed:', e);
    notificationStore.addNotification({
      type: 'error',
      message: '清理日志失败，请稍后再试',
      toastOnly: true
    });
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

const lineClass = (line: string) => {
  if (line.includes('[ERROR]')) return 'text-red-400';
  if (line.includes('[WARN]')) return 'text-yellow-400';
  if (line.includes('[TRACE]') || line.includes('[DEBUG]')) return 'text-white/40';
  if (line.includes('[INFO]') && /success|completed/i.test(line)) return 'text-green-400';
  return 'text-white/70';
};

onMounted(() => {
  loadFiles();
});
</script>

<template>
  <div class="h-full flex flex-col overflow-hidden">
    <!-- File List -->
    <div v-if="!currentFile" class="flex-1 overflow-y-auto no-rubber-band">
      <div v-if="loading" class="flex items-center justify-center h-32 text-white/30 text-xs">
        加载中...
      </div>
      <div v-else-if="errorText" class="mx-4 mt-4 flex items-center justify-between gap-3 border-l-2 border-red-500 bg-red-500/6 px-3 py-2 text-[11px] text-red-300">
        <span>{{ errorText }}</span>
        <button
          class="shrink-0 border-l-2 border-blue-400 px-2 py-1 text-[10px] text-blue-300"
          @click="loadFiles"
        >
          重新加载
        </button>
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
        <div class="flex items-center gap-1">
          <button @click="copyCurrentFile"
            class="flex min-h-9 items-center gap-1 px-3 text-[10px] text-white/50 transition-colors active:text-white">
            <Copy :size="13" />
            复制
          </button>
          <button
            :disabled="shareBusy"
            class="flex min-h-9 items-center gap-1 border-l border-white/10 px-3 text-[10px] text-blue-300 transition-colors active:text-white disabled:opacity-30"
            @click="shareCurrentFile"
          >
            <Share2 :size="13" />
            {{ shareBusy ? '分享中' : '分享' }}
          </button>
        </div>
      </div>

      <div class="flex-1 overflow-y-auto overflow-x-auto px-4 py-3 font-mono text-[10px] leading-relaxed min-w-0 no-rubber-band">
        <div v-for="(line, i) in visibleLines()" :key="i"
          class="whitespace-nowrap"
          :class="lineClass(line)">
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

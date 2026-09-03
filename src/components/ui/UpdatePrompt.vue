<script setup lang="ts">
import { watch, computed } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { openUrl } from '@tauri-apps/plugin-opener';
import { useModalHistory } from '../../core/composables/useModalHistory';
import { useUpdateStore } from '../../core/stores/update';
import { marked } from 'marked';
import { withCodeBlockClass } from '../../core/utils/astRenderer';
import DOMPurify from 'dompurify';

const props = defineProps<{
  isOpen: boolean;
  version: string;
  releaseNotes?: string | null;
  apkSize?: number | null;
}>();

const emit = defineEmits<{
  (e: 'confirm'): void;
  (e: 'dismiss'): void;
  (e: 'skip'): void;
  (e: 'update:isOpen', value: boolean): void;
}>();

const { registerModal, unregisterModal } = useModalHistory();
const modalId = 'UpdatePrompt';
const updateStore = useUpdateStore();

watch(
  () => props.isOpen,
  (newVal) => {
    if (newVal) {
      registerModal(modalId, () => {
        emit('dismiss');
        emit('update:isOpen', false);
      });
    } else {
      unregisterModal(modalId);
    }
  },
);

const handleDismiss = () => {
  emit('dismiss');
  emit('update:isOpen', false);
};

const handleConfirm = () => emit('confirm');
const handleSkip = () => {
  emit('skip');
  emit('update:isOpen', false);
};

const state = computed(() => updateStore.state);
const error = computed(() => updateStore.error);
const percent = computed(() => updateStore.progressPercent);
const currentVersion = computed(() => updateStore.info?.currentVersion || '');

const formatBytes = (bytes: number) => {
  if (bytes === 0) return '0 B';
  const k = 1024;
  const sizes = ['B', 'KB', 'MB', 'GB'];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + ' ' + sizes[i];
};

marked.setOptions({ breaks: true, gfm: true });

const releaseNotesHtml = computed(() => {
  if (!props.releaseNotes) return '';
  const parsed = withCodeBlockClass(marked.parse(props.releaseNotes) as string);
  return DOMPurify.sanitize(parsed as string);
});

const primaryText = computed(() => {
  switch (state.value) {
    case 'downloading':
      return `下载中 ${percent.value}%`;
    case 'verifying':
      return '校验中…';
    case 'installing':
      return '正在拉起安装器…';
    case 'readyToInstall':
      return '立即安装';
    default:
      return error.value?.retryable ? '重试' : '立即更新';
  }
});

const primaryDisabled = computed(() =>
  ['downloading', 'verifying', 'installing'].includes(state.value),
);

const openReleasePage = () => {
  const url = updateStore.info?.releasePageUrl;
  if (url) {
    openUrl(url).catch((e) => console.warn('[UpdatePrompt] open release page failed:', e));
  }
};

const openInstallPermission = () => {
  invoke('plugin:vcp-mobile|open_unknown_sources_settings').catch((e) =>
    console.warn('[UpdatePrompt] open unknown sources settings failed:', e),
  );
};

const cancelDownload = () => {
  updateStore.cancelDownload().catch((e) =>
    console.warn('[UpdatePrompt] cancel download failed:', e),
  );
};
</script>

<template>
  <Teleport to="body">
    <Transition name="prompt-fade">
      <div
        v-if="isOpen"
        class="fixed inset-0 z-dialog flex items-start justify-center pt-[12vh] bg-black/65"
        @click.self="handleDismiss"
      >
        <div
          class="bg-white dark:bg-[#101f26] w-11/12 max-w-sm rounded-lg shadow-lg border border-black/10 dark:border-white/10 p-5 relative"
        >
          <!-- Accent Bar -->
          <div class="absolute left-0 top-4 bottom-4 w-0.5 bg-blue-500 rounded-r"></div>

          <!-- 标题区 -->
          <div class="flex items-center justify-between mb-3 pl-2">
            <h3 class="text-sm font-bold text-gray-800 dark:text-gray-100">
              发现新版本
            </h3>
            <span
              class="px-1.5 py-0.5 rounded text-[10px] font-mono font-bold text-blue-600 dark:text-blue-400 border border-blue-500/30"
            >
              OTA
            </span>
          </div>

          <!-- 版本跃迁：一眼看懂「当前版本 → 新版本」 -->
          <div class="ml-2 mb-3 flex items-center gap-2 font-mono text-xs">
            <span
              v-if="currentVersion"
              class="px-2 py-1 rounded bg-black/5 dark:bg-white/5 text-gray-500 dark:text-gray-400"
            >
              v{{ currentVersion }}
            </span>
            <span v-if="currentVersion" class="text-gray-300 dark:text-gray-600">→</span>
            <span
              class="px-2 py-1 rounded bg-blue-500/10 border border-blue-500/25 text-blue-600 dark:text-blue-400 font-bold"
            >
              v{{ version }}
            </span>
            <span v-if="apkSize" class="ml-auto text-[10px] text-gray-400 dark:text-gray-500">
              {{ formatBytes(apkSize) }}
            </span>
          </div>

          <!-- 更新内容 -->
          <div v-if="releaseNotesHtml" class="ml-2 mb-3">
            <div
              class="text-[10px] font-bold uppercase tracking-wider text-gray-400 dark:text-gray-500 mb-1.5"
            >
              更新内容
            </div>
            <div
              class="vcp-markdown-block text-xs text-gray-700 dark:text-gray-300 max-h-[32vh] overflow-y-auto leading-relaxed p-3 bg-black/5 dark:bg-white/5 rounded border border-black/5 dark:border-white/5 custom-scrollbar"
              v-html="releaseNotesHtml"
            ></div>
          </div>

          <!-- 错误信息反馈 -->
          <div
            v-if="error"
            class="ml-2 text-xs text-red-500 bg-red-500/10 border border-red-500/20 rounded p-3 mb-3"
          >
            <div class="font-bold mb-1">更新失败（{{ error.stage }}）</div>
            <div class="font-mono break-all opacity-90 leading-tight">{{ error.message }}</div>
            <button
              v-if="error.stage === 'install' && error.retryable"
              class="mt-2 text-[11px] font-bold text-blue-600 dark:text-blue-400 underline underline-offset-2"
              @click="openInstallPermission"
            >
              前往系统授权设置
            </button>
          </div>

          <!-- 进度条 (下载/校验中展示) -->
          <div v-if="state === 'downloading' || state === 'verifying'" class="ml-2 mb-3 space-y-1.5">
            <div class="h-1 bg-black/10 dark:bg-white/10 rounded-full overflow-hidden">
              <div
                class="h-full bg-blue-500 transition-all duration-200"
                :style="{ width: (state === 'verifying' ? 100 : percent) + '%' }"
              />
            </div>
            <div class="flex justify-between text-[10px] font-mono text-gray-400 dark:text-gray-500">
              <span>{{ state === 'verifying' ? 'SHA-256 校验中' : `已下载 ${percent}%` }}</span>
              <span v-if="updateStore.status.total">
                {{ formatBytes(updateStore.status.downloaded) }} / {{ formatBytes(updateStore.status.total) }}
              </span>
            </div>
          </div>

          <!-- 次级操作：弱化为文本链接，避免与主操作争夺注意力 -->
          <div class="ml-2 flex items-center gap-4 mt-4 mb-3">
            <button
              class="text-[11px] text-gray-400 dark:text-gray-500 hover:text-gray-600 dark:hover:text-gray-300 underline underline-offset-2 transition-colors"
              @click="openReleasePage"
            >
              Release 页面
            </button>
            <button
              class="text-[11px] text-gray-400 dark:text-gray-500 hover:text-gray-600 dark:hover:text-gray-300 underline underline-offset-2 transition-colors"
              @click="handleSkip"
            >
              忽略此版本
            </button>
          </div>

          <!-- 主操作区：一个次按钮 + 一个主按钮 -->
          <div class="flex justify-end gap-2">
            <button
              v-if="state === 'downloading'"
              class="px-3 py-2 rounded text-xs font-bold text-red-500 hover:bg-red-500/10 transition-colors"
              @click="cancelDownload"
            >
              取消下载
            </button>
            <button
              v-else
              class="px-3 py-2 rounded text-xs font-bold text-gray-500 dark:text-gray-400 hover:bg-black/5 dark:hover:bg-white/5 transition-colors"
              @click="handleDismiss"
            >
              稍后
            </button>
            <button
              :disabled="primaryDisabled"
              class="px-4 py-2 rounded text-xs font-bold bg-blue-500 hover:bg-blue-600 text-white transition-colors disabled:opacity-50 disabled:pointer-events-none"
              @click="handleConfirm"
            >
              {{ primaryText }}
            </button>
          </div>
        </div>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
.custom-scrollbar::-webkit-scrollbar {
  width: 4px;
}
.custom-scrollbar::-webkit-scrollbar-track {
  background: transparent;
}
.custom-scrollbar::-webkit-scrollbar-thumb {
  background: rgba(156, 163, 175, 0.25);
  border-radius: 99px;
}

/* 仅透明度淡入淡出，禁止缩放弹跳 */
.prompt-fade-enter-active,
.prompt-fade-leave-active {
  transition: opacity 0.2s ease;
}
.prompt-fade-enter-from,
.prompt-fade-leave-to {
  opacity: 0;
}
</style>

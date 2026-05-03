<script setup lang="ts">
import { watch } from 'vue';
import { useModalHistory } from '../../core/composables/useModalHistory';

const props = defineProps<{
  isOpen: boolean;
  version: string;
  releaseNotes?: string | null;
  apkSize?: number | null;
}>();

const emit = defineEmits<{
  (e: 'confirm'): void;
  (e: 'dismiss'): void;
  (e: 'update:isOpen', value: boolean): void;
}>();

const { registerModal, unregisterModal } = useModalHistory();
const modalId = 'UpdatePrompt';

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

const handleConfirm = () => {
  emit('confirm');
  emit('update:isOpen', false);
};

const handleDismiss = () => {
  emit('dismiss');
  emit('update:isOpen', false);
};

const formatBytes = (bytes: number) => {
  if (bytes === 0) return '0 B';
  const k = 1024;
  const sizes = ['B', 'KB', 'MB', 'GB'];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + ' ' + sizes[i];
};
</script>

<template>
  <Teleport to="body">
    <Transition name="fade">
      <div
        v-if="isOpen"
        class="fixed inset-0 z-[300] flex items-start justify-center pt-[15vh] bg-black/40 backdrop-blur-sm"
        @click.self="handleDismiss"
      >
        <div
          class="bg-white dark:bg-[#1a2a30] w-11/12 max-w-sm rounded-2xl shadow-2xl border border-black/10 dark:border-white/10 p-5 transform transition-all relative overflow-hidden"
        >
          <div
            class="absolute -top-10 -right-10 w-32 h-32 bg-blue-500/10 dark:bg-blue-400/10 rounded-full blur-2xl pointer-events-none"
          ></div>

          <h3 class="text-lg font-bold text-gray-800 dark:text-gray-100 mb-1">
            发现新版本
          </h3>
          <div class="text-sm font-mono text-blue-500 mb-4">{{ version }}</div>

          <div
            v-if="releaseNotes"
            class="text-xs text-gray-600 dark:text-gray-300 opacity-80 max-h-40 overflow-y-auto whitespace-pre-wrap leading-relaxed mb-4 bg-black/5 dark:bg-white/5 rounded-xl p-3"
          >
            {{ releaseNotes }}
          </div>

          <div
            v-if="apkSize"
            class="text-[10px] text-gray-400 dark:text-gray-500 font-mono mb-4"
          >
            APK 大小: {{ formatBytes(apkSize) }}
          </div>

          <div class="flex justify-end gap-3">
            <button
              @click="handleDismiss"
              class="px-5 py-2.5 rounded-xl text-sm font-semibold text-gray-600 dark:text-gray-400 hover:bg-black/5 dark:hover:bg-white/5 transition-colors"
            >
              取消
            </button>
            <button
              @click="handleConfirm"
              class="px-5 py-2.5 rounded-xl text-sm font-semibold bg-blue-500 hover:bg-blue-600 text-white shadow-lg shadow-blue-500/30 transition-all active:scale-95"
            >
              立即更新
            </button>
          </div>
        </div>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
.fade-enter-active {
  transition: opacity 0.3s ease;
}
.fade-leave-active {
  transition: opacity 0.2s ease;
}
.fade-enter-from,
.fade-leave-to {
  opacity: 0;
}
</style>

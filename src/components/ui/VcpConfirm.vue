<script setup lang="ts">
defineOptions({
  inheritAttrs: false
});
import { watch } from 'vue';
import { useModalHistory } from '../../core/composables/useModalHistory';

const props = defineProps<{
  title: string;
  message: string;
  isOpen: boolean;
  isDanger?: boolean;
  onlyConfirm?: boolean;
}>();

const emit = defineEmits<{
  (e: 'update:isOpen', value: boolean): void;
  (e: 'confirm'): void;
  (e: 'cancel'): void;
}>();

const { registerModal, unregisterModal } = useModalHistory();
const modalId = 'VcpConfirm';

watch(() => props.isOpen, (newVal) => {
  if (newVal) {
    registerModal(modalId, () => {
      if (props.onlyConfirm) {
        emit('confirm');
      } else {
        emit('cancel');
      }
      emit('update:isOpen', false);
    });
  } else {
    unregisterModal(modalId);
  }
});

const handleConfirm = () => {
  emit('confirm');
  emit('update:isOpen', false);
};

const handleCancel = () => {
  emit('cancel');
  emit('update:isOpen', false);
};
</script>

<template>
  <Teleport to="body">
    <Transition name="fade">
      <div v-if="isOpen" v-bind="$attrs"
        class="fixed inset-0 z-dialog flex items-start justify-center pt-[15vh] bg-black/40"
        @click.self="onlyConfirm ? handleConfirm() : handleCancel()">
        <div
          class="vcp-confirm-modal bg-white dark:bg-[#1a2a30] w-11/12 max-w-sm rounded-2xl shadow-2xl border border-black/10 dark:border-white/10 p-5 transform transition-all relative overflow-hidden">

          <!-- Background Decoration -->
          <div
            class="absolute -top-10 -right-10 w-32 h-32 bg-blue-500/10 dark:bg-blue-400/10 rounded-full blur-2xl pointer-events-none">
          </div>

          <h3 class="text-lg font-bold text-gray-800 dark:text-gray-100 mb-2">{{ title }}</h3>
          <p class="text-sm text-gray-600 dark:text-gray-400 mb-6 leading-relaxed whitespace-pre-wrap text-left">{{ message }}</p>

          <div class="flex justify-end gap-3">
            <button v-if="!onlyConfirm" @click="handleCancel"
              class="px-5 py-2.5 rounded-xl text-sm font-semibold text-gray-600 dark:text-gray-400 hover:bg-black/5 dark:hover:bg-white/5 transition-colors">
              取消
            </button>
            <button @click="handleConfirm"
              class="px-5 py-2.5 rounded-xl text-sm font-semibold text-white shadow-lg transition-all active:scale-95"
              :class="isDanger ? 'bg-danger hover:opacity-90 shadow-danger/30' : 'bg-blue-500 hover:bg-blue-600 shadow-blue-500/30'">
              确认
            </button>
          </div>
        </div>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
.fade-enter-active,
.fade-leave-active {
  transition: opacity 0.3s ease;
}

.fade-enter-from,
.fade-leave-to {
  opacity: 0;
}

.fade-enter-active .vcp-confirm-modal {
  transition: all 0.3s cubic-bezier(0.34, 1.56, 0.64, 1);
}

.fade-leave-active .vcp-confirm-modal {
  transition: all 0.2s ease;
}

.fade-enter-from .vcp-confirm-modal,
.fade-leave-to .vcp-confirm-modal {
  transform: scale(0.9) translateY(10px);
  opacity: 0;
}
</style>

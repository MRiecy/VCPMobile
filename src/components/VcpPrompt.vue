<script setup lang="ts">
import { ref, onMounted, nextTick, watch } from 'vue';
import { useModalHistory } from '../composables/useModalHistory';

const props = defineProps<{
  title: string;
  initialValue?: string;
  placeholder?: string;
  isOpen: boolean;
}>();

const emit = defineEmits<{
  (e: 'update:isOpen', value: boolean): void;
  (e: 'confirm', value: string): void;
  (e: 'cancel'): void;
}>();

const { registerModal, unregisterModal } = useModalHistory();
const modalId = 'VcpPrompt';

watch(() => props.isOpen, (newVal) => {
  if (newVal) {
    registerModal(modalId, () => {
      emit('cancel');
      emit('update:isOpen', false);
    });
  } else {
    unregisterModal(modalId);
  }
});

const inputValue = ref(props.initialValue || '');
const inputRef = ref<HTMLInputElement | null>(null);

const handleConfirm = () => {
  emit('confirm', inputValue.value);
  emit('update:isOpen', false);
};

const handleCancel = () => {
  emit('cancel');
  emit('update:isOpen', false);
};

// Focus the input when the prompt opens
onMounted(() => {
  if (props.isOpen) {
    nextTick(() => {
      inputRef.value?.focus();
      inputRef.value?.select();
    });
  }
});
</script>

<template>
  <Teleport to="body">
    <Transition name="fade">
      <div v-if="isOpen" class="fixed inset-0 z-[300] flex items-start justify-center pt-[15vh] bg-black/40 backdrop-blur-sm" @click.self="handleCancel">
        <div class="vcp-prompt-modal bg-white dark:bg-[#1a2a30] w-11/12 max-w-sm rounded-2xl shadow-2xl border border-black/10 dark:border-white/10 p-5 transform transition-all relative overflow-hidden">
          
          <!-- Background Decoration -->
          <div class="absolute -top-10 -right-10 w-32 h-32 bg-blue-500/10 dark:bg-blue-400/10 rounded-full blur-2xl pointer-events-none"></div>
          
          <h3 class="text-lg font-bold text-gray-800 dark:text-gray-100 mb-4">{{ title }}</h3>
          
          <div class="relative mb-6">
            <input 
              ref="inputRef"
              v-model="inputValue" 
              type="text" 
              :placeholder="placeholder"
              class="w-full bg-black/5 dark:bg-black/20 text-gray-800 dark:text-gray-100 px-4 py-3 rounded-xl border border-black/10 dark:border-white/10 outline-none focus:border-blue-500/50 focus:ring-2 focus:ring-blue-500/20 transition-all text-sm"
              @keydown.enter="handleConfirm"
              @keydown.esc="handleCancel"
            />
          </div>
          
          <div class="flex justify-end gap-3">
            <button @click="handleCancel" class="px-5 py-2.5 rounded-xl text-sm font-semibold text-gray-600 dark:text-gray-400 hover:bg-black/5 dark:hover:bg-white/5 transition-colors">
              取消
            </button>
            <button @click="handleConfirm" class="px-5 py-2.5 rounded-xl text-sm font-semibold bg-blue-500 hover:bg-blue-600 text-white shadow-lg shadow-blue-500/30 transition-all active:scale-95">
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

.fade-enter-active .vcp-prompt-modal {
  transition: all 0.3s cubic-bezier(0.34, 1.56, 0.64, 1);
}
.fade-leave-active .vcp-prompt-modal {
  transition: all 0.2s ease;
}
.fade-enter-from .vcp-prompt-modal,
.fade-leave-to .vcp-prompt-modal {
  transform: scale(0.9) translateY(10px);
  opacity: 0;
}
</style>

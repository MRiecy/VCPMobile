<script setup lang="ts">
import { ref, watch, nextTick } from 'vue';
import { X, Check } from 'lucide-vue-next';
import { useModalHistory } from '../composables/useModalHistory';

const props = defineProps<{
  isOpen: boolean;
  initialValue: string;
}>();

const emit = defineEmits<{
  (e: 'update:isOpen', value: boolean): void;
  (e: 'save', value: string): void;
  (e: 'cancel'): void;
}>();

const { registerModal, unregisterModal } = useModalHistory();
const modalId = 'FullScreenEditor';

const editorContent = ref('');
const textareaRef = ref<HTMLTextAreaElement | null>(null);

watch(() => props.isOpen, (newVal) => {
  if (newVal) {
    // Register for back button handling
    registerModal(modalId, () => {
      emit('cancel');
      emit('update:isOpen', false);
    });

    editorContent.value = props.initialValue || '';
    nextTick(() => {
      textareaRef.value?.focus();
      textareaRef.value?.setSelectionRange(editorContent.value.length, editorContent.value.length);
    });
  } else {
    // Unregister and clean up history stack
    unregisterModal(modalId);
  }
});

const handleSave = () => {
  emit('save', editorContent.value);
  emit('update:isOpen', false);
};

const handleCancel = () => {
  emit('cancel');
  emit('update:isOpen', false);
};
</script>

<template>
  <Teleport to="body">
    <Transition name="slide-up">
      <div v-if="isOpen" 
           class="fixed inset-0 z-[2000] flex flex-col bg-[#f0f4f8] dark:bg-[#121e23] overflow-hidden">
        
        <!-- 顶部导航栏 -->
        <div class="flex items-center justify-between px-4 pt-[calc(env(safe-area-inset-top,24px)+8px)] pb-3 bg-white/80 dark:bg-gray-900/80 backdrop-blur-md border-b border-black/5 dark:border-white/5 shrink-0 shadow-sm z-10">
          <button @click="handleCancel" class="p-2 -ml-2 rounded-full active:bg-black/5 dark:active:bg-white/5 text-gray-500 hover:text-gray-800 dark:text-gray-400 dark:hover:text-gray-200 transition-colors">
            <X :size="24" />
          </button>
          
          <h2 class="text-sm font-bold text-gray-800 dark:text-gray-200 tracking-wider">编辑消息</h2>
          
          <button @click="handleSave" class="p-2 -mr-2 rounded-full active:bg-black/5 dark:active:bg-white/5 text-blue-500 hover:text-blue-600 dark:text-blue-400 dark:hover:text-blue-300 transition-colors">
            <Check :size="24" />
          </button>
        </div>
        
        <!-- 编辑器主体 -->
        <div class="flex-1 relative flex flex-col p-4 overflow-hidden">
          <textarea
            ref="textareaRef"
            v-model="editorContent"
            class="flex-1 w-full bg-transparent resize-none outline-none text-[15px] leading-relaxed text-gray-800 dark:text-gray-200 placeholder-gray-400 font-sans pb-[env(safe-area-inset-bottom)]"
            placeholder="输入消息内容..."
            spellcheck="false"
          ></textarea>
        </div>
        
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
.slide-up-enter-active,
.slide-up-leave-active {
  transition: transform 0.4s cubic-bezier(0.16, 1, 0.3, 1), opacity 0.4s ease;
}
.slide-up-enter-from,
.slide-up-leave-to {
  transform: translateY(100%);
  opacity: 0;
}
</style>

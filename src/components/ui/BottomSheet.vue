<script setup lang="ts">
import { watch } from 'vue';
import { useModalHistory } from '../../core/composables/useModalHistory';

export interface ActionItem {
  label: string;
  icon?: any; // lucide-vue-next component
  danger?: boolean;
  disabled?: boolean;
  handler: () => void;
}

const props = defineProps<{
  modelValue: boolean;
  title?: string;
  actions: ActionItem[];
}>();

const emit = defineEmits(['update:modelValue']);

const { registerModal, unregisterModal } = useModalHistory();
const modalId = 'BottomSheet';

watch(() => props.modelValue, (newVal) => {
  if (newVal) {
    registerModal(modalId, () => {
      emit('update:modelValue', false);
    });
  } else {
    unregisterModal(modalId);
  }
});

const close = () => {
  emit('update:modelValue', false);
};

const handleAction = (action: ActionItem) => {
  if (action.disabled) return;
  action.handler();
  close();
};
</script>

<template>
  <Teleport to="body">
    <!-- 遮罩层 -->
    <Transition name="fade">
      <div v-if="modelValue" 
           class="fixed inset-0 bg-black/50 z-[999] backdrop-blur-[2px]" 
           @click="close"
           @touchmove.prevent>
      </div>
    </Transition>
    
    <!-- 抽屉内容 -->
    <Transition name="slide-up">
      <div v-if="modelValue" 
           class="fixed bottom-0 left-0 right-0 z-[1000] bg-white/90 dark:bg-gray-900/90 backdrop-blur-xl rounded-t-[2rem] shadow-2xl p-4 flex flex-col border-t border-white/20 dark:border-white/5"
           style="padding-bottom: calc(env(safe-area-inset-bottom, 20px) + 16px);">
        
        <!-- 顶部拉手条 -->
        <div class="w-12 h-1.5 bg-black/10 dark:bg-white/20 rounded-full mx-auto mb-4"></div>
        
        <!-- 标题 -->
        <div v-if="title" class="text-[10px] font-bold text-center text-gray-400 uppercase tracking-[0.2em] mb-4">
          {{ title }}
        </div>
        
        <!-- 操作项列表 -->
        <div class="flex flex-col gap-2 px-1">
          <button v-for="(action, index) in actions" :key="index" 
            @click="handleAction(action)"
            :disabled="action.disabled"
            class="flex items-center justify-start px-6 py-4 rounded-2xl active:scale-[0.98] transition-all text-[16px] font-black border"
            :class="[
              action.danger 
                ? 'bg-red-50 dark:bg-red-500/10 text-red-600 dark:text-red-400 border-red-100 dark:border-red-500/20 shadow-sm shadow-red-500/10' 
                : 'bg-black/5 dark:bg-white/5 text-gray-800 dark:text-gray-200 border-transparent hover:bg-black/10 dark:hover:bg-white/10 shadow-sm',
              action.disabled ? 'opacity-40 cursor-not-allowed' : ''
            ]">
            <component v-if="action.icon" :is="action.icon" :size="20" class="mr-4 opacity-90" :class="action.danger ? 'text-red-500' : 'text-blue-500/80 dark:text-blue-400/80'" />
            <span class="tracking-wide">{{ action.label }}</span>
          </button>
          
          <!-- 取消按钮 -->
          <button @click="close" 
                  class="mt-3 py-4 rounded-2xl text-[16px] font-bold bg-black/5 dark:bg-white/5 text-gray-500 dark:text-gray-400 active:scale-[0.98] transition-all border border-transparent flex items-center justify-center">
            取消
          </button>
        </div>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
.fade-enter-active,
.fade-leave-active {
  transition: opacity 0.3s cubic-bezier(0.16, 1, 0.3, 1);
}
.fade-enter-from,
.fade-leave-to {
  opacity: 0;
}

.slide-up-enter-active,
.slide-up-leave-active {
  transition: transform 0.4s cubic-bezier(0.16, 1, 0.3, 1);
}
.slide-up-enter-from,
.slide-up-leave-to {
  transform: translateY(100%);
}
</style>
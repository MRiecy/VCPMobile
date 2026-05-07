<script setup lang="ts">
defineOptions({
  inheritAttrs: false
});
import { ref, watch, nextTick } from 'vue';
import { X, Check } from 'lucide-vue-next';
import { useKeyboardInsets } from '../../core/composables/useKeyboardInsets';

const props = defineProps<{
  isOpen: boolean;
  initialValue: string;
}>();

const emit = defineEmits<{
  (e: 'update:isOpen', value: boolean): void;
  (e: 'save', value: string): void;
  (e: 'cancel'): void;
}>();

const editorContent = ref(props.initialValue || '');
const textareaRef = ref<HTMLTextAreaElement | null>(null);
const { keyboardHeight } = useKeyboardInsets();

watch(() => props.isOpen, (newVal) => {
  if (newVal) {
    editorContent.value = props.initialValue || '';
    nextTick(() => {
      textareaRef.value?.focus();
      textareaRef.value?.setSelectionRange(editorContent.value.length, editorContent.value.length);
    });
  }
});

// 键盘弹出后，确保光标/底部内容仍在可视区域内
watch(keyboardHeight, () => {
  nextTick(() => {
    const textarea = textareaRef.value;
    if (!textarea || !props.isOpen) return;
    const pos = textarea.selectionStart;
    textarea.setSelectionRange(pos, pos);
  });
});

const handleSave = () => {
  emit('save', editorContent.value);
  emit('update:isOpen', false);
};

const handleCancel = () => {
  emit('cancel');
  emit('update:isOpen', false);
};

// --- 阻止 textarea 边界滑动导致页面被拖动（rubber-band） ---
const lastTouchY = ref(0);

const handleTouchStart = (e: TouchEvent) => {
  if (e.touches.length > 0) {
    lastTouchY.value = e.touches[0].clientY;
  }
};

const handleTouchMove = (e: TouchEvent) => {
  if (!textareaRef.value || e.touches.length === 0) return;
  const el = textareaRef.value;
  const scrollTop = el.scrollTop;
  const scrollHeight = el.scrollHeight;
  const clientHeight = el.clientHeight;
  const currentY = e.touches[0].clientY;
  const deltaY = lastTouchY.value - currentY;

  // 内容未溢出时，直接阻止页面被拖动
  if (scrollHeight <= clientHeight) {
    e.preventDefault();
    return;
  }

  // 在顶部且继续向下滑动（试图拉出上层/适应层）
  if (scrollTop <= 0 && deltaY < 0) {
    e.preventDefault();
    return;
  }

  // 在底部且继续向上滑动（试图推出页面）
  if (scrollTop + clientHeight >= scrollHeight - 1 && deltaY > 0) {
    e.preventDefault();
    return;
  }

  lastTouchY.value = currentY;
};
</script>

<template>
  <Teleport to="body">
    <Transition name="slide-up">
      <div v-if="isOpen" v-bind="$attrs"
        class="fixed inset-0 z-[2000] flex flex-col bg-[#f0f4f8] dark:bg-[#121e23] overflow-hidden"
        :style="{ paddingBottom: `calc(env(safe-area-inset-bottom) + ${keyboardHeight}px)` }">

        <!-- 顶部导航栏 -->
        <div
          class="flex items-center justify-between px-4 pt-[calc(env(safe-area-inset-top,24px)+8px)] pb-3 bg-white/80 dark:bg-gray-900/80 border-b border-black/5 dark:border-white/5 shrink-0 shadow-sm z-10">
          <button @click="handleCancel"
            class="p-2 -ml-2 rounded-full active:bg-black/5 dark:active:bg-white/5 text-gray-500 hover:text-gray-800 dark:text-gray-400 dark:hover:text-gray-200 transition-colors">
            <X :size="24" />
          </button>

          <h2 class="text-sm font-bold text-gray-800 dark:text-gray-200 tracking-wider">编辑消息</h2>

          <button @click="handleSave"
            class="p-2 -mr-2 rounded-full active:bg-black/5 dark:active:bg-white/5 text-blue-500 hover:text-blue-600 dark:text-blue-400 dark:hover:text-blue-300 transition-colors">
            <Check :size="24" />
          </button>
        </div>

        <!-- 编辑器主体 -->
        <div class="flex-1 relative flex flex-col p-4 overflow-hidden">
          <textarea ref="textareaRef" v-model="editorContent" @touchstart="handleTouchStart" @touchmove="handleTouchMove"
            class="vcp-fullscreen-textarea flex-1 w-full bg-transparent resize-none outline-none text-[15px] leading-relaxed text-gray-800 dark:text-gray-200 placeholder-gray-400 font-sans"
            placeholder="输入消息内容..." spellcheck="false"></textarea>
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

/* 优化 Android WebView 中 textarea 的点击与 focus 行为 */
.vcp-fullscreen-textarea {
  touch-action: manipulation;
  -webkit-tap-highlight-color: transparent;
  cursor: text;
  overscroll-behavior: contain;
}
</style>

<script setup lang="ts">
import { ref, watch, nextTick, computed } from 'vue';
import { useChatManagerStore } from '../../core/stores/chatManager';
import { useLongTextPaste } from './composables/useLongTextPaste';
import StagedAttachmentPreview from './StagedAttachmentPreview.vue';
import GroupStopAllButton from './components/GroupStopAllButton.vue';

const props = defineProps<{
  disabled?: boolean;
}>();

const emit = defineEmits<{
  (e: 'send', content: string): void;
  (e: 'attach'): void;
}>();

const input = ref('');
const chatStore = useChatManagerStore();
const textareaRef = ref<HTMLTextAreaElement | null>(null);

// 自动调整输入框高度
const autoResize = () => {
  if (textareaRef.value) {
    textareaRef.value.style.height = 'auto'; // Reset height
    const scrollHeight = textareaRef.value.scrollHeight;
    textareaRef.value.style.height = `${scrollHeight}px`;
  }
};

watch(input, () => {
  nextTick(() => {
    autoResize();
  });
});

// 是否正在生成中 (修正：回归纯净逻辑，只要当前话题有活跃的网络流，即视为生成中)
const isGenerating = computed(() => chatStore.activeStreamingIds.size > 0);

// 监听并接收外部注入的“编辑消息”内容
watch(() => chatStore.editMessageContent, async (newContent) => {
  if (newContent) {
    input.value = newContent;
    chatStore.editMessageContent = ''; // 消费掉
    // 强制更新高度和焦点
    await nextTick();
    if (textareaRef.value) {
      textareaRef.value.focus();
      // 触发 autoResize 逻辑(如果存在)
      textareaRef.value.dispatchEvent(new Event('input', { bubbles: true }));
    }
  }
});

const handleSend = () => {
  // 允许纯附件消息发送（即 input 为空但有暂存附件）
  if ((input.value.trim() || chatStore.stagedAttachments.length > 0) && !props.disabled) {
    emit('send', input.value);
    input.value = '';
  }
};

const handleAction = () => {
  if (isGenerating.value) {
    // 停止当前所有活跃的生成流 (单聊只有一个，群聊则为当前并行或串行的 Agent)
    const activeIds = Array.from(chatStore.activeStreamingIds);
    activeIds.forEach(id => chatStore.stopMessage(id as string));
  } else {
    handleSend();
  }
};

const handleKeydown = (e: KeyboardEvent) => {
  if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
    e.preventDefault();
    handleAction();
  }
};

const triggerFilePick = async () => {
  if (props.disabled) return;
  emit('attach');
  await chatStore.handleAttachment();
};

const { handlePaste, handleBeforeInput } = useLongTextPaste(input);

const removeStagedAttachment = (index: number) => {
  chatStore.stagedAttachments.splice(index, 1);
};

// --- 阻止 textarea 边界滑动导致页面被拖动 ---
const lastTouchY = ref(0);

const handleTextareaTouchStart = (e: TouchEvent) => {
  if (e.touches.length > 0) {
    lastTouchY.value = e.touches[0].clientY;
  }
};

const handleTextareaTouchMove = (e: TouchEvent) => {
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
  <div class="px-3 py-1 w-full transition-opacity duration-300 no-swipe relative" :class="{ 'opacity-70 pointer-events-none': disabled }">
    <!-- 全局群组停止按钮 (悬浮在输入框上方) -->
    <GroupStopAllButton />

    <!-- 暂存附件预览区 -->
    <div v-if="chatStore.stagedAttachments.length > 0" class="flex items-center gap-2 mb-2 px-2 overflow-x-auto pb-1 pt-2">
      <TransitionGroup name="list">
        <StagedAttachmentPreview 
          v-for="(file, idx) in chatStore.stagedAttachments" 
          :key="file.id || idx" 
          :file="file" 
          :index="idx"
          @remove="removeStagedAttachment"
        />
      </TransitionGroup>
    </div>

    <div class="flex items-end gap-2 px-1">
      <div
        class="flex-1 flex items-end gap-2 bg-[var(--secondary-bg)] border border-[var(--border-color)] rounded-[1.75rem] px-2 py-1 shadow-sm backdrop-blur-md relative overflow-hidden">
        
        <!-- 附件按钮 (归位到 Pill 内部) -->
        <button @click="triggerFilePick"
          class="w-9 h-9 mb-0.5 flex items-center justify-center shrink-0 rounded-full hover:bg-black/5 dark:hover:bg-white/5 text-[var(--primary-text)] opacity-60 hover:opacity-100 active:scale-90 transition-all"
          :disabled="disabled">
          <svg width="19" height="19" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"
            stroke-linecap="round" stroke-linejoin="round">
            <path
              d="M21.44 11.05l-9.19 9.19a6 6 0 0 1-8.49-8.49l9.19-9.19a4 4 0 0 1 5.66 5.66l-9.2 9.19a2 2 0 0 1-2.83-2.83l8.49-8.48">
            </path>
          </svg>
        </button>

        <!-- 核心输入区 -->
        <div class="flex-1 flex flex-col justify-end relative min-h-[36px] py-[1px]">
          <textarea ref="textareaRef" v-model="input" @keydown="handleKeydown" @paste="handlePaste" @beforeinput="handleBeforeInput" @touchstart="handleTextareaTouchStart" @touchmove="handleTextareaTouchMove" rows="1"
            class="w-full bg-transparent border-none focus:outline-none focus:ring-0 text-[var(--primary-text)] text-[15px] placeholder-opacity-40 resize-none leading-[1.25] py-[8px] scrollbar-hide"
            style="max-height: 114px;"
            :placeholder="disabled ? '请先选择话题以开启对话' : '说点什么...'" :disabled="disabled"></textarea>
          
          <div class="absolute top-0 left-0 right-0 h-4 pointer-events-none bg-gradient-to-b from-[var(--secondary-bg)] to-transparent opacity-90"></div>
        </div>

        <!-- 发送/中止按钮 -->
        <button @click="handleAction"
          class="w-9 h-9 mb-0.5 flex items-center justify-center shrink-0 rounded-full shadow-md active:scale-90 transition-all"
          :class="[
            isGenerating ? 'bg-red-500 hover:bg-red-600 text-white' : 'bg-blue-500 text-white',
            {
              'opacity-30 scale-90': !isGenerating && !input.trim() && chatStore.stagedAttachments.length === 0,
              'hover:bg-blue-600': !isGenerating && (input.trim() || chatStore.stagedAttachments.length > 0)
            }
          ]" :disabled="!isGenerating && !input.trim() && chatStore.stagedAttachments.length === 0">
          <!-- 停止图标 -->
          <svg v-if="isGenerating" width="16" height="16" viewBox="0 0 24 24" fill="currentColor" stroke="none">
            <rect x="6" y="6" width="12" height="12" rx="1.5"></rect>
          </svg>
          <!-- 发送图标 -->
          <svg v-else width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
            stroke-linecap="round" stroke-linejoin="round" class="-ml-0.5">
            <line x1="22" y1="2" x2="11" y2="13"></line>
            <polygon points="22 2 15 22 11 13 2 9 22 2"></polygon>
          </svg>
        </button>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* 隐藏滚动条 */
.overflow-x-auto {
  scrollbar-width: none;
  -ms-overflow-style: none;
}

.overflow-x-auto::-webkit-scrollbar {
  display: none;
}

.scrollbar-hide::-webkit-scrollbar {
  display: none;
}
.scrollbar-hide {
  -ms-overflow-style: none;
  scrollbar-width: none;
}
</style>

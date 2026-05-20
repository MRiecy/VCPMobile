<script setup lang="ts">
import { ref, watch, nextTick, computed } from 'vue';
import { useChatHistoryStore } from '../../core/stores/chatHistoryStore';
import { useChatStreamStore } from '../../core/stores/chatStreamStore';
import { useAttachmentStore } from '../../core/stores/attachmentStore';
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
const showAttachMenu = ref(false);
const historyStore = useChatHistoryStore();
const streamStore = useChatStreamStore();
const attachmentStore = useAttachmentStore();
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

// 是否正在生成中
const isGenerating = computed(() => streamStore.activeStreamingIds.size > 0);

// 是否有内容可发送
const hasContent = computed(() => input.value.trim() !== '' || attachmentStore.stagedAttachments.length > 0);

// 监听并接收外部注入的“编辑消息”内容
watch(() => historyStore.editMessageContent, async (newContent) => {
  if (newContent) {
    input.value = newContent;
    historyStore.editMessageContent = ''; // 消费掉
    // 强制更新高度和焦点
    await nextTick();
    if (textareaRef.value) {
      textareaRef.value.focus();
      // 触发 autoResize 逻辑
      textareaRef.value.dispatchEvent(new Event('input', { bubbles: true }));
    }
  }
});

const handleSend = () => {
  if (hasContent.value && !props.disabled) {
    emit('send', input.value);
    input.value = '';
    showAttachMenu.value = false;
  }
};

const handleAction = () => {
  if (isGenerating.value) {
    const activeIds = Array.from(streamStore.activeStreamingIds);
    activeIds.forEach(id => streamStore.stopMessage(id as string));
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

const triggerFilePick = async (mode: 'camera' | 'gallery' | 'file') => {
  if (props.disabled) return;
  showAttachMenu.value = false;
  emit('attach');
  await attachmentStore.handleAttachment(mode);
};

const { handlePaste, handleBeforeInput } = useLongTextPaste(input);

const removeStagedAttachment = (index: number) => {
  attachmentStore.stagedAttachments.splice(index, 1);
};

// 注意：textarea 上的 touch 拦截已移除，交由 WebView 原生处理 focus 与滚动。
// 若后续仍需防止 rubber-band，应在容器层面（ChatView）统一处理，而非在 input 上。
</script>

<template>
  <div class="px-1 py-1 w-full transition-opacity duration-300 no-swipe relative" :class="{ 'opacity-70 pointer-events-none': disabled }">
    <!-- 全局群组停止按钮 -->
    <GroupStopAllButton />

    <!-- 附件选择菜单 (浮层) -->
    <Transition name="fade-scale">
      <div v-if="showAttachMenu" 
        class="absolute bottom-16 right-4 bg-[var(--secondary-bg)] border border-[var(--border-color)] rounded-2xl shadow-2xl p-2 z-local flex flex-col gap-1 min-w-[120px] backdrop-blur-md"
      >
        <button @click="triggerFilePick('camera')" class="flex items-center gap-3 px-3 py-2.5 rounded-xl hover:bg-black/5 dark:hover:bg-white/5 active:scale-95 transition-all">
          <div class="i-heroicons-camera text-lg text-blue-500"></div>
          <span class="text-sm font-medium text-[var(--primary-text)]">拍摄</span>
        </button>
        <button @click="triggerFilePick('gallery')" class="flex items-center gap-3 px-3 py-2.5 rounded-xl hover:bg-black/5 dark:hover:bg-white/5 active:scale-95 transition-all">
          <div class="i-heroicons-photo text-lg text-purple-500"></div>
          <span class="text-sm font-medium text-[var(--primary-text)]">相册</span>
        </button>
        <button @click="triggerFilePick('file')" class="flex items-center gap-3 px-3 py-2.5 rounded-xl hover:bg-black/5 dark:hover:bg-white/5 active:scale-95 transition-all">
          <div class="i-heroicons-document-text text-lg text-orange-500"></div>
          <span class="text-sm font-medium text-[var(--primary-text)]">文件</span>
        </button>
      </div>
    </Transition>

    <!-- 暂存附件预览区 -->
    <div v-if="attachmentStore.stagedAttachments.length > 0" class="flex items-center gap-2 mb-2 px-2 overflow-x-auto pb-1 pt-2">
      <TransitionGroup name="list">
        <StagedAttachmentPreview 
          v-for="(file, idx) in attachmentStore.stagedAttachments" 
          :key="file.id || idx" 
          :file="file" 
          :index="idx"
          @remove="removeStagedAttachment"
        />
      </TransitionGroup>
    </div>

    <div class="flex items-end gap-2 px-1">
      <div class="flex-1 flex items-end gap-1.5 bg-[var(--secondary-bg)] border border-[var(--border-color)] rounded-2xl px-2 py-1 shadow-sm relative overflow-visible">
        
        <!-- 左侧：语音按钮 (使用用户提供的精准 SVG) -->
        <button class="w-9 h-9 mb-0.5 flex items-center justify-center shrink-0 rounded-full hover:bg-black/5 dark:hover:bg-white/5 text-[var(--primary-text)] opacity-90 active:scale-90 transition-all">
          <svg width="26" height="26" viewBox="0 0 48 48" fill="none" xmlns="http://www.w3.org/2000/svg" class="shrink-0">
            <!-- 外圆环 -->
            <circle cx="24" cy="24" r="19.5" stroke="currentColor" stroke-width="3.5" fill="none"/>
            <!-- 声源点 -->
            <circle cx="17.5" cy="24" r="3" fill="currentColor"/>
            <!-- 内侧声波 -->
            <path d="M 21.5 18 A 6.5 6.5 0 0 1 21.5 30" stroke="currentColor" stroke-width="3" stroke-linecap="round" fill="none"/>
            <!-- 外侧声波 -->
            <path d="M 26 13.5 A 12 12 0 0 1 26 34.5" stroke="currentColor" stroke-width="3" stroke-linecap="round" fill="none"/>
          </svg>
        </button>

        <!-- 核心输入区 -->
        <div class="flex-1 flex flex-col justify-end relative min-h-[36px] py-[1px]">
          <textarea ref="textareaRef" v-model="input" @keydown="handleKeydown" @paste="handlePaste" @beforeinput="handleBeforeInput" rows="1"
            class="w-full bg-transparent border-none focus:outline-none focus:ring-0 text-[var(--primary-text)] text-[15px] placeholder-opacity-40 resize-none leading-[1.25] py-[8px] scrollbar-hide vcp-textarea"
            style="max-height: 114px;"
            :placeholder="disabled ? '请先选择话题以开启对话' : '说点什么...'" :disabled="disabled"></textarea>
          
          <div class="absolute top-0 left-0 right-0 h-4 pointer-events-none bg-gradient-to-b from-[var(--secondary-bg)] to-transparent opacity-90"></div>
        </div>

        <!-- 右侧动态操作区 -->
        <div class="flex items-center shrink-0 mb-0.5 relative gap-1.5">
          <!-- 展开附件按钮 (带旋转动画) -->
          <button
            @click="showAttachMenu = !showAttachMenu"
            class="w-9 h-9 flex items-center justify-center rounded-full hover:bg-black/5 dark:hover:bg-white/5 text-[var(--primary-text)] opacity-80 hover:opacity-100 active:scale-90 transition-all"
          >
            <div class="i-heroicons-plus-circle text-2xl transition-transform duration-300 ease-out" :class="{ 'rotate-45': showAttachMenu }"></div>
          </button>

          <Transition name="pop-slide">
            <!-- 发送/中止按钮 (浅蓝色背景，w-8h-8，精准居中) -->
            <button v-if="hasContent || isGenerating"
              @click="handleAction"
              class="w-8 h-8 flex items-center justify-center rounded-full shadow-sm active:scale-95 transition-all bg-blue-500 text-white"
              :class="{ 'bg-red-500': isGenerating }"
            >
              <div v-if="isGenerating" class="i-heroicons-stop-16-solid text-lg"></div>
              <div v-else class="i-heroicons-paper-airplane text-[15px] -rotate-45 translate-x-0.2 -translate-y-0.2"></div>
            </button>
          </Transition>
        </div>
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

/* 优化 Android WebView 中 textarea 的点击与 focus 行为 */
.vcp-textarea {
  touch-action: manipulation;
  -webkit-tap-highlight-color: transparent;
  cursor: text;
}

/* 气泡弹出/切换动画 (优化宽度塌陷，确保 + 按钮平滑跟随) */
.pop-slide-enter-active,
.pop-slide-leave-active {
  transition: all 0.3s cubic-bezier(0.34, 1.56, 0.64, 1);
  overflow: hidden;
  white-space: nowrap;
}

.pop-slide-enter-from {
  opacity: 0;
  transform: scale(0.4) translateX(20px);
  width: 0;
}

.pop-slide-leave-to {
  opacity: 0;
  transform: scale(0.4) translateX(10px);
  width: 0;
  margin-left: -6px; /* 抵消 gap-1.5 (6px)，让 + 按钮平滑吸附 */
}

/* 附件菜单淡入淡出缩放 */
.fade-scale-enter-active,
.fade-scale-leave-active {
  transition: all 0.2s ease-out;
}
.fade-scale-enter-from,
.fade-scale-leave-to {
  opacity: 0;
  transform: scale(0.9) translateY(10px);
}
</style>

<script setup lang="ts">
import { ref, watch, nextTick, computed } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { useChatHistoryStore } from '../../core/stores/chatHistoryStore';
import { useChatStreamStore } from '../../core/stores/chatStreamStore';
import { useAttachmentStore } from '../../core/stores/attachmentStore';
import { useNotificationStore } from '../../core/stores/notification';
import { useLongTextPaste } from './composables/useLongTextPaste';
import { useSpeechRecognition } from '../../core/composables/useSpeechRecognition';
import { useAudioRecorder } from '../../core/composables/useAudioRecorder';
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
const notificationStore = useNotificationStore();
const textareaRef = ref<HTMLTextAreaElement | null>(null);

// ----------------------------------------------------
// 语音模块：实时识别 (STT) 与 录音附件 Hook 联动
// ----------------------------------------------------
const {
  transcriptionResult,
  startListening,
  stopListening,
  cancelListening
} = useSpeechRecognition();

const {
  recordingDuration,
  startRecording,
  stopRecording,
  cancelRecording
} = useAudioRecorder();

const isVoiceInputMode = ref(false);       // 是否处于点击语音转文字模式
const isLongPressRecording = ref(false);   // 是否处于长按录音附件模式
const isSwipeCancel = ref(false);          // 手势是否判定为上滑取消
let touchStartY = 0;
let touchStartTime = 0;
let isLongPress = false;
let longPressTimeout: number | null = null;

// 点击按钮切换流式语音输入
const handleVoiceClick = async () => {
  if (isVoiceInputMode.value) {
    // 结束转文字
    const recognizedText = await stopListening();
    isVoiceInputMode.value = false;
    if (recognizedText && !recognizedText.startsWith('[')) {
      input.value += recognizedText;
    }
    await nextTick();
    if (textareaRef.value) {
      textareaRef.value.focus();
      autoResize();
    }
  } else {
    // 开启流式语音识别
    isVoiceInputMode.value = true;
    try {
      await startListening((_text) => {
        // 流式回调可实时更新 transcriptionResult，已由 Hook 接管
      });
      if (navigator.vibrate) navigator.vibrate(40);
    } catch (err: any) {
      isVoiceInputMode.value = false;
      notificationStore.addNotification({
        type: 'warning',
        title: '语音识别启动失败',
        message: err.message || String(err),
        toastOnly: true,
      });
    }
  }
};

// 放弃当前倾听
const discardVoiceInput = () => {
  cancelListening();
  isVoiceInputMode.value = false;
};

// Touch 手势管理：实现完美的长按与点击分流交互
const handleVoiceTouchStart = (e: TouchEvent) => {
  if (props.disabled) return;
  e.preventDefault(); // 彻底阻止 WebView 弹出长按菜单或缩放抖动

  touchStartTime = Date.now();
  isLongPress = false;
  isSwipeCancel.value = false;
  touchStartY = e.touches[0].clientY;

  // 如果按住超过 350ms，自动唤醒“长按录音附件”模式
  longPressTimeout = window.setTimeout(async () => {
    isLongPress = true;
    if (isVoiceInputMode.value) {
      await handleVoiceClick(); // 强行清算当前的流式输入
    }
    isLongPressRecording.value = true;
    try {
      await startRecording();
      if (navigator.vibrate) navigator.vibrate(50);
    } catch (err: any) {
      isLongPressRecording.value = false;
      isLongPress = false;
    }
  }, 350);
};

const handleVoiceTouchMove = (e: TouchEvent) => {
  if (!isLongPress || !isLongPressRecording.value) return;
  const currentY = e.touches[0].clientY;
  const diffY = currentY - touchStartY;

  // 手指上滑超过 60 像素判定为上滑取消
  if (diffY < -60) {
    if (!isSwipeCancel.value) {
      isSwipeCancel.value = true;
      if (navigator.vibrate) navigator.vibrate(30); // 触发微弱警告震动
    }
  } else {
    isSwipeCancel.value = false;
  }
};

const handleVoiceTouchEnd = async (e: TouchEvent) => {
  e.preventDefault();
  
  if (longPressTimeout) {
    clearTimeout(longPressTimeout);
    longPressTimeout = null;
  }

  const duration = Date.now() - touchStartTime;

  if (!isLongPress && duration < 350) {
    // 判定为 Tap：开启/关闭流式语音输入
    await handleVoiceClick();
  } else if (isLongPressRecording.value) {
    isLongPressRecording.value = false;
    
    if (isSwipeCancel.value) {
      cancelRecording();
      notificationStore.addNotification({
        type: 'warning',
        title: '已取消录音',
        message: '上滑取消操作已完成',
        toastOnly: true,
      });
    } else {
      const result = await stopRecording();
      if (result) {
        try {
          const originalName = `Voice_${Date.now()}.webm`;
          // 零拷贝直传：保存至后端沙盒，生成正式的 Attachment
          const finalData = await invoke<any>('store_file', {
            originalName,
            fileBytes: result.bytes,
            mimeType: result.blob.type || 'audio/webm',
          });

          if (finalData) {
            attachmentStore.stagedAttachments.unshift({
              id: `att_${Date.now()}_${Math.random().toString(36).slice(2, 7)}`,
              type: finalData.type,
              src: finalData.internalPath,
              name: finalData.name,
              size: finalData.size,
              hash: finalData.hash,
              status: 'done',
            });

            if (navigator.vibrate) navigator.vibrate([40, 40]);
            notificationStore.addNotification({
              type: 'success',
              title: '语音录制成功',
              message: '已添加为音频附件',
              toastOnly: true,
            });
          }
        } catch (err: any) {
          console.error('[InputEnhancer] Store voice file failed:', err);
          notificationStore.addNotification({
            type: 'warning',
            title: '录音保存异常',
            message: String(err),
            toastOnly: true,
          });
        }
      }
    }
  }
  isLongPress = false;
  isSwipeCancel.value = false;
};

const handleVoiceTouchCancel = (e: TouchEvent) => {
  e.preventDefault();
  if (longPressTimeout) {
    clearTimeout(longPressTimeout);
    longPressTimeout = null;
  }
  if (isLongPressRecording.value) {
    cancelRecording();
    isLongPressRecording.value = false;
    isSwipeCancel.value = false;
  }
  isLongPress = false;
};

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
    await nextTick();
    if (textareaRef.value) {
      textareaRef.value.focus();
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
  attachmentStore.removeStaged(index);
};
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
      <div class="flex-1 flex items-end gap-1.5 bg-[var(--secondary-bg)] border border-[var(--border-color)] rounded-2xl px-2 py-1 shadow-sm relative overflow-visible"
        :class="{ 'ring-1 ring-blue-500/30 border-blue-500/50': isVoiceInputMode || isLongPressRecording, 'ring-1 ring-red-500/30 border-red-500/50': isSwipeCancel }"
      >
        
        <!-- 左侧：语音按钮 (Touch 手势激活，点击流式转文字，长按录音作为音频附件) -->
        <button 
          @touchstart.prevent="handleVoiceTouchStart"
          @touchmove="handleVoiceTouchMove"
          @touchend="handleVoiceTouchEnd"
          @touchcancel="handleVoiceTouchCancel"
          class="w-9 h-9 mb-0.5 flex items-center justify-center shrink-0 rounded-full hover:bg-black/5 dark:hover:bg-white/5 text-[var(--primary-text)] opacity-90 active:scale-90 transition-all relative select-none touch-none"
          :class="{ 
            'bg-blue-500/10 text-blue-500': isVoiceInputMode || isLongPressRecording,
            'bg-red-500/10 text-red-500': isSwipeCancel
          }"
        >
          <svg width="26" height="26" viewBox="0 0 48 48" fill="none" xmlns="http://www.w3.org/2000/svg" class="shrink-0" :class="{ 'animate-pulse text-blue-500': isVoiceInputMode || isLongPressRecording }">
            <circle cx="24" cy="24" r="19.5" stroke="currentColor" stroke-width="3.5" fill="none"/>
            <circle cx="17.5" cy="24" r="3" fill="currentColor"/>
            <path d="M 21.5 18 A 6.5 6.5 0 0 1 21.5 30" stroke="currentColor" stroke-width="3" stroke-linecap="round" fill="none"/>
            <path d="M 26 13.5 A 12 12 0 0 1 26 34.5" stroke="currentColor" stroke-width="3" stroke-linecap="round" fill="none"/>
          </svg>
        </button>

        <!-- 核心输入与极简占位融合区 (Balthasar 美学：框内平滑收缩) -->
        <div class="flex-1 flex flex-col justify-end relative min-h-[36px] py-[1px] overflow-hidden">
          <!-- 1. 普通文本输入状态 -->
          <textarea 
            v-if="!isVoiceInputMode && !isLongPressRecording"
            ref="textareaRef" 
            v-model="input" 
            @keydown="handleKeydown" 
            @paste="handlePaste" 
            @beforeinput="handleBeforeInput" 
            rows="1"
            class="w-full bg-transparent border-none focus:outline-none focus:ring-0 text-[var(--primary-text)] text-[15px] placeholder-opacity-40 resize-none leading-[1.25] py-[8px] scrollbar-hide vcp-textarea"
            style="max-height: 114px;"
            :placeholder="disabled ? '请先选择话题以开启对话' : '说点什么...'" 
            :disabled="disabled"
          ></textarea>
          
          <!-- 2. 点击模式：流式语音转文字占位 -->
          <div v-else-if="isVoiceInputMode" class="w-full flex flex-col justify-center min-h-[36px] py-1 px-1.5 select-none animate-fade-in">
            <div class="flex items-center justify-between gap-2 mb-1">
              <div class="flex items-center gap-1.5 text-xs font-semibold text-blue-500">
                <span class="w-2.5 h-2.5 rounded-full bg-blue-500 animate-ping"></span>
                <span>正在倾听... (点击麦克风保存)</span>
              </div>
              <button @click="discardVoiceInput" class="text-[10px] text-[var(--primary-text)] opacity-40 hover:opacity-100 flex items-center gap-1 active:scale-95 transition-all px-1.5 py-0.5 rounded-md hover:bg-black/5 dark:hover:bg-white/5">
                <span>取消</span>
              </button>
            </div>
            <div class="text-[14px] text-[var(--primary-text)] min-h-[1.25rem] leading-[1.25] break-all opacity-80 italic font-medium">
              {{ transcriptionResult || '请开始说话...' }}
            </div>
          </div>

          <!-- 3. 长按模式：录音附件暂存占位 -->
          <div v-else-if="isLongPressRecording" class="w-full flex flex-col justify-center min-h-[36px] py-1 px-1.5 select-none animate-fade-in">
            <div class="flex items-center gap-1.5 text-xs font-semibold" :class="isSwipeCancel ? 'text-red-500' : 'text-blue-500'">
              <span class="w-2.5 h-2.5 rounded-full" :class="isSwipeCancel ? 'bg-red-500 animate-pulse' : 'bg-blue-500 animate-ping'"></span>
              <span>{{ isSwipeCancel ? '松手取消发送' : '按住录制音频附件... (上滑取消)' }}</span>
            </div>
            <div class="text-[14px] text-[var(--primary-text)] font-mono opacity-80 mt-0.5">
              已录制: {{ recordingDuration }} 秒
            </div>
          </div>
          
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

.animate-fade-in {
  animation: fadeIn 0.25s cubic-bezier(0.16, 1, 0.3, 1);
}

@keyframes fadeIn {
  from { opacity: 0; transform: translateY(4px); }
  to { opacity: 1; transform: translateY(0); }
}
</style>

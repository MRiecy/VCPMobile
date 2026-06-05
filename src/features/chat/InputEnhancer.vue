<script setup lang="ts">
import { ref, watch, nextTick, computed, onMounted, onUnmounted } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { useChatHistoryStore } from '../../core/stores/chatHistoryStore';
import { useChatSessionStore } from '../../core/stores/chatSessionStore';
import { useChatStreamStore } from '../../core/stores/chatStreamStore';
import { useAttachmentStore } from '../../core/stores/attachmentStore';
import { useNotificationStore } from '../../core/stores/notification';
import { useTarvenStore } from '../../core/stores/tarvenStore';
import { useLongTextPaste } from './composables/useLongTextPaste';
import { useSpeechRecognition } from '../../core/composables/useSpeechRecognition';
import { useAudioRecorder } from '../../core/composables/useAudioRecorder';
import StagedAttachmentPreview from './StagedAttachmentPreview.vue';
import GroupStopAllButton from './components/GroupStopAllButton.vue';

const tarvenStore = useTarvenStore();

const openTarvenSelector = () => {
  if (props.disabled) return;
  if (navigator.vibrate) navigator.vibrate(50);
  tarvenStore.isSelectorOpen = true;
};

const props = defineProps<{
  disabled?: boolean;
}>();

const emit = defineEmits<{
  (e: 'send', content: string): void;
  (e: 'attach'): void;
  (e: 'toggle-menu', visible: boolean): void;
  (e: 'focus-input'): void;
}>();

const input = ref('');
const showAttachMenu = ref(false);

watch(showAttachMenu, (val) => {
  emit('toggle-menu', val);
});
const historyStore = useChatHistoryStore();
const sessionStore = useChatSessionStore();
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

// 状态控制器
const isAudioMode = ref(false);            // 是否切换为经典的“按住 说话”模式
const isSTTActive = ref(false);             // 语音转文字（点击切换后的按住模式）是否激活
const isLongPressRecording = ref(false);   // 直接长按语音按钮录制音频附件是否激活
const isSwipeCancel = ref(false);          // 手势是否判定为上滑取消

let touchStartY = 0;
let iconTouchStartTime = 0;
let isIconLongPress = false;
let iconLongPressTimeout: number | null = null;

// 切换语音模式与普通文本模式
const toggleAudioMode = () => {
  if (props.disabled) return;
  
  showAttachMenu.value = false;
  
  // 如果当前正处于某种语音输入状态中，先彻底释放
  if (isSTTActive.value) {
    cancelListening();
    isSTTActive.value = false;
  }
  if (isLongPressRecording.value) {
    cancelRecording();
    isLongPressRecording.value = false;
  }
  
  isAudioMode.value = !isAudioMode.value;
  isSwipeCancel.value = false;
  
  if (navigator.vibrate) navigator.vibrate(35);
  
  if (!isAudioMode.value) {
    nextTick(() => {
      if (textareaRef.value) {
        textareaRef.value.focus();
        autoResize();
      }
    });
  }
};

// ----------------------------------------------------
// 交互 A：在“按住 说话”大长条上的 STT 手势管理
// ----------------------------------------------------
const handleSTTTouchStart = async (e: TouchEvent) => {
  if (props.disabled) return;
  if (e.cancelable) e.preventDefault(); // 阻断默认上下文菜单与滚动

  isSTTActive.value = true;
  isSwipeCancel.value = false;
  touchStartY = e.touches[0].clientY;

  try {
    await startListening((_text) => {
      // 实时流式回调更新 transcriptionResult
    });
    if (navigator.vibrate) navigator.vibrate(50);
  } catch (err: any) {
    isSTTActive.value = false;
    notificationStore.addNotification({
      type: 'warning',
      title: '语音转写启动失败',
      message: err.message || String(err),
      toastOnly: true,
    });
  }
};

const handleSTTTouchMove = (e: TouchEvent) => {
  if (!isSTTActive.value) return;
  const currentY = e.touches[0].clientY;
  const diffY = currentY - touchStartY;

  if (diffY < -60) {
    if (!isSwipeCancel.value) {
      isSwipeCancel.value = true;
      if (navigator.vibrate) navigator.vibrate(30);
    }
  } else {
    isSwipeCancel.value = false;
  }
};

const handleSTTTouchEnd = async (e: TouchEvent) => {
  if (e.cancelable) e.preventDefault();
  if (!isSTTActive.value) return;

  isSTTActive.value = false;

  if (isSwipeCancel.value) {
    cancelListening();
    notificationStore.addNotification({
      type: 'warning',
      title: '已取消转写',
      message: '上滑取消操作已完成',
      toastOnly: true,
    });
  } else {
    const recognizedText = await stopListening();
    if (recognizedText && !recognizedText.startsWith('[')) {
      input.value += recognizedText;
      
      // 自动切回键盘并聚焦，方便用户在键盘上微调！
      isAudioMode.value = false;
      await nextTick();
      if (textareaRef.value) {
        textareaRef.value.focus();
        autoResize();
      }
      
      if (navigator.vibrate) navigator.vibrate([40, 40]);
    }
  }
  isSwipeCancel.value = false;
};

const handleSTTTouchCancel = (e: TouchEvent) => {
  if (e.cancelable) e.preventDefault();
  if (isSTTActive.value) {
    cancelListening();
    isSTTActive.value = false;
    isSwipeCancel.value = false;
  }
};

// ----------------------------------------------------
// 交互 B：直接长按左侧语音按钮，进行附件录制并松手直发
// ----------------------------------------------------
const handleIconTouchStart = (e: TouchEvent) => {
  if (props.disabled) return;
  
  // 核心修复：如果当前已经处于“按住说话”模式，点击此按钮是为了切回键盘，直接触发 Tap 切换！
  // 彻底解决了由于 touchstart.prevent 导致 click 无法触发的“点不回去”手势缺陷！
  if (isAudioMode.value) {
    if (e.cancelable) e.preventDefault();
    toggleAudioMode();
    return;
  }
  
  if (e.cancelable) e.preventDefault(); // 阻止 WebView 的震动和菜单弹起

  iconTouchStartTime = Date.now();
  isIconLongPress = false;
  isSwipeCancel.value = false;
  touchStartY = e.touches[0].clientY;

  iconLongPressTimeout = window.setTimeout(async () => {
    isIconLongPress = true;
    isLongPressRecording.value = true;
    try {
      await startRecording();
      if (navigator.vibrate) navigator.vibrate(50);
    } catch (err: any) {
      isLongPressRecording.value = false;
      isIconLongPress = false;
    }
  }, 350);
};

const handleIconTouchMove = (e: TouchEvent) => {
  if (!isIconLongPress || !isLongPressRecording.value) return;
  const currentY = e.touches[0].clientY;
  const diffY = currentY - touchStartY;

  if (diffY < -60) {
    if (!isSwipeCancel.value) {
      isSwipeCancel.value = true;
      if (navigator.vibrate) navigator.vibrate(30);
    }
  } else {
    isSwipeCancel.value = false;
  }
};

const handleIconTouchEnd = async (e: TouchEvent) => {
  if (e.cancelable) e.preventDefault();
  
  if (iconLongPressTimeout) {
    clearTimeout(iconLongPressTimeout);
    iconLongPressTimeout = null;
  }

  const duration = Date.now() - iconTouchStartTime;

  if (!isIconLongPress && duration < 350) {
    // 判定为 Tap：直接切换为语音模式
    toggleAudioMode();
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
          // 保存音频至后端物理沙盒
          const finalData = await invoke<any>('store_file', {
            originalName,
            fileBytes: result.bytes,
            mimeType: result.blob.type || 'audio/webm',
          });

          if (finalData) {
            // 塞入 stagedAttachments
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
            
            // 松手直接以附件形式发送
            await nextTick();
            handleSend();
          }
        } catch (err: any) {
          console.error('[InputEnhancer] Direct send voice failed:', err);
          notificationStore.addNotification({
            type: 'warning',
            title: '语音发送异常',
            message: String(err),
            toastOnly: true,
          });
        }
      }
    }
  }
  isIconLongPress = false;
  isSwipeCancel.value = false;
};

const handleIconTouchCancel = (e: TouchEvent) => {
  if (e.cancelable) e.preventDefault();
  if (iconLongPressTimeout) {
    clearTimeout(iconLongPressTimeout);
    iconLongPressTimeout = null;
  }
  if (isLongPressRecording.value) {
    cancelRecording();
    isLongPressRecording.value = false;
    isSwipeCancel.value = false;
  }
  isIconLongPress = false;
};

const handleFocus = () => {
  showAttachMenu.value = false;
  if (!props.disabled && historyStore.currentChatHistory.length > 0) {
    emit('focus-input');
  }
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

// 监听并接收外部注入的”编辑消息”内容
watch(() => historyStore.editMessageContent, async (newContent) => {
  if (newContent) {
    input.value = newContent;
    historyStore.editMessageContent = ''; // 消费掉
    isAudioMode.value = false; // 强行切回键盘输入态
    await nextTick();
    if (textareaRef.value) {
      textareaRef.value.focus();
      textareaRef.value.dispatchEvent(new Event('input', { bubbles: true }));
    }
  }
});

// 监听外部分享意图预填文本
watch(() => sessionStore.sharePrefillText, async (newText) => {
  if (newText) {
    input.value = newText;
    sessionStore.sharePrefillText = "";
    isAudioMode.value = false;
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
  emit('attach');
  await attachmentStore.handleAttachment(mode);
};

const { handlePaste, handleBeforeInput } = useLongTextPaste(input);

const removeStagedAttachment = (index: number) => {
  attachmentStore.removeStaged(index);
};

const rootRef = ref<HTMLElement | null>(null);

const handleClickOutside = (event: MouseEvent) => {
  if (showAttachMenu.value && rootRef.value && !rootRef.value.contains(event.target as Node)) {
    showAttachMenu.value = false;
  }
};

onMounted(() => {
  document.addEventListener('click', handleClickOutside, true);
});

onUnmounted(() => {
  document.removeEventListener('click', handleClickOutside, true);
});
</script>

<template>
  <div ref="rootRef" class="px-1 py-1 w-full transition-opacity duration-300 no-swipe relative flex flex-col gap-1.5" :class="{ 'opacity-70 pointer-events-none': disabled }">
    <!-- 全局群组停止按钮 -->
    <GroupStopAllButton />

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

    <!-- 框体主容器 -->
    <div class="flex items-end gap-2 px-1">
      <div class="flex-1 flex items-end gap-1.5 bg-[var(--secondary-bg)] border border-[var(--border-color)] rounded-2xl px-2 py-1 shadow-sm relative overflow-visible transition-all duration-300"
        :class="{ 
          'ring-1 ring-blue-500/30 border-blue-500/50': isSTTActive || isLongPressRecording, 
          'ring-1 ring-red-500/30 border-red-500/50': isSwipeCancel 
        }"
      >
        
        <!-- 左侧：语音/键盘切换及长按附件录制按钮 -->
        <button 
          @touchstart.prevent="handleIconTouchStart"
          @touchmove="handleIconTouchMove"
          @touchend="handleIconTouchEnd"
          @touchcancel="handleIconTouchCancel"
          class="w-9 h-9 mb-0.5 flex items-center justify-center shrink-0 rounded-full hover:bg-black/5 dark:hover:bg-white/5 text-[var(--primary-text)] opacity-90 active:scale-90 transition-all relative select-none touch-none"
          :class="{ 
            'bg-blue-500/10 text-blue-500': isAudioMode || isLongPressRecording,
            'bg-red-500/10 text-red-500': isSwipeCancel && isLongPressRecording
          }"
        >
          <!-- 键盘图标 (纯 inline SVG，100% 精美、零依赖，规避 UnoCSS 找不到图标的问题) -->
          <svg v-if="isAudioMode" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" class="w-6 h-6 shrink-0 animate-fade-in">
            <rect x="2" y="4" width="20" height="16" rx="2" ry="2"></rect>
            <line x1="6" y1="8" x2="6" y2="8"></line>
            <line x1="10" y1="8" x2="10" y2="8"></line>
            <line x1="14" y1="8" x2="14" y2="8"></line>
            <line x1="18" y1="8" x2="18" y2="8"></line>
            <line x1="6" y1="12" x2="6" y2="12"></line>
            <line x1="10" y1="12" x2="10" y2="12"></line>
            <line x1="14" y1="12" x2="14" y2="12"></line>
            <line x1="18" y1="12" x2="18" y2="12"></line>
            <line x1="7" y1="16" x2="17" y2="16"></line>
          </svg>
          
          <!-- 麦克风图标 -->
          <svg v-else width="26" height="26" viewBox="0 0 48 48" fill="none" xmlns="http://www.w3.org/2000/svg" class="shrink-0" :class="{ 'animate-pulse text-blue-500': isLongPressRecording }">
            <circle cx="24" cy="24" r="19.5" stroke="currentColor" stroke-width="3.5" fill="none"/>
            <circle cx="17.5" cy="24" r="3" fill="currentColor"/>
            <path d="M 21.5 18 A 6.5 6.5 0 0 1 21.5 30" stroke="currentColor" stroke-width="3" stroke-linecap="round" fill="none"/>
            <path d="M 26 13.5 A 12 12 0 0 1 26 34.5" stroke="currentColor" stroke-width="3" stroke-linecap="round" fill="none"/>
          </svg>
        </button>

        <!-- 核心输入/按住说话极简交互区 (极致自然：仅原 textarea 区域变化，右侧按钮完全正常静止显示) -->
        <div class="flex-1 flex flex-col justify-end relative min-h-[36px] py-[1px] overflow-visible">
          
          <!-- 情况 1：普通文本输入键盘状态 -->
          <textarea 
            v-if="!isAudioMode && !isLongPressRecording"
            ref="textareaRef" 
            v-model="input" 
            @focus="handleFocus"
            @keydown="handleKeydown" 
            @paste="handlePaste" 
            @beforeinput="handleBeforeInput" 
            rows="1"
            class="w-full bg-transparent border-none focus:outline-none focus:ring-0 text-[var(--primary-text)] text-[15px] placeholder-opacity-40 resize-none leading-[1.25] py-[8px] scrollbar-hide vcp-textarea"
            style="max-height: 114px;"
            :placeholder="disabled ? '请先选择话题以开启对话' : '说点什么...'" 
            :disabled="disabled"
          ></textarea>
          
          <!-- 情况 2：语音模式 - “按住 说话” 大条状态 (仅静默填入原本 textarea 的位置，右侧加号/发送正常显示) -->
          <div 
            v-else-if="isAudioMode && !isSTTActive" 
            @touchstart.prevent="handleSTTTouchStart"
            @touchmove="handleSTTTouchMove"
            @touchend="handleSTTTouchEnd"
            @touchcancel="handleSTTTouchCancel"
            class="w-full h-[36px] flex items-center justify-center rounded-xl bg-black/5 dark:bg-white/5 border border-black/10 dark:border-white/10 active:bg-black/15 dark:active:bg-white/15 select-none touch-none cursor-pointer transform active:scale-[0.98] transition-all duration-75"
          >
            <span class="text-[13px] font-semibold text-[var(--primary-text)] opacity-85 tracking-wider">按住 说话</span>
          </div>

          <!-- 情况 3：正在按压“按住说话”大条进行实时流式转文字中 (展示倾听与临时出字) -->
          <div v-else-if="isSTTActive" class="w-full flex flex-col justify-center min-h-[36px] py-1 px-1 select-none animate-fade-in">
            <div class="flex items-center gap-1.5 text-xs font-semibold text-blue-500" :class="{ 'text-red-500': isSwipeCancel }">
              <span class="w-2.5 h-2.5 rounded-full" :class="isSwipeCancel ? 'bg-red-500 animate-pulse' : 'bg-blue-500 animate-ping'"></span>
              <span>{{ isSwipeCancel ? '松手取消转写' : '正在识别流式文字... (上滑取消)' }}</span>
            </div>
            <div class="text-[14px] text-[var(--primary-text)] min-h-[1.25rem] leading-[1.25] break-all opacity-85 italic font-medium mt-0.5">
              {{ transcriptionResult || '请开始说话...' }}
            </div>
          </div>

          <!-- 情况 4：在非语音模式下，直接长按语音图标录制音频附件进行中 (经典的“倾听中”说明，松手直发) -->
          <div v-else-if="isLongPressRecording" class="w-full flex flex-col justify-center min-h-[36px] py-1 px-1 select-none animate-fade-in">
            <div class="flex items-center gap-1.5 text-xs font-semibold text-blue-500" :class="{ 'text-red-500': isSwipeCancel }">
              <span class="w-2.5 h-2.5 rounded-full" :class="isSwipeCancel ? 'bg-red-500 animate-pulse' : 'bg-blue-500 animate-ping'"></span>
              <span>{{ isSwipeCancel ? '松手取消发送' : '倾听中... (上滑取消)' }}</span>
            </div>
            <div class="text-[14px] text-[var(--primary-text)] font-mono opacity-85 mt-0.5">
              录音时长: {{ recordingDuration }} 秒
            </div>
          </div>
          
          <div class="absolute top-0 left-0 right-0 h-4 pointer-events-none bg-gradient-to-b from-[var(--secondary-bg)] to-transparent opacity-90"></div>
        </div>

        <!-- 右侧动态操作区 (始终正常、平静地保留，保证视觉绝无抖动！) -->
        <div class="flex items-center shrink-0 mb-0.5 relative gap-1.5">
          <!-- 展开附件按钮 -->
          <button
            v-longpress="openTarvenSelector"
            @click="showAttachMenu = !showAttachMenu"
            class="w-9 h-9 flex items-center justify-center rounded-full hover:bg-black/5 dark:hover:bg-white/5 text-[var(--primary-text)] opacity-80 hover:opacity-100 active:scale-90 transition-all relative"
          >
            <div class="i-heroicons-plus-circle text-2xl transition-transform duration-300 ease-out" :class="{ 'rotate-45': showAttachMenu }"></div>
            <!-- 当有激活规则时显示绿色指示点 -->
            <div v-if="tarvenStore.rules.some(r => r.isEnabled)" 
              class="absolute top-1.5 right-1.5 w-2 h-2 bg-emerald-500 rounded-full border-2 border-[var(--secondary-bg)] shadow-[0_0_8px_rgba(16,185,129,0.5)]">
            </div>
          </button>

          <Transition name="pop-slide">
            <!-- 发送/中止按钮 -->
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

    <!-- 往上平滑弹起的百搭扩展面板 -->
    <div 
      class="overflow-hidden transition-all duration-300 ease-[cubic-bezier(0.34,1.56,0.64,1)]"
      :class="showAttachMenu ? 'opacity-100' : 'opacity-0 pointer-events-none'"
      :style="{ height: showAttachMenu ? '112px' : '0px' }"
    >
      <div 
        class="w-full h-full border-t border-[var(--border-color)]/20 pt-3 pb-2 flex justify-around items-center transition-all duration-300"
        :class="showAttachMenu ? 'translate-y-0 opacity-100 scale-100' : 'translate-y-4 opacity-0 scale-95'"
      >
        <!-- 拍摄按钮 -->
        <button @click="triggerFilePick('camera')" 
          class="group flex flex-col items-center gap-1.5 active:scale-95 transition-all outline-none"
        >
          <div class="w-13 h-13 flex items-center justify-center rounded-2xl bg-black/5 dark:bg-white/5 border border-[var(--border-color)]/30 group-hover:border-[var(--highlight-text)]/40 transition-all shadow-inner">
            <div class="i-heroicons-camera text-2xl text-blue-500/80 group-hover:text-blue-500 group-hover:scale-105 transition-all"></div>
          </div>
          <span class="text-[11px] font-semibold text-[var(--primary-text)]/70 group-hover:text-[var(--primary-text)] transition-colors">拍摄</span>
        </button>

        <!-- 相册按钮 -->
        <button @click="triggerFilePick('gallery')" 
          class="group flex flex-col items-center gap-1.5 active:scale-95 transition-all outline-none"
        >
          <div class="w-13 h-13 flex items-center justify-center rounded-2xl bg-black/5 dark:bg-white/5 border border-[var(--border-color)]/30 group-hover:border-[var(--highlight-text)]/40 transition-all shadow-inner">
            <div class="i-heroicons-photo text-2xl text-purple-500/80 group-hover:text-purple-500 group-hover:scale-105 transition-all"></div>
          </div>
          <span class="text-[11px] font-semibold text-[var(--primary-text)]/70 group-hover:text-[var(--primary-text)] transition-colors">相册</span>
        </button>

        <!-- 文件按钮 -->
        <button @click="triggerFilePick('file')" 
          class="group flex flex-col items-center gap-1.5 active:scale-95 transition-all outline-none"
        >
          <div class="w-13 h-13 flex items-center justify-center rounded-2xl bg-black/5 dark:bg-white/5 border border-[var(--border-color)]/30 group-hover:border-[var(--highlight-text)]/40 transition-all shadow-inner">
            <div class="i-heroicons-document-text text-2xl text-orange-500/80 group-hover:text-orange-500 group-hover:scale-105 transition-all"></div>
          </div>
          <span class="text-[11px] font-semibold text-[var(--primary-text)]/70 group-hover:text-[var(--primary-text)] transition-colors">文件</span>
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

/* 优化 Android WebView 中 textarea 的点击与 focus 行为 */
.vcp-textarea {
  touch-action: manipulation;
  -webkit-tap-highlight-color: transparent;
  cursor: text;
}

/* 气泡弹出/切换动画 */
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
  margin-left: -6px;
}

.animate-fade-in {
  animation: fadeIn 0.25s cubic-bezier(0.16, 1, 0.3, 1);
}

@keyframes fadeIn {
  from { opacity: 0; transform: translateY(4px); }
  to { opacity: 1; transform: translateY(0); }
}
</style>

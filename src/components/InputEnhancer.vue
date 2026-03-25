<script setup lang="ts">
import { ref, watch, nextTick, computed } from 'vue';
import { useChatManagerStore } from '../stores/chatManager';

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

// 是否正在生成中
const isGenerating = computed(() => !!chatStore.streamingMessageId);

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
    chatStore.stopGenerating();
  } else {
    handleSend();
  }
};

const handleKeydown = (e: KeyboardEvent) => {
  if (e.key === 'Enter' && !e.shiftKey) {
    e.preventDefault();
    handleAction();
  }
};

const triggerFilePick = async () => {
  if (props.disabled) return;
  emit('attach');
  await chatStore.handleAttachment();
};

const removeStagedAttachment = (index: number) => {
  chatStore.stagedAttachments.splice(index, 1);
};
</script>

<template>
  <div class="px-3 py-1 w-full transition-opacity duration-300"
       :class="{ 'opacity-70 pointer-events-none': disabled }">
    
    <!-- 暂存附件预览区 -->
    <div v-if="chatStore.stagedAttachments.length > 0" class="flex items-center gap-2 mb-1.5 px-2 overflow-x-auto pb-1">
      <div v-for="(file, idx) in chatStore.stagedAttachments" :key="idx" 
           class="relative group shrink-0 flex items-center gap-2 bg-black/5 dark:bg-black/20 border border-black/5 dark:border-white/10 rounded-xl px-3 py-2 max-w-[200px]">
        
        <!-- 文件类型图标 -->
        <div class="w-8 h-8 rounded-lg bg-black/5 dark:bg-white/5 flex items-center justify-center shrink-0">
          <svg v-if="file.type.startsWith('image/')" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="text-blue-400"><rect x="3" y="3" width="18" height="18" rx="2" ry="2"></rect><circle cx="8.5" cy="8.5" r="1.5"></circle><polyline points="21 15 16 10 5 21"></polyline></svg>
          <svg v-else width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="text-gray-400"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"></path><polyline points="14 2 14 8 20 8"></polyline><line x1="16" y1="13" x2="8" y2="13"></line><line x1="16" y1="17" x2="8" y2="17"></line><polyline points="10 9 9 9 8 9"></polyline></svg>
        </div>
        
        <div class="flex flex-col overflow-hidden">
          <span class="text-xs text-white truncate font-medium">{{ file.name }}</span>
          <span class="text-[9px] opacity-40 uppercase">{{ (file.size / 1024).toFixed(1) }} KB</span>
        </div>
        
        <!-- 删除按钮 -->
        <button @click.stop="removeStagedAttachment(idx)" class="absolute -top-1 -right-1 w-5 h-5 bg-red-500 rounded-full flex items-center justify-center text-white opacity-0 group-hover:opacity-100 transition-opacity shadow-md">
          <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><line x1="18" y1="6" x2="6" y2="18"></line><line x1="6" y1="6" x2="18" y2="18"></line></svg>
        </button>
      </div>
    </div>

    <div class="flex items-center gap-2 bg-[var(--secondary-bg)] border border-[var(--border-color)] rounded-[1.75rem] px-2 py-1 shadow-sm backdrop-blur-md">
      
      <!-- 附件按钮 -->
      <button 
        @click="triggerFilePick"
        class="w-9 h-9 flex items-center justify-center shrink-0 rounded-full text-[var(--primary-text)] opacity-50 hover:opacity-100 hover:bg-white/5 active:scale-95 transition-all"
        :disabled="disabled"
      >
        <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <path d="M21.44 11.05l-9.19 9.19a6 6 0 0 1-8.49-8.49l9.19-9.19a4 4 0 0 1 5.66 5.66l-9.2 9.19a2 2 0 0 1-2.83-2.83l8.49-8.48"></path>
        </svg>
      </button>

      <!-- 核心输入区 -->
      <div class="flex-1 flex items-center min-h-[36px]">
        <textarea
          ref="textareaRef"
          v-model="input"
          @keydown="handleKeydown"
          rows="1"
          class="w-full bg-transparent border-none focus:outline-none focus:ring-0 text-[var(--primary-text)] text-[15px] placeholder-opacity-40 max-h-28 resize-none leading-tight py-1.5"
          :placeholder="disabled ? '请先选择话题以开启对话' : '说点什么...'"
          :disabled="disabled"
        ></textarea>
      </div>

      <!-- 发送/中止按钮 -->
      <button
        @click="handleAction"
        class="w-9 h-9 flex items-center justify-center shrink-0 rounded-full shadow-md active:scale-90 transition-all ml-1"
        :class="[
          isGenerating ? 'bg-red-500 hover:bg-red-600 text-white' : 'bg-blue-500 text-white',
          { 
            'opacity-30 scale-90': !isGenerating && !input.trim() && chatStore.stagedAttachments.length === 0 && !disabled, 
            'hover:bg-blue-600': !isGenerating && (input.trim() || chatStore.stagedAttachments.length > 0) && !disabled 
          }
        ]"
        :disabled="!isGenerating && ((!input.trim() && chatStore.stagedAttachments.length === 0) || disabled)"
      >
        <!-- 停止图标 -->
        <svg v-if="isGenerating" width="16" height="16" viewBox="0 0 24 24" fill="currentColor" stroke="none">
          <rect x="6" y="6" width="12" height="12" rx="1.5"></rect>
        </svg>
        <!-- 发送图标 -->
        <svg v-else width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="-ml-0.5">
          <line x1="22" y1="2" x2="11" y2="13"></line>
          <polygon points="22 2 15 22 11 13 2 9 22 2"></polygon>
        </svg>
      </button>
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
</style>

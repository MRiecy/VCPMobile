<script setup lang="ts">
import { ref, onMounted, nextTick, watch } from "vue";
import { useFloatingAssistantStore } from "../../core/stores/floatingAssistant";
import AssistantMessageCard from "./AssistantMessageCard.vue";

const floatingStore = useFloatingAssistantStore();
const inputText = ref("");
const messageContainer = ref<HTMLDivElement | null>(null);
const textareaRef = ref<HTMLTextAreaElement | null>(null);

const closeAssistant = () => {
  if (floatingStore.isFloatingMode) {
    if ((window as any).AndroidBridge) {
      (window as any).AndroidBridge.closeWindow();
    }
  }
};

const handleSend = () => {
  if (!inputText.value.trim() || floatingStore.isGenerating) return;

  const text = inputText.value;
  inputText.value = "";

  floatingStore.sendMessage(text).then(() => {
    scrollToBottom();
    focusTextarea();
  });
  scrollToBottom();
};

// 流式输出时自动滚动：监听最后一条消息内容变化
watch(
  () => {
    const msgs = floatingStore.messages;
    if (msgs.length === 0) return "";
    const last = msgs[msgs.length - 1];
    return last.content || "";
  },
  () => {
    requestAnimationFrame(() => scrollToBottom());
  },
);

const scrollToBottom = () => {
  nextTick(() => {
    if (messageContainer.value) {
      messageContainer.value.scrollTop = messageContainer.value.scrollHeight;
    }
  });
};

const focusTextarea = () => {
  nextTick(() => {
    if (textareaRef.value) {
      textareaRef.value.focus();
    }
  });
};

const tryFillClipboard = () => {
  if ((window as any).AndroidBridge) {
    const text = (window as any).AndroidBridge.getClipboard();
    if (text && text.trim()) {
      inputText.value = text.trim();
    }
  }
};

onMounted(() => {
  if (floatingStore.isFloatingMode) {
    floatingStore.initWebSocket();
    tryFillClipboard();
  }
  focusTextarea();
});
</script>

<template>
  <div
    class="h-full w-full flex flex-col bg-white dark:bg-zinc-900 shadow-2xl overflow-hidden rounded-t-[16px] border-t border-black/5 dark:border-white/10"
  >
    <!-- Header -->
    <div
      class="flex items-center justify-between px-4 py-3 border-b border-black/5 dark:border-white/5 shrink-0 bg-white dark:bg-zinc-900"
    >
      <div class="flex items-center gap-2">
        <div
          class="w-1.5 h-1.5 rounded-full"
          :class="
            floatingStore.wsConfigured || !floatingStore.isFloatingMode
              ? 'bg-green-500'
              : floatingStore.wsReady
                ? 'bg-yellow-500'
                : 'bg-red-500'
          "
        ></div>
        <span
          class="text-[11px] font-bold tracking-widest text-primary-text/60 uppercase"
          >VCP 划词助手</span
        >
      </div>
      <button
        @click="closeAssistant"
        class="text-primary-text/40 hover:text-primary-text/80 p-1.5 active:scale-95 transition-all"
      >
        <svg
          width="18"
          height="18"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2.5"
          stroke-linecap="round"
          stroke-linejoin="round"
        >
          <line x1="18" y1="6" x2="6" y2="18"></line>
          <line x1="6" y1="6" x2="18" y2="18"></line>
        </svg>
      </button>
    </div>

    <!-- Messages -->
    <div
      ref="messageContainer"
      class="flex-1 overflow-y-auto px-4 py-4 no-rubber-band"
    >
      <!-- Empty state -->
      <div
        v-if="floatingStore.messages.length === 0"
        class="h-full flex flex-col items-center justify-center py-10 opacity-20 select-none"
      >
        <svg
          width="32"
          height="32"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="1.5"
          class="mb-2"
        >
          <path
            d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"
          ></path>
        </svg>
        <span class="text-[10px] font-semibold"
          >输入开始对话，点击右上角收起</span
        >
      </div>

      <!-- Message list with AssistantMessageCard -->
      <AssistantMessageCard
        v-for="msg in floatingStore.messages"
        :key="msg.id"
        :message="msg"
      />
    </div>

    <!-- Toast notifications -->
    <TransitionGroup
      name="toast"
      tag="div"
      class="absolute top-12 left-0 right-0 flex flex-col items-center gap-1 pointer-events-none z-50"
    >
      <div
        v-for="toast in floatingStore.toasts"
        :key="toast.id"
        class="px-3 py-1.5 rounded-full text-[11px] font-medium shadow-lg pointer-events-auto"
        :class="
          toast.type === 'success'
            ? 'bg-green-500 text-white'
            : toast.type === 'error'
              ? 'bg-red-500 text-white'
              : 'bg-black/80 dark:bg-white/80 text-white dark:text-black'
        "
      >
        {{ toast.title }}
      </div>
    </TransitionGroup>

    <!-- Input -->
    <div
      class="p-3 border-t border-black/5 dark:border-white/5 shrink-0 bg-white/50 dark:bg-zinc-900/50 pb-6"
    >
      <div
        class="flex items-end gap-2 bg-black/5 dark:bg-white/5 border border-black/10 dark:border-white/10 rounded-xl p-2 focus-within:border-blue-500 transition-colors"
      >
        <textarea
          ref="textareaRef"
          v-model="inputText"
          rows="1"
          :placeholder="floatingStore.isFloatingMode && !floatingStore.wsConfigured ? '正在连接助手服务...' : '问问 VCP...'"
          class="flex-1 max-h-32 bg-transparent border-none outline-none resize-none text-[13px] py-1 px-1 text-primary-text placeholder-primary-text/30"
          @keydown.enter.prevent="handleSend"
        ></textarea>
        <button
          @click="handleSend"
          :disabled="!inputText.trim() || floatingStore.isGenerating || (floatingStore.isFloatingMode && !floatingStore.wsConfigured)"
          class="p-2 bg-blue-500 disabled:bg-black/10 dark:disabled:bg-white/10 text-white rounded-lg disabled:text-primary-text/20 active:scale-95 transition-all shrink-0"
        >
          <svg
            v-if="!floatingStore.isGenerating"
            width="16"
            height="16"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2.5"
            stroke-linecap="round"
            stroke-linejoin="round"
          >
            <line x1="22" y1="2" x2="11" y2="13"></line>
            <polygon points="22 2 15 22 11 13 2 9 22 2"></polygon>
          </svg>
          <svg
            v-else
            class="animate-spin"
            width="16"
            height="16"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2.5"
          >
            <circle
              cx="12"
              cy="12"
              r="10"
              stroke-dasharray="32"
              stroke-dashoffset="16"
              fill="none"
            ></circle>
          </svg>
        </button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.animate-spin {
  animation: spin 1s linear infinite;
}
@keyframes spin {
  from {
    transform: rotate(0deg);
  }
  to {
    transform: rotate(360deg);
  }
}

.toast-enter-active {
  transition: all 0.3s ease;
}
.toast-leave-active {
  transition: all 0.2s ease;
}
.toast-enter-from {
  opacity: 0;
  transform: translateY(-8px);
}
.toast-leave-to {
  opacity: 0;
  transform: translateY(-4px);
}
</style>

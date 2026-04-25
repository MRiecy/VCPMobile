<script setup lang="ts">
import { ref, onMounted, onUnmounted, watch, nextTick } from "vue";
import { useChatManagerStore } from "../../core/stores/chatManager";
import { useThemeStore } from "../../core/stores/theme";
import { useAppLifecycleStore } from "../../core/stores/appLifecycle";
import { useLayoutStore } from "../../core/stores/layout";
import MessageRenderer from "./MessageRenderer.vue";
import InputEnhancer from "./InputEnhancer.vue";
import VcpAvatar from "../../components/ui/VcpAvatar.vue";
import CoreStatusIndicator from "../../components/ui/CoreStatusIndicator.vue";
import { ArrowDown } from "lucide-vue-next";

const chatStore = useChatManagerStore();
const themeStore = useThemeStore();
const lifecycleStore = useAppLifecycleStore();
const layoutStore = useLayoutStore();

// 自动滚动到底部
const messageListRef = ref<HTMLElement | null>(null);
const showScrollToBottom = ref(false);
const isInitialTopicLoad = ref(true);
let mutationObserver: MutationObserver | null = null;

const scrollToBottom = (smooth = false) => {
  if (messageListRef.value) {
    messageListRef.value.scrollTo({
      top: messageListRef.value.scrollHeight,
      behavior: smooth ? "smooth" : "auto",
    });
  }
};

const handleScroll = () => {
  if (!messageListRef.value) return;
  const { scrollTop, scrollHeight, clientHeight } = messageListRef.value;
  // 如果距离底部超过 150px，显示置底按钮
  showScrollToBottom.value = scrollHeight - scrollTop - clientHeight > 150;
};

// 监听话题切换，重置加载状态
watch(
  () => chatStore.currentTopicId,
  () => {
    isInitialTopicLoad.value = true;
    showScrollToBottom.value = false;
  }
);

// 监听新消息，如果已经在底部附近，则自动平滑滚动
watch(
  () => chatStore.currentChatHistory.length,
  async () => {
    if (!isInitialTopicLoad.value && !showScrollToBottom.value) {
      await nextTick();
      scrollToBottom(true);
    }
  },
);

const handleVcpButtonClick = (e: any) => {
  if (e.detail && e.detail.text) {
    chatStore.sendMessage(e.detail.text);
  }
};

// --- OOM Defense: Viewport Handler Reference ---
const handleViewportResize = () => {
  if (chatStore.currentChatHistory.length > 0) {
    scrollToBottom(true);
  }
};

onMounted(async () => {
  // 监听来自内联 HTML 按钮的点击事件
  window.addEventListener("vcp-button-click", handleVcpButtonClick);

  // 监听视口变化（键盘弹起）- 保持引用以便销毁
  if (window.visualViewport) {
    window.visualViewport.addEventListener("resize", handleViewportResize);
  }

  // 使用 MutationObserver 精准监听真实 DOM 节点的挂载
  if (messageListRef.value) {
    mutationObserver = new MutationObserver(() => {
      // 只要发生子节点变动，且处于初始加载期
      if (isInitialTopicLoad.value && chatStore.currentChatHistory.length > 0) {
        scrollToBottom(false);
        // 稍微延迟解除状态，包容短时间内的多批次子节点渲染
        setTimeout(() => {
           isInitialTopicLoad.value = false;
        }, 50);
      }
    });
    
    mutationObserver.observe(messageListRef.value, { 
      childList: true, // 监听直接子节点（聊天气泡）的增删
      subtree: true,   // 监听子树，包容内部长文本或图片的延迟渲染撑开高度
      characterData: true // 监听文本变动（针对流式加载）
    });
  }
});

onUnmounted(() => {
  window.removeEventListener("vcp-button-click", handleVcpButtonClick);

  if (window.visualViewport) {
    window.visualViewport.removeEventListener("resize", handleViewportResize);
  }
  
  if (mutationObserver) {
    mutationObserver.disconnect();
  }
});
</script>

<template>
  <div class="chat-view-container flex flex-col h-full w-full min-w-0 relative bg-transparent overflow-hidden">
    <!-- 1. Header (强制保底高度 80px，确保刘海屏可见) -->
    <header class="vcp-header-fixed shrink-0 flex items-center justify-between gap-3 px-4 border-b border-white/5">
      <div class="flex items-center gap-3 min-w-0 flex-1">
        <!-- 侧边栏按钮 (使用内联 SVG 确保 100% 可见) -->
        <button @click="layoutStore.toggleLeftDrawer()"
          class="w-10 h-10 flex items-center justify-center rounded-xl bg-black/5 dark:bg-white/10 active:scale-90 transition-all border border-black/5 dark:border-white/5">
          <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"
            stroke-linecap="round">
            <line x1="3" y1="12" x2="21" y2="12"></line>
            <line x1="3" y1="6" x2="21" y2="6"></line>
            <line x1="3" y1="18" x2="21" y2="18"></line>
          </svg>
        </button>

        <!-- 头像展示 -->
        <VcpAvatar 
          v-if="chatStore.currentSelectedItem"
          :owner-type="chatStore.currentSelectedItem.type" 
          :owner-id="chatStore.currentSelectedItem.id" 
          :fallback-name="chatStore.currentSelectedItem.name"
          :dominant-color="chatStore.currentSelectedItem.avatarCalculatedColor"
          :outer-border="true"
          size="w-10 h-10"
          rounded="rounded-full"
        />

        <div class="flex flex-col min-w-0">
          <span 
            class="font-bold text-sm truncate transition-colors duration-500"
            :style="{ color: chatStore.currentSelectedItem?.avatarCalculatedColor || 'var(--primary-text)' }"
          >
            {{ chatStore.currentSelectedItem?.name || "VCP Mobile" }}
          </span>
  <div class="flex items-center gap-1" :title="lifecycleStore.errorMsg || undefined">
    <CoreStatusIndicator />
  </div>
</div>
</div>

<div class="flex items-center gap-2 shrink-0">
        <!-- 黑白模式切换 (内联 SVG) -->
        <button @click="themeStore.toggleTheme()"
          class="w-10 h-10 flex items-center justify-center rounded-xl bg-black/5 dark:bg-white/10 active:scale-90 transition-all border border-black/5 dark:border-white/5"
          :class="themeStore.isDarkResolved ? 'text-blue-300/80' : 'text-yellow-500'
            ">
          <svg v-if="themeStore.isDarkResolved" width="18" height="18" viewBox="0 0 24 24" fill="none"
            stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z"></path>
          </svg>
          <svg v-else width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
            stroke-linecap="round" stroke-linejoin="round">
            <circle cx="12" cy="12" r="5"></circle>
            <line x1="12" y1="1" x2="12" y2="3"></line>
            <line x1="12" y1="21" x2="12" y2="23"></line>
            <line x1="4.22" y1="4.22" x2="5.64" y2="5.64"></line>
            <line x1="18.36" y1="18.36" x2="19.78" y2="19.78"></line>
            <line x1="1" y1="12" x2="3" y2="12"></line>
            <line x1="21" y1="12" x2="23" y2="12"></line>
            <line x1="4.22" y1="19.78" x2="5.64" y2="18.36"></line>
            <line x1="18.36" y1="5.64" x2="19.78" y2="4.22"></line>
          </svg>
        </button>
        <!-- 通知中心按钮 -->
        <button @click="layoutStore.toggleRightDrawer()"
          class="w-10 h-10 flex items-center justify-center rounded-xl bg-black/5 dark:bg-white/10 active:scale-90 transition-all border border-black/5 dark:border-white/5">
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
            stroke-linecap="round" stroke-linejoin="round">
            <path d="M18 8A6 6 0 0 0 6 8c0 7-3 9-3 9h18s-3-2-3-9"></path>
            <path d="M13.73 21a2 2 0 0 1-3.46 0"></path>
          </svg>
        </button>
      </div>
    </header>

    <!-- 2. 消息展示区 (确保 flex-1 撑开) -->
    <div ref="messageListRef" @scroll="handleScroll" class="flex-1 overflow-y-auto py-4 space-y-2 relative">
      <!-- 零数据引导状态 -->
      <div v-if="chatStore.currentChatHistory.length === 0"
        class="absolute inset-0 flex flex-col items-center justify-center text-center px-10">
        <div
          class="w-24 h-24 rounded-[2.5rem] bg-gradient-to-br from-primary/20 to-blue-500/20 flex-center mb-8 border border-white/10 shadow-2xl rotate-3">
          <svg width="48" height="48" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5"
            class="text-primary opacity-60">
            <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"></path>
          </svg>
        </div>
        <h3 class="text-xl font-bold text-primary-text mb-3">开启智能对话</h3>
        <p class="text-sm opacity-50 leading-relaxed max-w-[260px]">
          {{
            chatStore.currentSelectedItem
              ? "当前话题暂无消息，开始发送第一条吧！"
              : "请从左侧列表选择一个助手，或前往助手市场发现更多可能。"
          }}
        </p>
        <button v-if="!chatStore.currentSelectedItem" @click="layoutStore.toggleLeftDrawer()"
          class="mt-10 px-8 py-4 bg-primary text-white rounded-2xl text-sm font-bold shadow-lg shadow-primary/20 active:scale-95 transition-all">
          立即开始
        </button>
      </div>

      <MessageRenderer v-for="msg in chatStore.currentChatHistory" :key="msg.id" :message="msg"
        :agent-id="chatStore.currentSelectedItem?.id" />
      <div class="h-20"></div>
      <!-- 底部填充，防止输入框挡住最后一条 -->
    </div>

    <!-- 一键置底按钮 -->
    <Transition name="fade-slide-up">
      <button v-if="showScrollToBottom" @click="scrollToBottom(true)"
        class="absolute bottom-24 right-4 w-10 h-10 bg-white/80 dark:bg-gray-800/80 backdrop-blur-md rounded-full shadow-lg border border-black/10 dark:border-white/10 flex items-center justify-center text-primary-text z-50 active:scale-90 transition-all">
        <ArrowDown :size="20" />
      </button>
    </Transition>

    <!-- 3. 输入增强区 (固定底部) -->
    <footer class="px-4 py-1.5 bg-black/10 backdrop-blur-md border-t border-white/5 shrink-0">
      <InputEnhancer :disabled="!chatStore.currentTopicId" @send="chatStore.sendMessage" />
      <div class="h-[var(--vcp-safe-bottom, 20px)]"></div>
    </footer>
  </div>
</template>

<style scoped>
.vcp-header-fixed {
  /* 强制适配刘海屏，增加保底 padding */
  padding-top: calc(var(--vcp-safe-top, 24px) + 8px);
  padding-bottom: 12px;
  background-color: color-mix(in srgb, var(--secondary-bg) 80%, transparent);
  backdrop-filter: blur(20px) saturate(180%);
  -webkit-backdrop-filter: blur(20px) saturate(180%);
  border-bottom: 1px solid transparent;
}

/* 隐藏滚动条 */
.overflow-y-auto {
  scrollbar-width: none;
  -ms-overflow-style: none;
}

.overflow-y-auto::-webkit-scrollbar {
  display: none;
}

.fade-slide-up-enter-active,
.fade-slide-up-leave-active {
  transition: all 0.3s cubic-bezier(0.4, 0, 0.2, 1);
}

.fade-slide-up-enter-from,
.fade-slide-up-leave-to {
  opacity: 0;
  transform: translateY(10px) scale(0.9);
}

.core-glow-red {
  box-shadow: 0 0 6px 1px rgba(239, 68, 68, 0.6);
}
</style>

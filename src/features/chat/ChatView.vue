<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted, watch, nextTick } from "vue";
import { useChatSessionStore } from "../../core/stores/chatSessionStore";
import { useChatHistoryStore } from "../../core/stores/chatHistoryStore";
import { useChatStreamStore } from "../../core/stores/chatStreamStore";
import { useTopicStore } from "../../core/stores/topicListManager";
import { useThemeStore } from "../../core/stores/theme";
import { useAppLifecycleStore } from "../../core/stores/appLifecycle";
import { useLayoutStore } from "../../core/stores/layout";
import MessageRenderer from "./MessageRenderer.vue";
import InputEnhancer from "./InputEnhancer.vue";
import VcpAvatar from "../../components/ui/VcpAvatar.vue";
import CoreStatusIndicator from "../../components/ui/CoreStatusIndicator.vue";
import { ArrowDown } from "lucide-vue-next";

const sessionStore = useChatSessionStore();
const historyStore = useChatHistoryStore();
const topicStore = useTopicStore();
const streamStore = useChatStreamStore();
const themeStore = useThemeStore();
const lifecycleStore = useAppLifecycleStore();
const layoutStore = useLayoutStore();

// 自动滚动到底部
const messageListRef = ref<HTMLElement | null>(null);
const chatViewContainerRef = ref<HTMLElement | null>(null);
const showScrollToBottom = ref(false);

// 哨兵元素（IntersectionObserver 双哨兵架构，替代 scroll 事件）
const topSentinelRef = ref<HTMLElement | null>(null);
const bottomSentinelRef = ref<HTMLElement | null>(null);
let topObserver: IntersectionObserver | null = null;
let bottomObserver: IntersectionObserver | null = null;
let topSentinelVisible = false;

// 流式消息持续置底（RAF 轮询，替代 MutationObserver）
const isStreamingActive = computed(() => streamStore.activeStreamingIds.size > 0);
let scrollRafId: number | null = null;
let lastScrollHeight = 0;

const scrollToBottom = (smooth = false) => {
  if (messageListRef.value) {
    messageListRef.value.scrollTo({
      top: messageListRef.value.scrollHeight,
      behavior: smooth ? "smooth" : "auto",
    });
  }
};

// --- IntersectionObserver 双哨兵架构 ---
const setupTopSentinelObserver = () => {
  if (!messageListRef.value || !topSentinelRef.value) return;
  topObserver = new IntersectionObserver(
    (entries) => {
      topSentinelVisible = entries[0].isIntersecting;
      if (topSentinelVisible && historyStore.hasMoreHistory && !historyStore.isLoadingHistory) {
        historyStore.loadMoreHistory();
      }
    },
    {
      root: messageListRef.value,
      rootMargin: "200px 0px 0px 0px", // 提前 200px 触发
      threshold: 0,
    },
  );
  topObserver.observe(topSentinelRef.value);
};

const setupBottomSentinelObserver = () => {
  if (!messageListRef.value || !bottomSentinelRef.value) return;
  bottomObserver = new IntersectionObserver(
    (entries) => {
      // 底部哨兵不可见 = 用户已离开底部 > 150px
      showScrollToBottom.value = !entries[0].isIntersecting;
    },
    {
      root: messageListRef.value,
      rootMargin: "0px 0px 150px 0px", // 视口底部向下扩展 150px，哨兵在扩展区内 = 距底 ≤ 150px
      threshold: 0,
    },
  );
  bottomObserver.observe(bottomSentinelRef.value);
};

// 监听话题切换，触发历史加载与状态重置
watch(
  () => sessionStore.currentTopicId,
  (newTopicId) => {
    showScrollToBottom.value = false;
    if (newTopicId && sessionStore.currentSelectedItem) {
      console.log(`[ChatView] Topic changed to ${newTopicId}, loading history...`);
      topicStore.markTopicAsRead(newTopicId);
      historyStore.loadHistoryPaginated(
        sessionStore.currentSelectedItem.id,
        sessionStore.currentSelectedItem.type,
        newTopicId
      );
    } else if (!newTopicId) {
      historyStore.currentChatHistory = [];
    }
  },
  { immediate: true }
);

// 监听消息列表长度变化：首屏加载期间无动画置底，之后平滑滚动
watch(
  () => historyStore.currentChatHistory.length,
  async () => {
    if (!showScrollToBottom.value) {
      await nextTick();
      scrollToBottom(!historyStore.isLoadingHistory);
    }
  },
);





// --- 阻止不可滚动区域的 touchmove，防止键盘弹起后 footer/空白区滑动导致页面被拖动 ---
const handleContainerTouchMove = (e: TouchEvent) => {
  if (e.target instanceof Element) {
    // 放行可滚动区域内的 touchmove（消息列表、输入框、附件预览、侧边栏等）
    const scrollable = e.target.closest(
      ".overflow-y-auto, .overflow-x-auto, textarea, input, .vcp-scrollable",
    );
    if (scrollable) return;
  }
  // 在不可滚动区域阻止默认行为，阻断 WebView viewport panning
  e.preventDefault();
};

const handleVcpButtonClick = (e: any) => {
  if (e.detail && e.detail.text) {
    historyStore.sendMessage(e.detail.text);
  }
};

// --- Streaming Auto-Scroll (RAF-based, replaces MutationObserver) ---
const startStreamingScroll = () => {
  if (scrollRafId) return;
  lastScrollHeight = messageListRef.value?.scrollHeight ?? 0;
  const tick = () => {
    if (!messageListRef.value) return;
    const sh = messageListRef.value.scrollHeight;
    
    // 只要 scrollHeight 发生变化，且用户处于置底状态，就尝试置底
    // sh < lastScrollHeight 通常发生在气泡重渲染或高度坍缩时，此时也需要重置追踪器
    if (sh !== lastScrollHeight) {
      if (!showScrollToBottom.value) {
        scrollToBottom(false);
      }
      lastScrollHeight = sh;
    }
    scrollRafId = requestAnimationFrame(tick);
  };
  scrollRafId = requestAnimationFrame(tick);
};

const stopStreamingScroll = () => {
  if (scrollRafId) {
    cancelAnimationFrame(scrollRafId);
    scrollRafId = null;
  }
};

watch(isStreamingActive, (active) => {
  active ? startStreamingScroll() : stopStreamingScroll();
});

// --- Keyboard Offset & Viewport Handler ---
let keyboardRafId: number | null = null;

const applyKeyboardOffset = () => {
  if (!chatViewContainerRef.value || !window.visualViewport) return;

  let keyboardHeight = Math.max(
    0,
    window.innerHeight - window.visualViewport.height - window.visualViewport.offsetTop,
  );

  // 阈值处理：若键盘高度很小，视为 adjustResize 已生效的计算误差，避免错误增加 footer 高度
  if (keyboardHeight < 30) {
    keyboardHeight = 0;
  }

  chatViewContainerRef.value.style.setProperty(
    "--keyboard-offset",
    `${keyboardHeight}px`,
  );

  if (keyboardHeight > 0 && historyStore.currentChatHistory.length > 0) {
    scrollToBottom(true);
  }
};

const handleViewportResize = () => {
  if (keyboardRafId) return;
  keyboardRafId = requestAnimationFrame(() => {
    keyboardRafId = null;
    applyKeyboardOffset();
  });
};

onMounted(async () => {
  // 监听来自内联 HTML 按钮的点击事件
  window.addEventListener("vcp-button-click", handleVcpButtonClick);

  // 监听视口变化（键盘弹起）- 保持引用以便销毁
  if (window.visualViewport) {
    window.visualViewport.addEventListener("resize", handleViewportResize);
  }

  // 监听任意元素获得焦点，即时补偿键盘偏移
  if (chatViewContainerRef.value) {
    chatViewContainerRef.value.addEventListener("focusin", applyKeyboardOffset);
  }

  // 初始化 IntersectionObserver 双哨兵
  setupTopSentinelObserver();
  setupBottomSentinelObserver();

  // 兜底：若消息列表未产生滚动条但仍有更多数据，主动触发加载
  if (messageListRef.value && messageListRef.value.scrollHeight <= messageListRef.value.clientHeight) {
    if (historyStore.hasMoreHistory && !historyStore.isLoadingHistory) {
      historyStore.loadMoreHistory();
    }
  }
});

onUnmounted(() => {
  window.removeEventListener("vcp-button-click", handleVcpButtonClick);

  if (window.visualViewport) {
    window.visualViewport.removeEventListener("resize", handleViewportResize);
  }

  if (chatViewContainerRef.value) {
    chatViewContainerRef.value.removeEventListener("focusin", applyKeyboardOffset);
  }

  topObserver?.disconnect();
  bottomObserver?.disconnect();

  stopStreamingScroll();
});
</script>

<template>
  <div ref="chatViewContainerRef" class="chat-view-container flex flex-col h-full w-full min-w-0 relative bg-transparent overflow-hidden" @touchmove="handleContainerTouchMove">
    <!-- 1. Header (强制保底高度 80px，确保刘海屏可见) -->
    <header class="vcp-header-fixed shrink-0 flex items-center justify-between gap-3 px-4 border-b border-white/5">
      <div class="flex items-center gap-3 min-w-0 flex-1">
        <!-- 侧边栏按钮 (使用内联 SVG 确保 100% 可见) -->
        <button @click="layoutStore.toggleLeftDrawer()"
          class="w-10 h-10 shrink-0 flex items-center justify-center rounded-xl bg-black/5 dark:bg-white/10 active:scale-90 transition-all border border-black/5 dark:border-white/5">
          <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"
            stroke-linecap="round">
            <line x1="3" y1="12" x2="21" y2="12"></line>
            <line x1="3" y1="6" x2="21" y2="6"></line>
            <line x1="3" y1="18" x2="21" y2="18"></line>
          </svg>
        </button>

        <!-- 头像展示 -->
        <VcpAvatar 
          v-if="sessionStore.currentSelectedItem"
          :owner-type="sessionStore.currentSelectedItem.type" 
          :owner-id="sessionStore.currentSelectedItem.id" 
          :fallback-name="sessionStore.currentSelectedItem.name"
          :dominant-color="sessionStore.currentSelectedItem.avatarCalculatedColor"
          :outer-border="true"
          size="w-10 h-10"
          rounded="rounded-full"
          class="shrink-0"
        />

        <div class="flex flex-col min-w-0 flex-1">
          <span 
            class="font-bold text-sm truncate transition-colors duration-500"
            :style="{ color: sessionStore.currentSelectedItem?.avatarCalculatedColor || 'var(--primary-text)' }"
          >
            {{ sessionStore.currentSelectedItem?.name || "VCP Mobile" }}
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
    <div ref="messageListRef" class="flex-1 overflow-y-auto py-4 space-y-2 relative" style="overscroll-behavior-y: contain;">
      <!-- 零数据引导状态 -->
      <div v-if="historyStore.currentChatHistory.length === 0"
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
            sessionStore.currentSelectedItem
              ? "当前话题暂无消息，开始发送第一条吧！"
              : "请从左侧列表选择一个助手，或前往助手市场发现更多可能。"
          }}
        </p>
        <button v-if="!sessionStore.currentSelectedItem" @click="layoutStore.toggleLeftDrawer()"
          class="mt-10 px-8 py-4 bg-primary text-white rounded-2xl text-sm font-bold shadow-lg shadow-primary/20 active:scale-95 transition-all">
          立即开始
        </button>
      </div>

      <!-- 顶部哨兵：IntersectionObserver 监听分页触发 -->
      <div ref="topSentinelRef" class="h-0"></div>

      <MessageRenderer
        v-for="msg in historyStore.currentChatHistory"
        :key="msg.id"
        :message="msg"
        :agent-id="sessionStore.currentSelectedItem?.id"
        :data-message-id="msg.id"
      />
      <!-- 底部哨兵：IntersectionObserver 监听置底按钮状态 -->
      <div ref="bottomSentinelRef" class="h-0"></div>
      <div class="h-20"></div>
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
      <InputEnhancer :disabled="!sessionStore.currentTopicId" @send="historyStore.sendMessage" />
      <div class="h-[calc(var(--vcp-safe-bottom,20px)+var(--keyboard-offset,0px))] no-swipe pointer-events-none"></div>
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

@media (hover: none) and (pointer: coarse) {
  .vcp-header-fixed {
    backdrop-filter: blur(4px) saturate(180%);
    -webkit-backdrop-filter: blur(4px) saturate(180%);
  }
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

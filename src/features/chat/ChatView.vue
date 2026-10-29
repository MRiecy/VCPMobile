<script setup lang="ts">
import { ref, computed, nextTick, onMounted, onUnmounted, watch } from "vue";
import { useChatSessionStore, type ConversationKey } from "../../core/stores/chatSessionStore";
import { useChatHistoryStore } from "../../core/stores/chatHistoryStore";
import { useChatStreamStore } from "../../core/stores/chatStreamStore";
import { useTopicStore } from "../../core/stores/topicListManager";
import { useThemeStore } from "../../core/stores/theme";
import { useChatPreferencesStore } from "./preferences/store";
import { useOverlayStore } from "../../core/stores/overlay";
import { useAppLifecycleStore } from "../../core/stores/appLifecycle";
import { useLayoutStore } from "../../core/stores/layout";
import { useNotificationStore } from "../../core/stores/notification";
import MessageRenderer from "./MessageRenderer.vue";
import InputEnhancer from "./InputEnhancer.vue";
import TarvenSelector from "./components/TarvenSelector.vue";
import VcpAvatar from "../../components/ui/VcpAvatar.vue";
import CoreStatusIndicator from "../../components/ui/CoreStatusIndicator.vue";
import { ArrowDown, ListChecks, SlidersHorizontal } from "@lucide/vue";
import VoiceCallControls from "./voice/VoiceCallControls.vue";
import { useVoiceCallStore } from "./voice/voiceCallStore";
import { useKeyboardInsets } from "../../core/composables/useKeyboardInsets";
import { useChatScroll } from "../../core/composables/useChatScroll";
import { openPresentationMenu } from "./presentationMenu";

const emit = defineEmits<{
  (event: "content-settled", key: ConversationKey | null): void;
  (event: "content-pending", key: ConversationKey | null): void;
}>();
let contentGeneration = 0;
let viewDisposed = false;

const sessionStore = useChatSessionStore();
const historyStore = useChatHistoryStore();
const topicStore = useTopicStore();
const streamStore = useChatStreamStore();
const themeStore = useThemeStore();
const chatPreferences = useChatPreferencesStore();
const lifecycleStore = useAppLifecycleStore();
const layoutStore = useLayoutStore();
const notificationStore = useNotificationStore();
const overlayStore = useOverlayStore();
const voiceCall = useVoiceCallStore();
const { keyboardHeight, forceRecalculate } = useKeyboardInsets();

// 容器 refs
const messageListRef = ref<HTMLElement | null>(null);
const chatViewContainerRef = ref<HTMLElement | null>(null);
const inputRef = ref<InstanceType<typeof InputEnhancer> | null>(null);
const footerRef = ref<HTMLElement | null>(null);
const footerHeight = ref(0);
let footerObserver: ResizeObserver | null = null;

// 哨兵元素已废弃，新滚动架构不再需要
// const topSentinelRef = ref<HTMLElement | null>(null);
// const bottomSentinelRef = ref<HTMLElement | null>(null);

// 流式状态
const isStreamingActive = computed(() => streamStore.activeStreamingIds.size > 0);
const isConversationReady = computed(() => {
  const current = sessionStore.currentConversationKey;
  const loaded = historyStore.loadedConversationKey;
  return Boolean(
    current &&
      loaded &&
      current.ownerId === loaded.ownerId &&
      current.ownerType === loaded.ownerType &&
      current.topicId === loaded.topicId &&
      current.epoch === loaded.epoch,
  );
});

// 滚动管理 composable（封装 IO 双哨兵 + RAF 轮询 + 消息新增自动滚动）
const {
  showScrollToBottom,
  scrollToBottom,
  startAutoScroll,
  stopAutoScroll,
  checkAndLoadMore,
  preserveViewportAcrossLayoutChange,
  reset: resetChatScroll,
  dispose: disposeChatScroll,
} = useChatScroll({
  messageListRef,
  messageCount: computed(() => historyStore.currentChatHistory.length),
  hasMoreHistory: computed(() => historyStore.hasMoreHistory),
  isLoadingHistory: computed(() => historyStore.isLoadingHistory),
  onLoadMore: () => historyStore.loadMoreHistory(),
});

const isPresentationMenuOpen = computed(
  () => overlayStore.contextMenuConfig?.title === "消息呈现",
);

// The pre-flush watcher captures the old geometry before Vue patches the data
// attribute. This protects both the long-press menu and the settings-page entry.
watch(
  () => chatPreferences.presentationMode,
  () => {
    void preserveViewportAcrossLayoutChange(() => undefined);
  },
  { flush: "pre" },
);

watch(() => voiceCall.active, () => {
  void preserveViewportAcrossLayoutChange(() => undefined);
}, { flush: 'pre' });

const startVoiceCall = () => {
  if (!isConversationReady.value || voiceCall.active) return;
  inputRef.value?.suspendComposer();
  layoutStore.setLeftDrawer(false);
  layoutStore.setRightDrawer(false);
  voiceCall.start();
};

const scrollToLatest = async (smooth = true) => {
  if (historyStore.hasEvictedNewer && !historyStore.editingMessage) {
    const result = await historyStore.returnToLatest();
    if (result.error || result.aborted || historyStore.hasEvictedNewer) return;
    await nextTick();
  }
  scrollToBottom(smooth);
};

// 监听话题切换与智能体变更，触发历史加载与防御性状态清空
watch(
  () => sessionStore.currentConversationKey,
  (key) => {
    showScrollToBottom.value = false;
    resetChatScroll();
    if (key) {
      console.log(`[ChatView] Topic changed to ${key.topicId}, loading history...`);
      topicStore.markTopicAsRead(key.ownerId, key.ownerType, key.topicId);
      historyStore.loadHistoryPaginated(
        key.ownerId,
        key.ownerType,
        key.topicId,
      );
    } else {
      console.log('[ChatView] Clearing chat history (Selected item or Topic became null).');
      historyStore.resetHistoryForConversation();
    }
  },
  { immediate: true }
);

// Follow the actual history owner, including a replacement load in the same
// conversation. An old aborted Promise cannot declare the new page ready.
watch(
  [() => historyStore.loading, isConversationReady, () => sessionStore.currentConversationKey?.epoch],
  async ([loading]) => {
    const generation = ++contentGeneration;
    const key = sessionStore.currentConversationKey;
    if (!isConversationReady.value) emit("content-pending", key);
    // Loading an older page retains the current visible window. Only a reset
    // current window revokes readiness, including a reload in the same epoch.
    if (loading && !isConversationReady.value) return;
    await nextTick();
    if (viewDisposed || generation !== contentGeneration
      || historyStore.loading && !isConversationReady.value) return;
    if (key ? !sessionStore.isConversationCurrent(key) : sessionStore.currentConversationKey !== null) return;
    // Load errors already have visible feedback and are a usable rendered state.
    emit("content-settled", key);
  },
  { immediate: true, flush: "sync" },
);

// --- 阻止非交互空白区域的 touchmove，防止 WebView viewport 被拖动 ---
const handleContainerTouchMove = (e: TouchEvent) => {
  // 系统滚动过程中事件不可取消，强行 preventDefault 会报错
  if (!e.cancelable) return;
  if (e.target instanceof Element) {
    const scrollable = e.target.closest(
      ".overflow-y-auto, .overflow-x-auto, textarea, input, .vcp-scrollable, button, a",
    );
    if (scrollable) return;
  }
  e.preventDefault();
};

const handleVcpButtonClick = (e: any) => {
  if (e.detail && e.detail.text) {
    historyStore.sendMessage(e.detail.text);
  }
};


watch(isStreamingActive, (active) => {
  if (active) startAutoScroll();
  else stopAutoScroll();
});

// --- Keyboard Offset & Inset Handler ---
watch(keyboardHeight, (height) => {
  if (!chatViewContainerRef.value) return;

  chatViewContainerRef.value.style.setProperty(
    "--keyboard-offset",
    `${height}px`,
  );

  // 只有当焦点确实处于主界面的输入框中时，键盘高度变化才执行置底
  const isMainInputFocused = document.activeElement?.classList.contains("vcp-textarea");
  if (!isMainInputFocused) {
    return;
  }

  if (height > 0 && historyStore.currentChatHistory.length > 0) {
    void scrollToLatest(true);
  }
});

onMounted(async () => {
  window.addEventListener("vcp-button-click", handleVcpButtonClick);

  if (chatViewContainerRef.value) {
    chatViewContainerRef.value.addEventListener("focusin", forceRecalculate);
  }

  checkAndLoadMore();
  if (footerRef.value) {
    footerHeight.value = footerRef.value.getBoundingClientRect().height;
    footerObserver = new ResizeObserver(() => {
      footerHeight.value = footerRef.value?.getBoundingClientRect().height ?? 0;
    });
    footerObserver.observe(footerRef.value);
  }
});

onUnmounted(() => {
  footerObserver?.disconnect();
  viewDisposed = true;
  contentGeneration += 1;
  window.removeEventListener("vcp-button-click", handleVcpButtonClick);

  if (chatViewContainerRef.value) {
    chatViewContainerRef.value.removeEventListener("focusin", forceRecalculate);
  }

  disposeChatScroll();
});
</script>

<template>
  <div ref="chatViewContainerRef" class="chat-view-container flex flex-col h-full w-full min-w-0 relative bg-transparent overflow-hidden"
    :data-presentation-mode="chatPreferences.presentationMode" :data-voice-active="voiceCall.active || undefined"
    @touchmove="handleContainerTouchMove">
    <!-- 1. Header (强制保底高度 80px，确保刘海屏可见) -->
    <header class="vcp-header-fixed shrink-0 flex items-center justify-between gap-3 px-4 border-b border-white/5">
      <div class="flex items-center gap-3 min-w-0 flex-1">
        <!-- 侧边栏按钮 (使用内联 SVG 确保 100% 可见) -->
        <button @click="voiceCall.active ? overlayStore.openVoiceStyleGuidance() : layoutStore.toggleLeftDrawer()"
          class="w-10 h-10 shrink-0 flex items-center justify-center rounded-xl bg-black/5 dark:bg-white/10 active:opacity-60 transition-opacity border border-black/5 dark:border-white/5"
          :class="{ 'vcp-left-drawer-trigger': !voiceCall.active }"
          :aria-label="voiceCall.active ? '风格指导' : '打开助手与话题侧栏'"
          :aria-controls="voiceCall.active ? undefined : 'agent-sidebar'"
          :aria-expanded="voiceCall.active ? undefined : layoutStore.leftDrawerOpen">
          <Transition name="voice-icon" mode="out-in">
          <ListChecks v-if="voiceCall.active" :size="21" :stroke-width="1.8" aria-hidden="true" />
          <svg v-else width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"
            stroke-linecap="round">
            <line x1="3" y1="12" x2="21" y2="12"></line>
            <line x1="3" y1="6" x2="21" y2="6"></line>
            <line x1="3" y1="18" x2="21" y2="18"></line>
          </svg>
          </Transition>
        </button>

        <!-- 头像展示 -->
        <VcpAvatar 
          v-if="sessionStore.currentSelectedItem"
          :target="sessionStore.currentSelectedItem"
          size="w-10 h-10"
          rounded="rounded-full"
          class="shrink-0"
        />

        <div class="flex flex-col min-w-0 flex-1">
          <span 
            class="font-bold text-sm truncate"
            :style="{ color: sessionStore.currentSelectedItem?.avatarCalculatedColor || 'var(--primary-text)' }"
          >
            {{ sessionStore.headerTitle }}
          </span>
  <div class="flex items-center gap-1" :title="lifecycleStore.errorMsg || undefined">
    <CoreStatusIndicator />
  </div>
</div>
</div>

<div class="flex items-center gap-2 shrink-0">
        <!-- 黑白模式切换 (内联 SVG) -->
        <button @click="themeStore.toggleTheme()" v-longpress.suppress-click="openPresentationMenu"
          v-guide="'chat-theme-button'"
          class="w-10 h-10 flex items-center justify-center rounded-xl bg-black/5 dark:bg-white/10 active:scale-90 transition-transform border border-black/5 dark:border-white/5"
          :class="themeStore.isDarkResolved ? 'text-blue-300/80' : 'text-yellow-500'
            " aria-label="切换深浅模式，长按切换消息呈现" aria-haspopup="dialog"
          :aria-expanded="isPresentationMenuOpen">
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
        <button @click="voiceCall.active ? overlayStore.openVoiceSettings() : layoutStore.toggleRightDrawer()"
          class="w-10 h-10 flex items-center justify-center rounded-xl bg-black/5 dark:bg-white/10 active:opacity-60 transition-opacity border border-black/5 dark:border-white/5 relative"
          :class="{ 'vcp-right-drawer-trigger': !voiceCall.active }"
          :aria-label="voiceCall.active ? '语音设置' : '打开通知与工具侧栏'"
          :aria-controls="voiceCall.active ? undefined : 'notification-sidebar'"
          :aria-expanded="voiceCall.active ? undefined : layoutStore.rightDrawerOpen">
          <Transition name="voice-icon" mode="out-in">
          <SlidersHorizontal v-if="voiceCall.active" :size="21" :stroke-width="1.8" aria-hidden="true" />
          <svg v-else width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
            stroke-linecap="round" stroke-linejoin="round">
            <path d="M18 8A6 6 0 0 0 6 8c0 7-3 9-3 9h18s-3-2-3-9"></path>
            <path d="M13.73 21a2 2 0 0 1-3.46 0"></path>
          </svg>
          </Transition>
          <!-- 当有未读通知时显示绿色指示点 -->
          <div v-if="!voiceCall.active && notificationStore.unreadCount > 0"
            class="absolute top-1.5 right-1.5 w-2 h-2 bg-emerald-500 rounded-full border-2 border-[var(--secondary-bg)] shadow-[0_0_8px_rgba(16,185,129,0.5)]">
          </div>
        </button>
      </div>
    </header>

    <div class="chat-conversation-surface flex-1 min-h-0 min-w-0 flex flex-col relative">
    <!-- 2. 消息展示区 (确保 flex-1 撑开) -->
    <div ref="messageListRef" class="flex-1 min-h-0 overflow-y-auto relative no-rubber-band">
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
          class="vcp-left-drawer-trigger mt-10 px-8 py-4 bg-primary text-white rounded-2xl text-sm font-bold shadow-lg shadow-primary/20 active:scale-95 transition-all"
          aria-controls="agent-sidebar"
          :aria-expanded="layoutStore.leftDrawerOpen">
          立即开始
        </button>
      </div>

      <div class="messages-inner-container py-4 space-y-2 flex flex-col min-h-full">
        <MessageRenderer
          v-for="msg in historyStore.currentChatHistory"
          :key="msg.id"
          :message="msg"
          :agent-id="sessionStore.currentSelectedItem?.id"
          :is-background="lifecycleStore.isBackground"
          :data-message-id="msg.id"
        />
        <!-- 底部留白，避免最后一条消息被输入框遮挡 -->
        <div class="h-20"></div>
      </div>
    </div>

    <!-- 一键置底按钮 -->
    <Transition name="fade-slide-up">
      <button v-if="showScrollToBottom || historyStore.hasEvictedNewer" @click="scrollToLatest(true)"
        class="absolute right-4 w-10 h-10 bg-white/80 dark:bg-gray-800/80 rounded-full shadow-lg border border-black/10 dark:border-white/10 flex items-center justify-center text-primary-text z-local active:scale-90 transition-[bottom,transform,opacity] duration-300 ease-[cubic-bezier(0.34,1.56,0.64,1)]"
        :style="{ bottom: `${footerHeight + 12}px` }"
      >
        <ArrowDown :size="20" />
      </button>
    </Transition>

    <!-- 3. 输入增强区 (固定底部) -->
    <footer ref="footerRef" class="vcp-input-footer relative px-4 py-1.5 shrink-0"
      :class="voiceCall.active ? 'voice-footer' : 'border-t border-white/5'">
      <Transition name="voice-fade">
      <div v-show="!voiceCall.active" :inert="voiceCall.active" :aria-hidden="voiceCall.active || undefined">
      <InputEnhancer ref="inputRef"
        :disabled="!sessionStore.currentTopicId || !isConversationReady"
        :suspended="voiceCall.active"
        @send="historyStore.sendMessage" 
        @start-call="startVoiceCall"
        @focus-input="scrollToLatest(true)"
      />
      </div>
      </Transition>
      <Transition name="voice-fade">
        <VoiceCallControls v-if="voiceCall.active"
          :microphone-state="voiceCall.microphoneState" :level="voiceCall.level" :bands="voiceCall.bands"
          :error-message="voiceCall.errorMessage"
          :service-state="voiceCall.serviceState" :service-message="voiceCall.serviceMessage" :partial-text="voiceCall.partialText"
          @toggle-microphone="voiceCall.toggleMicrophone" @hangup="voiceCall.end" />
      </Transition>
      <div class="no-swipe pointer-events-none" :style="{ height: voiceCall.active
        ? 'var(--vcp-safe-bottom, 0px)'
        : 'calc(var(--vcp-safe-bottom, 48px) + var(--keyboard-offset, 0px))' }"></div>
    </footer>
    </div>

    <!-- VCPChatTarven 规则快捷选择器 -->
    <TarvenSelector />
  </div>
</template>

<style scoped>
.vcp-header-fixed {
  /* 强制适配刘海屏，增加保底 padding */
  padding-top: calc(var(--vcp-safe-top, 24px));
  padding-bottom: 12px;
  background-color: var(--vcp-panel-bg-97, var(--secondary-bg));
  border-bottom: 1px solid transparent;
}

.vcp-input-footer {
  background-color: var(--vcp-panel-bg-90, var(--secondary-bg));
}

.vcp-input-footer.voice-footer {
  background: transparent;
  border: 0;
}

.voice-fade-enter-active, .voice-fade-leave-active {
  transition: opacity 200ms ease;
}
.voice-fade-leave-active {
  position: absolute;
  width: auto;
  top: 6px;
  left: 16px;
  right: 16px;
  pointer-events: none;
}
.voice-fade-enter-from, .voice-fade-leave-to,
.voice-icon-enter-from, .voice-icon-leave-to {
  opacity: 0;
}
.voice-icon-enter-active, .voice-icon-leave-active {
  transition: opacity 100ms ease;
}
@media (prefers-reduced-motion: reduce) {
  .voice-fade-enter-active, .voice-fade-leave-active,
  .voice-icon-enter-active, .voice-icon-leave-active { transition-duration: 0ms; }
}

@media (min-width: 1024px) {
  .vcp-left-drawer-trigger {
    display: none;
  }
}

@media (min-width: 1280px) {
  .vcp-right-drawer-trigger {
    display: none;
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
  transition: opacity 0.3s cubic-bezier(0.4, 0, 0.2, 1), transform 0.3s cubic-bezier(0.4, 0, 0.2, 1);
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

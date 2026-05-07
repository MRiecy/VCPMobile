import { ref, watch, nextTick, type Ref, type ComputedRef } from "vue";

interface UseChatScrollOptions {
  /** 消息列表容器的 ref */
  messageListRef: Ref<HTMLElement | null>;
  /** 当前消息数量（用于监听新增） */
  messageCount: ComputedRef<number>;
  /** 是否还有更多历史消息可加载 */
  hasMoreHistory: Ref<boolean>;
  /** 是否正在加载历史 */
  isLoadingHistory: Ref<boolean>;
  /** 加载更多历史的回调 */
  onLoadMore: () => void;
}

/**
 * ChatView 滚动管理组合式函数
 *
 * 统一封装三类滚动机制：
 * 1. IntersectionObserver 双哨兵（顶部触发分页、底部控制置底按钮）
 * 2. RAF 轮询自动置底（流式消息期间）
 * 3. 消息数量变化时自动滚动（首屏/新增消息）
 */
export function useChatScroll(options: UseChatScrollOptions) {
  const { messageListRef, messageCount, hasMoreHistory, isLoadingHistory, onLoadMore } = options;

  const showScrollToBottom = ref(false);

  let topObserver: IntersectionObserver | null = null;
  let bottomObserver: IntersectionObserver | null = null;
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

  // --- IntersectionObserver 双哨兵 ---
  const setupObservers = (
    topSentinelRef: Ref<HTMLElement | null>,
    bottomSentinelRef: Ref<HTMLElement | null>,
  ) => {
    if (!messageListRef.value || !topSentinelRef.value || !bottomSentinelRef.value) return;

    topObserver = new IntersectionObserver(
      (entries) => {
        const isVisible = entries[0].isIntersecting;
        if (isVisible && hasMoreHistory.value && !isLoadingHistory.value) {
          onLoadMore();
        }
      },
      {
        root: messageListRef.value,
        rootMargin: "200px 0px 0px 0px",
        threshold: 0,
      },
    );
    topObserver.observe(topSentinelRef.value);

    bottomObserver = new IntersectionObserver(
      (entries) => {
        // 底部哨兵不可见 = 用户已离开底部 > 150px
        showScrollToBottom.value = !entries[0].isIntersecting;
      },
      {
        root: messageListRef.value,
        rootMargin: "0px 0px 150px 0px",
        threshold: 0,
      },
    );
    bottomObserver.observe(bottomSentinelRef.value);
  };

  // --- 消息新增时自动滚动 ---
  watch(messageCount, async () => {
    if (!showScrollToBottom.value) {
      await nextTick();
      scrollToBottom(!isLoadingHistory.value);
    }
  });

  // --- RAF 轮询自动置底（流式期间） ---
  const startAutoScroll = () => {
    if (scrollRafId) return;
    lastScrollHeight = messageListRef.value?.scrollHeight ?? 0;

    const tick = () => {
      if (!messageListRef.value) return;
      const sh = messageListRef.value.scrollHeight;

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

  const stopAutoScroll = () => {
    if (scrollRafId) {
      cancelAnimationFrame(scrollRafId);
      scrollRafId = null;
    }
  };

  // --- 兜底：若列表无滚动条但仍有数据，主动触发加载 ---
  const checkAndLoadMore = () => {
    if (!messageListRef.value) return;
    if (
      messageListRef.value.scrollHeight <= messageListRef.value.clientHeight &&
      hasMoreHistory.value &&
      !isLoadingHistory.value
    ) {
      onLoadMore();
    }
  };

  // --- 清理 ---
  const dispose = () => {
    topObserver?.disconnect();
    bottomObserver?.disconnect();
    stopAutoScroll();
  };

  return {
    showScrollToBottom,
    scrollToBottom,
    setupObservers,
    startAutoScroll,
    stopAutoScroll,
    checkAndLoadMore,
    dispose,
  };
}

import { ref, watch, nextTick, type Ref, type ComputedRef } from "vue";

interface UseChatScrollOptions {
  /** 消息列表容器的 ref */
  messageListRef: Ref<HTMLElement | null>;
  /** 当前消息数量（用于检测话题切换/清空） */
  messageCount: ComputedRef<number>;
  /** 是否还有更多历史消息可加载 */
  hasMoreHistory: Ref<boolean>;
  /** 是否正在加载历史 */
  isLoadingHistory: Ref<boolean>;
  /** 加载更多历史的回调 */
  onLoadMore: () => Promise<{
    addedCount: number;
    error?: unknown;
    aborted?: boolean;
  }>;
}

type ScrollScene = "initial" | "following" | "free" | "loading-top";

/**
 * ChatView 滚动管理组合式函数 —— 根治版
 *
 * 核心架构：场景状态机 + 锚定元素恢复 + RAF 节流
 *
 * 放弃 IntersectionObserver 的模糊 rootMargin 和 nextTick 的"猜时机"，
 * 改用 scroll 事件精确几何检测 + MutationObserver 双重 RAF 确保布局稳定后操作。
 */
export function useChatScroll(options: UseChatScrollOptions) {
  const {
    messageListRef,
    messageCount,
    hasMoreHistory,
    isLoadingHistory,
    onLoadMore,
  } = options;

  const showScrollToBottom = ref(false);
  const scrollScene = ref<ScrollScene>("initial");

  // 高性能测量与安全保护罩状态
  let lastScrollHeight = 0;
  const isInitialRendering = ref(true);

  // 加载锚点：记录加载前视口中最顶部可见消息
  let loadAnchor: { messageId: string; offsetFromTop: number } | null = null;
  let scrollThrottleId: number | null = null;
  let resizeObserver: ResizeObserver | null = null;
  let scrollRafId: number | null = null;
  let loadMoreDebounceId: number | null = null;
  let loadRequestGeneration = 0;
  let loadInFlight = false;

  const scrollToBottom = (smooth = false) => {
    const list = messageListRef.value;
    if (!list) return;
    list.scrollTo({
      top: list.scrollHeight,
      behavior: smooth ? "smooth" : "auto",
    });
  };

  // --- 锚定元素 ---
  const prepareLoadAnchor = () => {
    const list = messageListRef.value;
    if (!list) return;
    const messages = list.querySelectorAll("[data-message-id]");
    const listRect = list.getBoundingClientRect();
    for (const el of Array.from(messages)) {
      const rect = el.getBoundingClientRect();
      if (rect.top >= listRect.top) {
        loadAnchor = {
          messageId: el.getAttribute("data-message-id")!,
          offsetFromTop: rect.top - listRect.top,
        };
        break;
      }
    }
  };

  const restoreScrollByAnchor = () => {
    if (!loadAnchor || !messageListRef.value) return;
    const el = messageListRef.value.querySelector(
      `[data-message-id="${loadAnchor.messageId}"]`,
    );
    if (el) {
      messageListRef.value.scrollTop =
        (el as HTMLElement).offsetTop - loadAnchor.offsetFromTop;
    }
    loadAnchor = null;
  };

  const requestLoadMore = async () => {
    if (
      loadInFlight ||
      scrollScene.value === "loading-top" ||
      !hasMoreHistory.value ||
      isLoadingHistory.value
    ) {
      return;
    }

    loadAnchor = null;
    prepareLoadAnchor();
    const hadAnchor = Boolean(loadAnchor);
    const generation = ++loadRequestGeneration;
    loadInFlight = true;
    scrollScene.value = "loading-top";
    try {
      await onLoadMore();
    } finally {
      await nextTick();
      if (generation !== loadRequestGeneration) return;
      if (hadAnchor) {
        restoreScrollByAnchor();
        scrollScene.value = "free";
      } else {
        loadAnchor = null;
        scrollToBottom(false);
        scrollScene.value = "following";
      }
      loadInFlight = false;
    }
  };

  // --- 自动加载历史几何判定与防抖 ---
  const evaluateAutoLoadMore = () => {
    const list = messageListRef.value;
    if (!list) return;

    // 首屏防抖稳定后，关闭首屏渲染保护罩
    isInitialRendering.value = false;

    if (
      scrollScene.value !== "initial" &&
      list.scrollHeight <= list.clientHeight + 10 &&
      hasMoreHistory.value &&
      !isLoadingHistory.value
    ) {
      console.log(
        `[useChatScroll] Auto loading more because height (${list.scrollHeight}) <= clientHeight (${list.clientHeight}) + 10`,
      );
      void requestLoadMore();
    }
  };

  const triggerAutoLoadMoreWithDebounce = () => {
    if (loadMoreDebounceId) {
      clearTimeout(loadMoreDebounceId);
    }
    loadMoreDebounceId = setTimeout(() => {
      loadMoreDebounceId = null;
      evaluateAutoLoadMore();
    }, 200) as unknown as number; // 200ms 防抖，完美等待图片、头像与异步排版彻底稳定
  };

  // --- 内容变化处理（等效于 ResizeObserver 的布局稳定信号）---
  const handleContentChange = () => {
    const list = messageListRef.value;
    if (!list) return;

    const currentScrollHeight = list.scrollHeight;
    // 高度物理守卫：物理高度若无实质变化，瞬间拦截并退出。这极大释放了 CPU 性能，并从物理上秒杀了用户手动上滑时的误置底无限回弹 Bug
    if (currentScrollHeight === lastScrollHeight) return;
    lastScrollHeight = currentScrollHeight;

    // 场景1：首屏加载完成（initial → following/free）
    if (scrollScene.value === "initial") {
      if (!isLoadingHistory.value) {
        if (messageCount.value > 0) {
          scrollToBottom(false);
          scrollScene.value = "following";
          // 状态迁移后，触发防抖续载判定，防抖时间内的高频高度增长（如头像加载）会重定位滚动位置并推迟加载
          triggerAutoLoadMoreWithDebounce();
        } else {
          // 首屏无消息，切换到 free，允许用户上滑尝试加载
          scrollScene.value = "free";
          isInitialRendering.value = false; // 空状态直接解除首屏保护罩
        }
      }
      return;
    }

    // 场景2：跟随模式下的新内容/流式追加
    if (scrollScene.value === "following" && !showScrollToBottom.value) {
      scrollToBottom(false);
      // 随着内容变动，持续评估并推迟可能的自动续载
      triggerAutoLoadMoreWithDebounce();
      return;
    }

    // 场景3：其他高度变化引起的续载评估
    triggerAutoLoadMoreWithDebounce();
  };

  // --- ResizeObserver：监听 DOM 物理渲染尺寸变化 ---
  const startContentObserver = () => {
    const list = messageListRef.value;
    if (!list) return;

    if (resizeObserver) return;

    // 优先监听专门用于测量高度的内部容器，它是无条件渲染的，100% 存在
    const target = list.querySelector(".messages-inner-container") || list;

    resizeObserver = new ResizeObserver(() => {
      // 🌟 流式跟随状态下，或正在加载历史消息时，必须同步处理滚动，以防止 DOM 重排和滚动条设置跨帧引发的上下跳变。
      if (
        scrollScene.value === "following" ||
        scrollScene.value === "loading-top"
      ) {
        if (scrollRafId) {
          cancelAnimationFrame(scrollRafId);
          scrollRafId = null;
        }
        handleContentChange();
      } else {
        // 其他初始/非流式跟随场景，继续使用 RAF 节流以保证能耗和页面初载的稳定性
        if (scrollRafId) cancelAnimationFrame(scrollRafId);
        scrollRafId = requestAnimationFrame(() => {
          scrollRafId = null;
          handleContentChange();
        });
      }
    });

    resizeObserver.observe(target);
  };

  const stopContentObserver = () => {
    if (resizeObserver) {
      resizeObserver.disconnect();
      resizeObserver = null;
    }
    if (scrollRafId) {
      cancelAnimationFrame(scrollRafId);
      scrollRafId = null;
    }
  };

  // --- scroll 事件 ---
  const onScroll = () => {
    // 🌟 修复无限置底死锁：在 scroll 触发的第一时间，同步（非节流）判定是否已向上偏离底部。
    // 若已偏离，瞬间切入 free 状态，使紧随其后的 ResizeObserver 同步回调直接走 else 节流分支，打断强位置底。
    const list = messageListRef.value;
    if (list) {
      const nearBottom =
        list.scrollHeight - list.scrollTop - list.clientHeight < 150;
      if (!nearBottom && scrollScene.value === "following") {
        scrollScene.value = "free";
        showScrollToBottom.value = true;
      }
    }

    if (scrollThrottleId) return; // 已调度，节流中
    scrollThrottleId = requestAnimationFrame(() => {
      scrollThrottleId = null;
      const list = messageListRef.value;
      if (!list) return;

      // 如果首屏渲染尚未彻底完成稳定（高度还在持续图片加载增长），屏蔽自由滚动，防止状态机提前叛逃
      if (isInitialRendering.value) {
        showScrollToBottom.value = false;
        scrollScene.value = "following";
        return;
      }

      const nearTop = list.scrollTop < 100;
      const nearBottom =
        list.scrollHeight - list.scrollTop - list.clientHeight < 150;

      // 更新底部按钮显隐
      showScrollToBottom.value = !nearBottom;

      // 场景切换：following ↔ free
      if (nearBottom && scrollScene.value === "free") {
        scrollScene.value = "following";
      } else if (!nearBottom && scrollScene.value === "following") {
        scrollScene.value = "free";
      }

      // 触发顶部加载（仅在 free 状态下，避免 following 模式误触）
      if (
        nearTop &&
        scrollScene.value === "free" &&
        hasMoreHistory.value &&
        !isLoadingHistory.value
      ) {
        void requestLoadMore();
      }
    });
  };

  // --- 手势与鼠标滚轮物理置顶继续滑动判定 ---
  let startY = 0;

  const onTouchStart = (e: TouchEvent) => {
    if (e.touches.length > 0) {
      startY = e.touches[0].pageY;
    }
  };

  const onTouchMove = (e: TouchEvent) => {
    const list = messageListRef.value;
    if (!list) return;

    // 如果已经在最顶部，且用户手指继续向下拉（deltaY > 10px）
    if (list.scrollTop <= 2 && e.touches.length > 0) {
      const deltaY = e.touches[0].pageY - startY;
      if (deltaY > 10 && hasMoreHistory.value && !isLoadingHistory.value) {
        void requestLoadMore();
      }
    }
  };

  const onWheel = (e: WheelEvent) => {
    const list = messageListRef.value;
    if (!list) return;

    // 如果已经在最顶部，且鼠标滚轮继续向上滚（试图拉出顶部）
    if (list.scrollTop <= 2 && e.deltaY < 0) {
      if (hasMoreHistory.value && !isLoadingHistory.value) {
        void requestLoadMore();
      }
    }
  };

  // --- 监听 messageListRef 变化，自动设置/清理事件与 Observer ---
  const stopWatchListRef = watch(messageListRef, (el, oldEl) => {
    if (oldEl) {
      oldEl.removeEventListener("scroll", onScroll);
      oldEl.removeEventListener("touchstart", onTouchStart);
      oldEl.removeEventListener("touchmove", onTouchMove);
      oldEl.removeEventListener("wheel", onWheel);
    }
    if (el) {
      startContentObserver();
      el.addEventListener("scroll", onScroll, { passive: true });
      el.addEventListener("touchstart", onTouchStart, { passive: true });
      el.addEventListener("touchmove", onTouchMove, { passive: true });
      el.addEventListener("wheel", onWheel, { passive: true });
      stopWatchListRef();
    }
  });

  // 首屏完成由 history loading 明确信号驱动；分页完成由 requestLoadMore 的 Promise 驱动。
  watch(isLoadingHistory, async (loading) => {
    if (loading || scrollScene.value !== "initial") return;
    await nextTick();
    if (scrollScene.value !== "initial") return;
    if (messageCount.value > 0) {
      scrollToBottom(false);
      scrollScene.value = "following";
      triggerAutoLoadMoreWithDebounce();
    } else {
      scrollScene.value = "free";
      isInitialRendering.value = false;
    }
  });

  // --- 话题切换/消息清空时重置状态 ---
  watch(messageCount, (newCount) => {
    if (newCount === 0) {
      scrollScene.value = "initial";
      showScrollToBottom.value = false;
      loadAnchor = null;
      lastScrollHeight = 0;
      isInitialRendering.value = true;
    }
  });

  // --- 兼容旧 API（调用方 ChatView.vue 无需改动）---
  const reset = () => {
    loadRequestGeneration += 1;
    loadInFlight = false;
    scrollScene.value = "initial";
    showScrollToBottom.value = false;
    loadAnchor = null;
    lastScrollHeight = 0;
    isInitialRendering.value = true;
    if (scrollRafId) {
      cancelAnimationFrame(scrollRafId);
      scrollRafId = null;
    }
    if (scrollThrottleId) {
      cancelAnimationFrame(scrollThrottleId);
      scrollThrottleId = null;
    }
    if (loadMoreDebounceId) {
      clearTimeout(loadMoreDebounceId);
      loadMoreDebounceId = null;
    }
  };

  const startAutoScroll = () => {
    // 新架构中由 ResizeObserver 自动处理，此函数保留以保持调用方兼容
  };

  const stopAutoScroll = () => {
    // 新架构中不再需要显式停止，此函数保留以保持调用方兼容
  };

  const checkAndLoadMore = () => {
    triggerAutoLoadMoreWithDebounce();
  };

  const dispose = () => {
    loadRequestGeneration += 1;
    loadInFlight = false;
    stopContentObserver();
    if (scrollThrottleId) {
      cancelAnimationFrame(scrollThrottleId);
      scrollThrottleId = null;
    }
    if (loadMoreDebounceId) {
      clearTimeout(loadMoreDebounceId);
      loadMoreDebounceId = null;
    }
    if (messageListRef.value) {
      const list = messageListRef.value;
      list.removeEventListener("scroll", onScroll);
      list.removeEventListener("touchstart", onTouchStart);
      list.removeEventListener("touchmove", onTouchMove);
      list.removeEventListener("wheel", onWheel);
    }
    loadAnchor = null;
  };

  return {
    showScrollToBottom,
    scrollToBottom,
    startAutoScroll,
    stopAutoScroll,
    checkAndLoadMore,
    reset,
    dispose,
  };
}

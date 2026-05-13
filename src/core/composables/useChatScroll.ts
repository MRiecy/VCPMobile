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
  onLoadMore: () => void;
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
  const { messageListRef, messageCount, hasMoreHistory, isLoadingHistory, onLoadMore } = options;

  const showScrollToBottom = ref(false);
  const scrollScene = ref<ScrollScene>("initial");

  // 加载锚点：记录加载前视口中最顶部可见消息
  let loadAnchor: { messageId: string; offsetFromTop: number } | null = null;
  let scrollThrottleId: number | null = null;
  let mutationObserver: MutationObserver | null = null;
  let scrollRafId: number | null = null;

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
    const el = messageListRef.value.querySelector(`[data-message-id="${loadAnchor.messageId}"]`);
    if (el) {
      messageListRef.value.scrollTop = (el as HTMLElement).offsetTop - loadAnchor.offsetFromTop;
    }
    loadAnchor = null;
  };

  // --- 内容变化处理（等效于 ResizeObserver 的布局稳定信号）---
  const handleContentChange = () => {
    const list = messageListRef.value;
    if (!list) return;

    // 场景1：首屏加载完成（initial → following/free）
    if (scrollScene.value === "initial") {
      if (!isLoadingHistory.value) {
        if (messageCount.value > 0) {
          scrollToBottom(false);
          scrollScene.value = "following";
          // 首屏置底后，如果内容仍未撑满容器，自动续载
          if (
            list.scrollHeight <= list.clientHeight + 10 &&
            hasMoreHistory.value &&
            !isLoadingHistory.value
          ) {
            prepareLoadAnchor();
            scrollScene.value = "loading-top";
            onLoadMore();
          }
        } else {
          // 首屏无消息，切换到 free，允许用户上滑尝试加载
          scrollScene.value = "free";
        }
      }
      return;
    }

    // 场景2：分页加载完成（loading-top → free/following）
    if (scrollScene.value === "loading-top" && !isLoadingHistory.value) {
      if (loadAnchor) {
        restoreScrollByAnchor();
        scrollScene.value = "free";
      } else {
        // 无锚点 = 自动续载，直接置底回到 following
        scrollToBottom(false);
        scrollScene.value = "following";
      }
      return;
    }

    // 场景3：跟随模式下的新内容/流式追加
    if (scrollScene.value === "following" && !showScrollToBottom.value) {
      scrollToBottom(false);
      return;
    }

    // 场景4：内容不足自动续载（执行到此处时 scrollScene 已不可能是 initial）
    if (
      list.scrollHeight <= list.clientHeight + 10 &&
      hasMoreHistory.value &&
      !isLoadingHistory.value
    ) {
      prepareLoadAnchor();
      scrollScene.value = "loading-top";
      onLoadMore();
    }
  };

  // --- MutationObserver：监听 DOM 变化，双重 RAF 确保布局稳定后处理 ---
  const startContentObserver = () => {
    if (mutationObserver || !messageListRef.value) return;

    mutationObserver = new MutationObserver(() => {
      // RAF 节流：合并同一帧内的所有变化
      if (scrollRafId) cancelAnimationFrame(scrollRafId);
      scrollRafId = requestAnimationFrame(() => {
        scrollRafId = null;
        // 第二次 RAF：确保浏览器布局计算已完成
        requestAnimationFrame(() => {
          handleContentChange();
        });
      });
    });

    mutationObserver.observe(messageListRef.value, {
      childList: true,
      subtree: true,
      characterData: true,
    });
  };

  const stopContentObserver = () => {
    if (mutationObserver) {
      mutationObserver.disconnect();
      mutationObserver = null;
    }
    if (scrollRafId) {
      cancelAnimationFrame(scrollRafId);
      scrollRafId = null;
    }
  };

  // --- scroll 事件 ---
  const onScroll = () => {
    if (scrollThrottleId) return; // 已调度，节流中
    scrollThrottleId = requestAnimationFrame(() => {
      scrollThrottleId = null;
      const list = messageListRef.value;
      if (!list) return;

      const nearTop = list.scrollTop < 100;
      const nearBottom = list.scrollHeight - list.scrollTop - list.clientHeight < 150;

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
        prepareLoadAnchor();
        scrollScene.value = "loading-top";
        onLoadMore();
      }
    });
  };

  // --- 监听 messageListRef 变化，自动设置/清理事件与 Observer ---
  const stopWatchListRef = watch(messageListRef, (el, oldEl) => {
    if (oldEl) {
      oldEl.removeEventListener("scroll", onScroll);
    }
    if (el) {
      startContentObserver();
      el.addEventListener("scroll", onScroll, { passive: true });
      stopWatchListRef();
    }
  });

  // --- 监听 isLoadingHistory：兜底恢复（应对 MutationObserver 未及时触发的情况）---
  watch(isLoadingHistory, async (loading) => {
    if (loading) return;

    // 首屏加载完成兜底 或 分页加载完成兜底
    if (scrollScene.value === "initial" || (scrollScene.value === "loading-top" && loadAnchor)) {
      await nextTick();
      requestAnimationFrame(() => {
        requestAnimationFrame(() => {
          handleContentChange();
        });
      });
    }
  });

  // --- 话题切换/消息清空时重置状态 ---
  watch(messageCount, (newCount) => {
    if (newCount === 0) {
      scrollScene.value = "initial";
      showScrollToBottom.value = false;
      loadAnchor = null;
    }
  });

  // --- 兼容旧 API（调用方 ChatView.vue 无需改动）---
  const reset = () => {
    scrollScene.value = "initial";
    showScrollToBottom.value = false;
    loadAnchor = null;
    if (scrollRafId) {
      cancelAnimationFrame(scrollRafId);
      scrollRafId = null;
    }
  };

  const startAutoScroll = () => {
    // 新架构中由 MutationObserver 自动处理，此函数保留以保持调用方兼容
  };

  const stopAutoScroll = () => {
    // 新架构中不再需要显式停止，此函数保留以保持调用方兼容
  };

  const checkAndLoadMore = () => {
    const list = messageListRef.value;
    if (!list) return;
    if (
      scrollScene.value !== "initial" &&
      list.scrollHeight <= list.clientHeight + 10 &&
      hasMoreHistory.value &&
      !isLoadingHistory.value
    ) {
      prepareLoadAnchor();
      scrollScene.value = "loading-top";
      onLoadMore();
    }
  };

  const dispose = () => {
    stopContentObserver();
    if (scrollThrottleId) {
      cancelAnimationFrame(scrollThrottleId);
      scrollThrottleId = null;
    }
    if (messageListRef.value) {
      messageListRef.value.removeEventListener("scroll", onScroll);
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

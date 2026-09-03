import { computed, nextTick, ref } from "vue";
import { describe, expect, it, vi } from "vitest";
import { useChatScroll } from "@/core/composables/useChatScroll";

const flushFrames = async () => {
  await new Promise((resolve) => setTimeout(resolve, 5));
  await nextTick();
};

const settleInitialRendering = async () => {
  await new Promise((resolve) => setTimeout(resolve, 210));
  await nextTick();
};

function installControllableResizeObserver() {
  const original = window.ResizeObserver;
  let callback: ResizeObserverCallback | null = null;
  let observer: ResizeObserver | null = null;

  class TestResizeObserver implements ResizeObserver {
    constructor(nextCallback: ResizeObserverCallback) {
      callback = nextCallback;
      observer = this;
    }

    observe() {}
    unobserve() {}
    disconnect() {}
  }

  window.ResizeObserver = TestResizeObserver;

  return {
    trigger: () => {
      if (!callback || !observer) {
        throw new Error("ResizeObserver has not been initialized");
      }
      callback([], observer);
    },
    restore: () => {
      window.ResizeObserver = original;
    },
  };
}

function createTouchEvent(type: string, pageY?: number): TouchEvent {
  const event = new Event(type, { bubbles: true });
  Object.defineProperty(event, "touches", {
    configurable: true,
    value: pageY === undefined ? [] : [{ pageY }],
  });
  return event as TouchEvent;
}

function scrollTopArgument(
  options?: ScrollToOptions | number,
  y?: number,
): number {
  return typeof options === "number"
    ? Number(y || 0)
    : Number(options?.top || 0);
}

describe("useChatScroll pagination completion", () => {
  it("leaves loading-top after a zero-result page and permits retry", async () => {
    const list = document.createElement("div");
    Object.defineProperties(list, {
      scrollHeight: { configurable: true, value: 1000 },
      clientHeight: { configurable: true, value: 100 },
      scrollTop: { configurable: true, writable: true, value: 0 },
    });
    list.scrollTo = vi.fn();

    const messageListRef = ref<HTMLElement | null>(null);
    const count = ref(0);
    const hasMoreHistory = ref(true);
    const isLoadingHistory = ref(true);
    const onLoadMore = vi.fn(async () => ({ addedCount: 0 }));
    const scroll = useChatScroll({
      messageListRef,
      messageCount: computed(() => count.value),
      hasMoreHistory,
      isLoadingHistory,
      onLoadMore,
    });

    messageListRef.value = list;
    await nextTick();
    isLoadingHistory.value = false;
    await nextTick();
    list.dispatchEvent(new Event("scroll"));
    await flushFrames();
    expect(onLoadMore).toHaveBeenCalledTimes(1);

    list.dispatchEvent(new WheelEvent("wheel", { deltaY: -1 }));
    await flushFrames();
    expect(onLoadMore).toHaveBeenCalledTimes(2);
    scroll.dispose();
  });
});

function createLayoutScroll(list: HTMLElement) {
  const messageListRef = ref<HTMLElement | null>(null);
  const count = ref(1);
  const scroll = useChatScroll({
    messageListRef,
    messageCount: computed(() => count.value),
    hasMoreHistory: ref(false),
    isLoadingHistory: ref(false),
    onLoadMore: vi.fn(async () => ({ addedCount: 0 })),
  });
  messageListRef.value = list;
  return scroll;
}

describe("useChatScroll streaming follow intent", () => {
  it("keeps following when a delayed programmatic scroll event sees another large content batch", async () => {
    const resizeObserver = installControllableResizeObserver();
    const list = document.createElement("div");
    const inner = document.createElement("div");
    inner.className = "messages-inner-container";
    list.appendChild(inner);
    let scrollHeight = 1000;
    const clientHeight = 400;
    Object.defineProperties(list, {
      scrollHeight: { configurable: true, get: () => scrollHeight },
      clientHeight: { configurable: true, get: () => clientHeight },
      scrollTop: { configurable: true, writable: true, value: 0 },
    });
    list.scrollTo = vi.fn((options?: ScrollToOptions | number, y?: number) => {
      list.scrollTop = Math.min(
        scrollTopArgument(options, y),
        Math.max(0, scrollHeight - clientHeight),
      );
    });
    const scroll = createLayoutScroll(list);

    try {
      await nextTick();
      resizeObserver.trigger();
      await flushFrames();
      expect(list.scrollTop).toBe(600);
      await settleInitialRendering();

      scrollHeight = 1200;
      resizeObserver.trigger();
      expect(list.scrollTop).toBe(800);

      // 上一次程序置底的 scroll 事件抵达前，下一批内容又增长了 200px。
      scrollHeight = 1400;
      list.dispatchEvent(new Event("scroll"));
      await flushFrames();
      expect(scroll.showScrollToBottom.value).toBe(false);

      resizeObserver.trigger();
      expect(list.scrollTop).toBe(1000);
      expect(scroll.showScrollToBottom.value).toBe(false);
    } finally {
      scroll.dispose();
      resizeObserver.restore();
    }
  });

  it("pauses following for a user drag toward history and resumes at the bottom", async () => {
    const resizeObserver = installControllableResizeObserver();
    const list = document.createElement("div");
    const inner = document.createElement("div");
    inner.className = "messages-inner-container";
    list.appendChild(inner);
    let scrollHeight = 1000;
    const clientHeight = 400;
    Object.defineProperties(list, {
      scrollHeight: { configurable: true, get: () => scrollHeight },
      clientHeight: { configurable: true, get: () => clientHeight },
      scrollTop: { configurable: true, writable: true, value: 0 },
    });
    const scrollTo = vi.fn((options?: ScrollToOptions | number, y?: number) => {
      list.scrollTop = Math.min(
        scrollTopArgument(options, y),
        Math.max(0, scrollHeight - clientHeight),
      );
    });
    list.scrollTo = scrollTo;
    const scroll = createLayoutScroll(list);

    try {
      await nextTick();
      resizeObserver.trigger();
      await settleInitialRendering();
      scrollTo.mockClear();

      list.dispatchEvent(createTouchEvent("touchstart", 100));
      list.dispatchEvent(createTouchEvent("touchmove", 120));
      list.scrollTop = 300;
      list.dispatchEvent(new Event("scroll"));
      list.dispatchEvent(createTouchEvent("touchend"));
      await flushFrames();
      expect(scroll.showScrollToBottom.value).toBe(true);

      scrollHeight = 1200;
      resizeObserver.trigger();
      await flushFrames();
      expect(scrollTo).not.toHaveBeenCalled();

      list.scrollTop = 800;
      list.dispatchEvent(new Event("scroll"));
      await flushFrames();
      expect(scroll.showScrollToBottom.value).toBe(false);

      scrollHeight = 1300;
      resizeObserver.trigger();
      expect(list.scrollTop).toBe(900);
      expect(scrollTo).toHaveBeenCalledTimes(1);
    } finally {
      scroll.dispose();
      resizeObserver.restore();
    }
  });

  it("re-applies bottom following when the scroll viewport height changes", async () => {
    const resizeObserver = installControllableResizeObserver();
    const list = document.createElement("div");
    const inner = document.createElement("div");
    inner.className = "messages-inner-container";
    list.appendChild(inner);
    const scrollHeight = 1000;
    let clientHeight = 400;
    Object.defineProperties(list, {
      scrollHeight: { configurable: true, get: () => scrollHeight },
      clientHeight: { configurable: true, get: () => clientHeight },
      scrollTop: { configurable: true, writable: true, value: 0 },
    });
    const scrollTo = vi.fn((options?: ScrollToOptions | number, y?: number) => {
      list.scrollTop = Math.min(
        scrollTopArgument(options, y),
        Math.max(0, scrollHeight - clientHeight),
      );
    });
    list.scrollTo = scrollTo;
    const scroll = createLayoutScroll(list);

    try {
      await nextTick();
      resizeObserver.trigger();
      await flushFrames();
      expect(list.scrollTop).toBe(600);
      scrollTo.mockClear();

      clientHeight = 300;
      resizeObserver.trigger();
      expect(list.scrollTop).toBe(700);
      expect(scrollTo).toHaveBeenCalledTimes(1);
    } finally {
      scroll.dispose();
      resizeObserver.restore();
    }
  });
});

describe("useChatScroll layout-change anchoring", () => {
  it("keeps a near-bottom reader attached to the new bottom", async () => {
    const list = document.createElement("div");
    let scrollHeight = 1000;
    Object.defineProperties(list, {
      scrollHeight: { configurable: true, get: () => scrollHeight },
      clientHeight: { configurable: true, value: 100 },
      scrollTop: { configurable: true, writable: true, value: 850 },
    });
    list.scrollTo = vi.fn();
    const scroll = createLayoutScroll(list);
    await nextTick();

    await scroll.preserveViewportAcrossLayoutChange(() => {
      scrollHeight = 1400;
    });

    expect(list.scrollTo).toHaveBeenCalledWith({ top: 1400, behavior: "auto" });
    expect(scroll.showScrollToBottom.value).toBe(false);
    scroll.dispose();
  });

  it("restores the same partially visible message and its negative viewport offset", async () => {
    const list = document.createElement("div");
    const message = document.createElement("article");
    message.dataset.messageId = "message-7";
    list.appendChild(message);
    let messageTop = 70;
    Object.defineProperties(list, {
      scrollHeight: { configurable: true, value: 2000 },
      clientHeight: { configurable: true, value: 400 },
      scrollTop: { configurable: true, writable: true, value: 300 },
    });
    list.getBoundingClientRect = () => ({
      top: 100,
      bottom: 500,
      left: 0,
      right: 400,
      width: 400,
      height: 400,
      x: 0,
      y: 100,
      toJSON: () => ({}),
    });
    message.getBoundingClientRect = () => ({
      top: messageTop,
      bottom: messageTop + 180,
      left: 0,
      right: 400,
      width: 400,
      height: 180,
      x: 0,
      y: messageTop,
      toJSON: () => ({}),
    });
    list.scrollTo = vi.fn();
    const scroll = createLayoutScroll(list);
    await nextTick();

    await scroll.preserveViewportAcrossLayoutChange(() => {
      messageTop = 20;
    });

    expect(list.scrollTop).toBe(250);
    expect(scroll.showScrollToBottom.value).toBe(true);
    scroll.dispose();
  });

  it("falls back to the bounded previous scrollTop when the anchor disappears", async () => {
    const list = document.createElement("div");
    const message = document.createElement("article");
    message.dataset.messageId = "message-8";
    list.appendChild(message);
    Object.defineProperties(list, {
      scrollHeight: { configurable: true, value: 1000 },
      clientHeight: { configurable: true, value: 300 },
      scrollTop: { configurable: true, writable: true, value: 240 },
    });
    list.getBoundingClientRect = () => ({
      top: 0,
      bottom: 300,
      left: 0,
      right: 400,
      width: 400,
      height: 300,
      x: 0,
      y: 0,
      toJSON: () => ({}),
    });
    message.getBoundingClientRect = () => ({
      top: -20,
      bottom: 160,
      left: 0,
      right: 400,
      width: 400,
      height: 180,
      x: 0,
      y: -20,
      toJSON: () => ({}),
    });
    list.scrollTo = vi.fn();
    const scroll = createLayoutScroll(list);
    await nextTick();

    await scroll.preserveViewportAcrossLayoutChange(() => {
      message.remove();
      list.scrollTop = 0;
    });

    expect(list.scrollTop).toBe(240);
    scroll.dispose();
  });

  it("lets only the latest overlapping layout transaction restore the viewport", async () => {
    const list = document.createElement("div");
    const message = document.createElement("article");
    message.dataset.messageId = "message-latest";
    list.appendChild(message);
    let messageTop = 80;
    Object.defineProperties(list, {
      scrollHeight: { configurable: true, value: 1800 },
      clientHeight: { configurable: true, value: 400 },
      scrollTop: { configurable: true, writable: true, value: 300 },
    });
    list.getBoundingClientRect = () => ({
      top: 100,
      bottom: 500,
      left: 0,
      right: 400,
      width: 400,
      height: 400,
      x: 0,
      y: 100,
      toJSON: () => ({}),
    });
    message.getBoundingClientRect = () => ({
      top: messageTop,
      bottom: messageTop + 180,
      left: 0,
      right: 400,
      width: 400,
      height: 180,
      x: 0,
      y: messageTop,
      toJSON: () => ({}),
    });
    list.scrollTo = vi.fn();
    const scroll = createLayoutScroll(list);
    await nextTick();

    let releaseFirst: (() => void) | undefined;
    const first = scroll.preserveViewportAcrossLayoutChange(
      () => new Promise<void>((resolve) => { releaseFirst = resolve; }),
    );
    const second = scroll.preserveViewportAcrossLayoutChange(() => {
      messageTop = 30;
    });

    await second;
    expect(list.scrollTop).toBe(250);

    releaseFirst?.();
    await first;
    expect(list.scrollTop).toBe(250);
    scroll.dispose();
  });

  it("does not schedule layout frames after disposal invalidates a pending transaction", async () => {
    const list = document.createElement("div");
    Object.defineProperties(list, {
      scrollHeight: { configurable: true, value: 1000 },
      clientHeight: { configurable: true, value: 300 },
      scrollTop: { configurable: true, writable: true, value: 200 },
    });
    list.scrollTo = vi.fn();
    const scroll = createLayoutScroll(list);
    await nextTick();

    const animationFrame = vi.mocked(requestAnimationFrame);
    animationFrame.mockClear();
    let release: (() => void) | undefined;
    const transaction = scroll.preserveViewportAcrossLayoutChange(
      () => new Promise<void>((resolve) => { release = resolve; }),
    );

    scroll.dispose();
    release?.();
    await transaction;

    expect(animationFrame).not.toHaveBeenCalled();
  });
});

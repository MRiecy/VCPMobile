import { computed, nextTick, ref } from "vue";
import { describe, expect, it, vi } from "vitest";
import { useChatScroll } from "@/core/composables/useChatScroll";

const flushFrames = async () => {
  await new Promise((resolve) => setTimeout(resolve, 5));
  await nextTick();
};

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

    list.dispatchEvent(new Event("scroll"));
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

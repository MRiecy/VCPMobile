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

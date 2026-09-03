import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createStreamRevealController } from "@/features/chat/streamRevealScheduler";

function installManualAnimationFrame() {
  const originalRequest = window.requestAnimationFrame;
  const originalCancel = window.cancelAnimationFrame;
  const callbacks = new Map<number, FrameRequestCallback>();
  let nextId = 1;

  window.requestAnimationFrame = vi.fn((callback: FrameRequestCallback) => {
    const id = nextId++;
    callbacks.set(id, callback);
    return id;
  });
  window.cancelAnimationFrame = vi.fn((id: number) => {
    callbacks.delete(id);
  });

  return {
    flush() {
      const pending = [...callbacks.values()];
      callbacks.clear();
      pending.forEach((callback) => callback(performance.now()));
    },
    pendingCount: () => callbacks.size,
    restore() {
      window.requestAnimationFrame = originalRequest;
      window.cancelAnimationFrame = originalCancel;
    },
  };
}

describe("stream reveal scheduler", () => {
  let animationFrame: ReturnType<typeof installManualAnimationFrame>;

  beforeEach(() => {
    vi.useFakeTimers();
    animationFrame = installManualAnimationFrame();
  });

  afterEach(() => {
    animationFrame.restore();
    vi.useRealTimers();
  });

  it("reveals text on demand and stops scheduling after the debt drains", () => {
    const applied: string[] = [];
    const completed: number[] = [];
    const controller = createStreamRevealController<number>({
      apply: (_targetId, text) => {
        applied.push(text);
        return true;
      },
      complete: (frame) => completed.push(frame),
      fail: vi.fn(),
    });

    controller.enqueue({ targetId: "t0.i0", text: "abcdefgh", metadata: 7 });
    expect(applied.join("")).toBe("abc");
    expect(controller.hasPending).toBe(true);
    expect(animationFrame.pendingCount()).toBe(0);

    for (let index = 0; index < 8 && controller.hasPending; index += 1) {
      vi.advanceTimersByTime(34);
      expect(animationFrame.pendingCount()).toBe(1);
      animationFrame.flush();
    }

    expect(applied.join("")).toBe("abcdefgh");
    expect(completed).toEqual([7]);
    expect(controller.hasPending).toBe(false);

    const scheduledFrames = vi.mocked(window.requestAnimationFrame).mock.calls
      .length;
    vi.advanceTimersByTime(1000);
    animationFrame.flush();
    expect(vi.mocked(window.requestAnimationFrame)).toHaveBeenCalledTimes(
      scheduledFrames,
    );
    controller.dispose();
  });

  it("uses one shared frame to advance multiple message controllers", () => {
    const first: string[] = [];
    const second: string[] = [];
    const createController = (output: string[]) =>
      createStreamRevealController<number>({
        apply: (_targetId, text) => {
          output.push(text);
          return true;
        },
        complete: vi.fn(),
        fail: vi.fn(),
      });
    const firstController = createController(first);
    const secondController = createController(second);

    firstController.enqueue({
      targetId: "first",
      text: "abcdefgh",
      metadata: 1,
    });
    secondController.enqueue({
      targetId: "second",
      text: "12345678",
      metadata: 2,
    });
    vi.advanceTimersByTime(34);

    expect(animationFrame.pendingCount()).toBe(1);
    animationFrame.flush();
    expect(first.join("").length).toBeGreaterThan(3);
    expect(second.join("").length).toBeGreaterThan(3);

    firstController.dispose();
    secondController.dispose();
  });

  it("keeps grapheme clusters intact and supports an immediate barrier flush", () => {
    const segmenterDescriptor = Object.getOwnPropertyDescriptor(
      Intl,
      "Segmenter",
    );
    const applied: string[] = [];
    const controller = createStreamRevealController<string>({
      apply: (_targetId, text) => {
        applied.push(text);
        return true;
      },
      complete: vi.fn(),
      fail: vi.fn(),
    });
    const content = "A👨‍👩‍👧‍👦é🇨🇳Z";

    try {
      Object.defineProperty(Intl, "Segmenter", {
        configurable: true,
        value: undefined,
      });
      controller.enqueue({
        targetId: "unicode",
        text: content,
        metadata: "unicode",
      });
      expect(controller.flush()).toBe(true);

      expect(applied.join("")).toBe(content);
      expect(applied.some((part) => part.includes("A"))).toBe(true);
      expect(applied.some((part) => part.includes("👨‍👩‍👧‍👦"))).toBe(true);
      expect(applied.some((part) => part.includes("é"))).toBe(true);
      expect(applied.some((part) => part.includes("🇨🇳"))).toBe(true);
      expect(controller.hasPending).toBe(false);
    } finally {
      controller.dispose();
      if (segmenterDescriptor) {
        Object.defineProperty(Intl, "Segmenter", segmenterDescriptor);
      }
    }
  });

  it("flushes accepted text and cancels scheduled work when the page hides", () => {
    const hiddenDescriptor = Object.getOwnPropertyDescriptor(
      document,
      "hidden",
    );
    const applied: string[] = [];
    const controller = createStreamRevealController<number>({
      apply: (_targetId, text) => {
        applied.push(text);
        return true;
      },
      complete: vi.fn(),
      fail: vi.fn(),
    });

    try {
      controller.enqueue({ targetId: "hidden", text: "abcdefgh", metadata: 1 });
      expect(controller.hasPending).toBe(true);
      Object.defineProperty(document, "hidden", {
        configurable: true,
        value: true,
      });
      document.dispatchEvent(new Event("visibilitychange"));

      expect(applied.join("")).toBe("abcdefgh");
      expect(controller.hasPending).toBe(false);
      vi.advanceTimersByTime(1000);
      expect(animationFrame.pendingCount()).toBe(0);
    } finally {
      controller.dispose();
      if (hiddenDescriptor) {
        Object.defineProperty(document, "hidden", hiddenDescriptor);
      }
    }
  });

  it("caps visible debt immediately and fast-forwards after hard lag", () => {
    const applied: string[] = [];
    const controller = createStreamRevealController<number>({
      apply: (_targetId, text) => {
        applied.push(text);
        return true;
      },
      complete: vi.fn(),
      fail: vi.fn(),
    });

    const first = "a".repeat(200);
    const second = "b".repeat(200);
    controller.enqueue({ targetId: "debt", text: first, metadata: 1 });
    expect(controller.hasPending).toBe(true);
    controller.enqueue({ targetId: "debt", text: second, metadata: 2 });
    expect((first + second).length - applied.join("").length).toBe(64);
    expect(controller.hasPending).toBe(true);
    expect(controller.flush()).toBe(true);
    expect(applied.join("")).toBe(first + second);

    const lagged = "c".repeat(60);
    controller.enqueue({ targetId: "lag", text: lagged, metadata: 3 });
    expect(controller.hasPending).toBe(true);
    vi.advanceTimersByTime(201);
    animationFrame.flush();
    expect(applied.join("")).toBe(first + second + lagged);
    expect(controller.hasPending).toBe(false);
    controller.dispose();
  });

  it("drops the remaining debt and reports a failed append target once", () => {
    const fail = vi.fn();
    const controller = createStreamRevealController<number>({
      apply: () => false,
      complete: vi.fn(),
      fail,
    });

    controller.enqueue({ targetId: "missing", text: "failure", metadata: 9 });

    expect(fail).toHaveBeenCalledWith(9, "append target is unavailable");
    expect(controller.hasPending).toBe(false);
    vi.advanceTimersByTime(1000);
    expect(animationFrame.pendingCount()).toBe(0);
    controller.dispose();
  });
});

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  applyFrame,
  cleanupRegistry,
  rebuildSnapshot,
} from "@/core/utils/astExecutor";
import {
  discardStreamTextFragments,
  flushStreamTextFragments,
  resolveStreamTextFade,
} from "@/core/utils/streamTextFade";

const mountedSandboxes: HTMLElement[] = [];
const animations = new WeakMap<Element, Animation>();
let originalAnimate: PropertyDescriptor | undefined;

function finishFade(element: Element): void {
  const animation = animations.get(element);
  expect(animation).toBeDefined();
  Object.defineProperty(animation, "playState", { configurable: true, value: "finished" });
  animation?.onfinish?.call(animation, new Event("finish") as AnimationPlaybackEvent);
}

function createTextSandbox(messageId: string, value = "base"): HTMLElement {
  const sandbox = document.createElement("div");
  document.body.appendChild(sandbox);
  mountedSandboxes.push(sandbox);
  rebuildSnapshot([{
    type: "paragraph",
    children: [{ type: "text", value }],
  }], messageId, sandbox);
  return sandbox;
}

beforeEach(() => {
  originalAnimate = Object.getOwnPropertyDescriptor(Element.prototype, "animate");
  Object.defineProperty(Element.prototype, "animate", {
    configurable: true,
    value: vi.fn(function (this: Element) {
      const animation = {
        playState: "running",
        onfinish: null,
        oncancel: null,
        cancel(this: { playState: string; oncancel: ((event: Event) => void) | null }) {
          Object.defineProperty(this, "playState", { configurable: true, value: "idle" });
          this.oncancel?.(new Event("cancel"));
        },
      } as unknown as Animation;
      animations.set(this, animation);
      return animation;
    }),
  });
});

afterEach(() => {
  if (originalAnimate) {
    Object.defineProperty(Element.prototype, "animate", originalAnimate);
  } else {
    Reflect.deleteProperty(Element.prototype, "animate");
  }
  for (const sandbox of mountedSandboxes.splice(0)) sandbox.remove();
});

describe("stream text fade", () => {
  it.each([
    [1, 110, 0.8],
    [4, 93, 0.8466666666666667],
    [16, 77, 0.8933333333333334],
    [64, 60, 0.9400000000000001],
    [512, 60, 0.9400000000000001],
  ])("maps %i code units to one frame-local fade", (units, duration, opacity) => {
    expect(resolveStreamTextFade(units)).toEqual({
      durationMs: duration,
      fromOpacity: opacity,
    });
  });

  it("puts complete chunks in the DOM immediately and drains out-of-order finishes in text order", () => {
    const messageId = "ordered-inline-fade";
    const sandbox = createTextSandbox(messageId);

    try {
      expect(applyFrame([
        { op: "append", id: "t0.i0", chunk: "中文" },
        { op: "append", id: "t0.i0", chunk: "👨‍👩‍👧‍👦é" },
        { op: "append", id: "t0.i0", chunk: " مرحبا https://example.com/a-b" },
      ], messageId, sandbox, { smoothStreaming: true }).ok).toBe(true);

      const fragments = sandbox.querySelectorAll<HTMLElement>("[data-vcp-stream-fragment]");
      expect(fragments).toHaveLength(3);
      expect(sandbox.textContent).toBe("base中文👨‍👩‍👧‍👦é مرحبا https://example.com/a-b");
      expect(fragments[0].style.getPropertyValue("--vcp-stream-fade-duration"))
        .toBe(fragments[1].style.getPropertyValue("--vcp-stream-fade-duration"));

      finishFade(fragments[2]);
      finishFade(fragments[1]);
      expect(sandbox.querySelectorAll("[data-vcp-stream-fragment]")).toHaveLength(3);
      finishFade(fragments[0]);

      expect(sandbox.querySelector("[data-vcp-stream-fragment]")).toBeNull();
      expect(sandbox.textContent).toBe("base中文👨‍👩‍👧‍👦é مرحبا https://example.com/a-b");
      expect(applyFrame([
        { op: "append", id: "t0.i0", chunk: "!" },
      ], messageId, sandbox).ok).toBe(true);
      expect(sandbox.textContent).toBe("base中文👨‍👩‍👧‍👦é مرحبا https://example.com/a-b!");
    } finally {
      cleanupRegistry(messageId);
    }
  });

  it("uses structural frames as barriers and only fades new block roots", () => {
    const messageId = "structural-barrier";
    const sandbox = createTextSandbox(messageId);

    try {
      applyFrame([
        { op: "append", id: "t0.i0", chunk: "A" },
      ], messageId, sandbox, { smoothStreaming: true });
      expect(sandbox.querySelector("[data-vcp-stream-fragment]")).not.toBeNull();

      expect(applyFrame([
        { op: "append", id: "t0.i0", chunk: "B" },
        {
          op: "add",
          id: "t1",
          parent: "root",
          node: { type: "paragraph", children: [{ type: "text", value: "block" }] },
        },
      ], messageId, sandbox, { smoothStreaming: true }).ok).toBe(true);

      expect(sandbox.textContent).toBe("baseABblock");
      expect(sandbox.querySelector("[data-vcp-stream-fragment]")).toBeNull();
      expect(sandbox.querySelectorAll(".vcp-stream-element-fade-in")).toHaveLength(1);

      const newBlock = sandbox.querySelectorAll("p")[1];
      applyFrame([
        { op: "append", id: "t1.i0", chunk: " nested" },
      ], messageId, sandbox, { smoothStreaming: true });
      expect(sandbox.querySelector("[data-vcp-stream-fragment]")).toBeNull();
      newBlock.dispatchEvent(new Event("animationend"));
      applyFrame([
        { op: "append", id: "t1.i0", chunk: " faded" },
      ], messageId, sandbox, { smoothStreaming: true });
      expect(sandbox.querySelector("[data-vcp-stream-fragment]")?.textContent).toBe(" faded");
      flushStreamTextFragments(messageId);

      applyFrame([{
        op: "add_inline",
        id: "t0.i1",
        parent: "t0",
        node: { type: "strong", children: [{ type: "text", value: "strong" }] },
      }], messageId, sandbox, { smoothStreaming: true });
      expect(sandbox.querySelector("strong")?.classList.contains("vcp-stream-element-fade-in"))
        .toBe(false);

      applyFrame([{
        op: "add",
        id: "t2",
        parent: "root",
        node: { type: "list", ordered: false, items: [] },
      }], messageId, sandbox, { smoothStreaming: true });
      applyFrame([{
        op: "add_list_item",
        id: "t2.l0",
        parent: "t2",
        children: [{
          type: "paragraph",
          children: [{ type: "text", value: "item" }],
        }],
      }], messageId, sandbox, { smoothStreaming: true });
      expect(sandbox.querySelector("li")?.classList.contains("vcp-stream-element-fade-in"))
        .toBe(false);

      applyFrame([{
        op: "replace",
        id: "t1",
        node: { type: "heading", level: 2, children: [{ type: "text", value: "heading" }] },
      }], messageId, sandbox, { smoothStreaming: true });
      expect(sandbox.querySelector("h2")?.classList.contains("vcp-stream-element-fade-in"))
        .toBe(false);
    } finally {
      cleanupRegistry(messageId);
    }
  });

  it("commits on flush and drops stale fragments when canonical content takes over", () => {
    const flushId = "flush-inline-fade";
    const flushSandbox = createTextSandbox(flushId);
    applyFrame([
      { op: "append", id: "t0.i0", chunk: " kept" },
    ], flushId, flushSandbox, { smoothStreaming: true });
    flushStreamTextFragments(flushId);
    expect(flushSandbox.textContent).toBe("base kept");
    expect(flushSandbox.querySelector("[data-vcp-stream-fragment]")).toBeNull();
    cleanupRegistry(flushId);

    const discardId = "discard-inline-fade";
    const discardSandbox = createTextSandbox(discardId);
    applyFrame([
      { op: "append", id: "t0.i0", chunk: " stale" },
    ], discardId, discardSandbox, { smoothStreaming: true });
    discardStreamTextFragments(discardId);
    expect(discardSandbox.textContent).toBe("base");
    cleanupRegistry(discardId);
  });

  it("writes directly to the canonical Text node when animations are unavailable", () => {
    Reflect.deleteProperty(Element.prototype, "animate");
    const messageId = "unsupported-animation";
    const sandbox = createTextSandbox(messageId);

    try {
      expect(applyFrame([
        { op: "append", id: "t0.i0", chunk: " immediate" },
      ], messageId, sandbox, { smoothStreaming: true }).ok).toBe(true);
      expect(sandbox.textContent).toBe("base immediate");
      expect(sandbox.querySelector("[data-vcp-stream-fragment]")).toBeNull();
    } finally {
      cleanupRegistry(messageId);
    }
  });

  it("commits active fragments when the document becomes hidden", () => {
    const hiddenDescriptor = Object.getOwnPropertyDescriptor(document, "hidden");
    const messageId = "hidden-document";
    const sandbox = createTextSandbox(messageId);

    try {
      applyFrame([
        { op: "append", id: "t0.i0", chunk: " hidden" },
      ], messageId, sandbox, { smoothStreaming: true });
      expect(sandbox.querySelector("[data-vcp-stream-fragment]")).not.toBeNull();

      Object.defineProperty(document, "hidden", { configurable: true, value: true });
      document.dispatchEvent(new Event("visibilitychange"));

      expect(sandbox.textContent).toBe("base hidden");
      expect(sandbox.querySelector("[data-vcp-stream-fragment]")).toBeNull();
    } finally {
      cleanupRegistry(messageId);
      if (hiddenDescriptor) {
        Object.defineProperty(document, "hidden", hiddenDescriptor);
      } else {
        Reflect.deleteProperty(document, "hidden");
      }
    }
  });
});

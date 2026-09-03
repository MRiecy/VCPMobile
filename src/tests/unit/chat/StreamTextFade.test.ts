import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  applyFrame,
  cleanupRegistry,
  rebuildSnapshot,
} from "@/core/utils/astExecutor";
import {
  discardStreamTextFragments,
  flushStreamTextFragments,
  STREAM_ELEMENT_REVEAL_CLASS,
  STREAM_INLINE_REVEAL_CLASS,
  STREAM_REVEAL_DURATION_MS,
  supportsStreamRevealMotion,
} from "@/core/utils/streamTextFade";

const mountedSandboxes: HTMLElement[] = [];
const animations = new WeakMap<Element, Animation>();
const animationInputs = new WeakMap<Element, { keyframes: Keyframe[]; options: KeyframeAnimationOptions }>();
let originalAnimate: PropertyDescriptor | undefined;
let originalCss: PropertyDescriptor | undefined;

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
  originalCss = Object.getOwnPropertyDescriptor(globalThis, "CSS");
  Object.defineProperty(globalThis, "CSS", {
    configurable: true,
    value: {
      supports: vi.fn(() => true),
      registerProperty: vi.fn(),
    },
  });
  Object.defineProperty(Element.prototype, "animate", {
    configurable: true,
    value: vi.fn(function (
      this: Element,
      keyframes: Keyframe[],
      options: KeyframeAnimationOptions,
    ) {
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
      animationInputs.set(this, { keyframes, options });
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
  if (originalCss) {
    Object.defineProperty(globalThis, "CSS", originalCss);
  } else {
    Reflect.deleteProperty(globalThis, "CSS");
  }
  for (const sandbox of mountedSandboxes.splice(0)) sandbox.remove();
});

describe("stream text reveal", () => {
  it("uses one fixed horizontal reveal contract", () => {
    expect(STREAM_REVEAL_DURATION_MS).toBe(250);
    expect({
      animate: typeof Element.prototype.animate,
      supports: typeof CSS.supports,
      registerProperty: typeof CSS.registerProperty,
      result: supportsStreamRevealMotion(),
    }).toEqual({
      animate: "function",
      supports: "function",
      registerProperty: "function",
      result: true,
    });
  });

  it("merges same-frame appends into one reveal surface with fixed mask keyframes", () => {
    const messageId = "ordered-inline-fade";
    const sandbox = createTextSandbox(messageId);

    try {
      expect(applyFrame([
        { op: "append", id: "t0.i0", chunk: "中文" },
        { op: "append", id: "t0.i0", chunk: "👨‍👩‍👧‍👦é" },
        { op: "append", id: "t0.i0", chunk: " مرحبا https://example.com/a-b" },
      ], messageId, sandbox, { smoothStreaming: true }).ok).toBe(true);

      const fragments = sandbox.querySelectorAll<HTMLElement>("[data-vcp-stream-fragment]");
      expect(fragments).toHaveLength(1);
      expect(sandbox.textContent).toBe("base中文👨‍👩‍👧‍👦é مرحبا https://example.com/a-b");
      expect(fragments[0].classList.contains(STREAM_INLINE_REVEAL_CLASS)).toBe(true);
      expect(animationInputs.get(fragments[0])).toEqual({
        keyframes: [
          { "--vcp-stream-reveal-progress": "0%" },
          { "--vcp-stream-reveal-progress": "calc(100% + 1.732em)" },
        ],
        options: { duration: 250, easing: "linear", fill: "both" },
      });
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

  it("drains independently arriving reveal surfaces in text order", () => {
    const messageId = "ordered-independent-reveals";
    const sandbox = createTextSandbox(messageId);

    try {
      for (const chunk of ["A", "B", "C"]) {
        applyFrame([{ op: "append", id: "t0.i0", chunk }], messageId, sandbox, {
          smoothStreaming: true,
        });
      }
      const fragments = sandbox.querySelectorAll<HTMLElement>("[data-vcp-stream-fragment]");
      expect(fragments).toHaveLength(3);
      finishFade(fragments[2]);
      finishFade(fragments[1]);
      expect(sandbox.querySelectorAll("[data-vcp-stream-fragment]")).toHaveLength(3);
      finishFade(fragments[0]);
      expect(sandbox.textContent).toBe("baseABC");
      expect(sandbox.querySelector("[data-vcp-stream-fragment]")).toBeNull();
    } finally {
      cleanupRegistry(messageId);
    }
  });

  it("uses structural frames as barriers and only reveals eligible new block roots", () => {
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
      expect(sandbox.querySelectorAll(`.${STREAM_ELEMENT_REVEAL_CLASS}`)).toHaveLength(1);

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
      expect(sandbox.querySelector("strong")?.classList.contains(STREAM_ELEMENT_REVEAL_CLASS))
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
      expect(sandbox.querySelector("li")?.classList.contains(STREAM_ELEMENT_REVEAL_CLASS))
        .toBe(true);

      applyFrame([{
        op: "replace",
        id: "t1",
        node: { type: "heading", level: 2, children: [{ type: "text", value: "heading" }] },
      }], messageId, sandbox, { smoothStreaming: true });
      expect(sandbox.querySelector("h2")?.classList.contains(STREAM_ELEMENT_REVEAL_CLASS))
        .toBe(false);
    } finally {
      cleanupRegistry(messageId);
    }
  });

  it("keeps code fences, thematic breaks, and raw HTML static", () => {
    const messageId = "excluded-block-reveals";
    const sandbox = createTextSandbox(messageId);

    try {
      expect(applyFrame([
        {
          op: "add",
          id: "t1",
          parent: "root",
          node: {
            type: "code_block",
            lang: "ts",
            code: "const value = 1;",
            highlighted_html: null,
            theme: null,
          },
        },
        { op: "add", id: "t2", parent: "root", node: { type: "thematic_break" } },
        {
          op: "add",
          id: "t3",
          parent: "root",
          node: { type: "raw_html", content: "<aside>raw</aside>" },
        },
      ], messageId, sandbox, { smoothStreaming: true }).ok).toBe(true);

      expect(sandbox.querySelectorAll(`.${STREAM_ELEMENT_REVEAL_CLASS}`)).toHaveLength(0);
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

  it("writes directly when CSS masks are unavailable", () => {
    vi.mocked(CSS.supports).mockReturnValue(false);
    const messageId = "unsupported-mask";
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

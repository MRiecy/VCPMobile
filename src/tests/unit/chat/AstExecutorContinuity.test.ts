import { describe, expect, it } from "vitest";
import {
  applyFrame,
  cleanupRegistry,
  rebuildSnapshot,
} from "@/core/utils/astExecutor";

describe("AST executor continuity", () => {
  it("keeps heading child registry references live after changing the level", () => {
    const sandbox = document.createElement("div");
    document.body.appendChild(sandbox);

    try {
      rebuildSnapshot([{
        type: "heading",
        level: 1,
        children: [{ type: "text", value: "A" }],
      }], "heading-message", sandbox);

      expect(applyFrame([{
        op: "prop",
        id: "t0",
        key: "level",
        value: "2",
      }], "heading-message", sandbox).ok).toBe(true);
      expect(applyFrame([{
        op: "append",
        id: "t0.i0",
        chunk: "B",
      }], "heading-message", sandbox).ok).toBe(true);

      expect(sandbox.querySelector("h1")).toBeNull();
      expect(sandbox.querySelector("h2")?.textContent).toBe("AB");
    } finally {
      cleanupRegistry("heading-message");
      sandbox.remove();
    }
  });

  it("appends completed highlighted lines while replacing only the active line", () => {
    const sandbox = document.createElement("div");
    document.body.appendChild(sandbox);

    try {
      rebuildSnapshot([{
        type: "code_block",
        lang: "html",
        code: "<div>one\n<span>two",
        highlighted_html: '<pre class="vcp-code-block"><code data-vcp-stream-code><span data-vcp-code-stable><span>one\n</span></span><span data-vcp-code-active><span>two</span></span></code></pre>',
        theme: null,
      }], "code-message", sandbox);

      expect(applyFrame([{
        op: "patch_code",
        id: "t0",
        completed_html: "<span>two complete\n</span>",
        active_html: "<span>三</span>",
      }], "code-message", sandbox).ok).toBe(true);
      expect(applyFrame([{
        op: "patch_code",
        id: "t0",
        completed_html: "",
        active_html: "<span>三四</span>",
      }], "code-message", sandbox).ok).toBe(true);

      const stable = sandbox.querySelector("[data-vcp-code-stable]");
      const active = sandbox.querySelector("[data-vcp-code-active]");
      expect(stable?.textContent).toBe("one\ntwo complete\n");
      expect(active?.textContent).toBe("三四");
    } finally {
      cleanupRegistry("code-message");
      sandbox.remove();
    }
  });

  it("morphs one raw HTML node without fragmenting its nested DOM", () => {
    const sandbox = document.createElement("div");
    document.body.appendChild(sandbox);

    try {
      rebuildSnapshot([{
        type: "raw_html",
        content: '<div class="card"><section><span>one</span></section></div>',
      }], "raw-html-message", sandbox);
      const rawRoot = sandbox.firstElementChild;

      expect(applyFrame([{
        op: "replace",
        id: "t0",
        node: {
          type: "raw_html",
          content: '<div class="card"><section><span>one two</span></section><p>tail</p></div>',
        },
      }], "raw-html-message", sandbox).ok).toBe(true);

      expect(sandbox.firstElementChild).toBe(rawRoot);
      expect(sandbox.querySelector(".card > section > span")?.textContent).toBe("one two");
      expect(sandbox.querySelector(".card > p")?.textContent).toBe("tail");
      expect(sandbox.querySelectorAll(".vcp-raw-html-container")).toHaveLength(1);
    } finally {
      cleanupRegistry("raw-html-message");
      sandbox.remove();
    }
  });

  it("freezes closed root children of a growing raw HTML tail and keeps their DOM identity", () => {
    const sandbox = document.createElement("div");
    document.body.appendChild(sandbox);

    try {
      rebuildSnapshot([{
        type: "raw_html",
        content: '<div class="a">A</div>',
      }], "freeze-message", sandbox);
      const container = sandbox.querySelector(".vcp-raw-html-container");

      const push = (content: string) =>
        applyFrame(
          [{ op: "replace", id: "t0", node: { type: "raw_html", content } }],
          "freeze-message",
          sandbox,
        ).ok;

      // 帧 1：b 未闭合（活跃区）；a 应在此帧建立冻结基线
      expect(push('<div class="a">A</div><div class="b">B')).toBe(true);
      const aEl = sandbox.querySelector(".a");

      // 帧 2：b 闭合，c 活跃；a 必须保持对象身份（未被重建）
      expect(push('<div class="a">A</div><div class="b">B</div><div class="c">C')).toBe(true);
      const bEl = sandbox.querySelector(".b");

      // 帧 3：c 内容变长后闭合，d 活跃
      expect(push('<div class="a">A</div><div class="b">B</div><div class="c">CC</div><div class="d">D')).toBe(true);

      expect(sandbox.querySelector(".a")).toBe(aEl);
      expect(sandbox.querySelector(".b")).toBe(bEl);
      expect(sandbox.querySelector(".c")?.textContent).toBe("CC");
      expect(sandbox.querySelector(".d")?.textContent).toBe("D");
      expect(container?.childNodes.length).toBe(4);
      expect(container?.textContent).toBe("ABCCD");
    } finally {
      cleanupRegistry("freeze-message");
      sandbox.remove();
    }
  });

  it("updates a trailing live text node in place without touching frozen siblings", () => {
    const sandbox = document.createElement("div");
    document.body.appendChild(sandbox);

    try {
      rebuildSnapshot([{
        type: "raw_html",
        content: '<div class="a">A</div>',
      }], "freeze-text-message", sandbox);

      const push = (content: string) =>
        applyFrame(
          [{ op: "replace", id: "t0", node: { type: "raw_html", content } }],
          "freeze-text-message",
          sandbox,
        ).ok;

      expect(push('<div class="a">A</div>xy')).toBe(true);
      const aEl = sandbox.querySelector(".a");

      expect(push('<div class="a">A</div>xyz')).toBe(true);

      const container = sandbox.querySelector(".vcp-raw-html-container");
      expect(sandbox.querySelector(".a")).toBe(aEl);
      expect(container?.childNodes.length).toBe(2);
      expect(container?.lastChild?.nodeType).toBe(Node.TEXT_NODE);
      expect(container?.lastChild?.nodeValue).toBe("xyz");
    } finally {
      cleanupRegistry("freeze-text-message");
      sandbox.remove();
    }
  });
});

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

  it("applies patch_raw_html seed then steady frozen/live advances with identity preserved", () => {
    const sandbox = document.createElement("div");
    document.body.appendChild(sandbox);

    try {
      // 种子帧：整棵外层下发，frozen_total=1 表示第一个子节点（section）已冻结
      expect(applyFrame([{
        op: "patch_raw_html",
        id: "t0",
        frozen_html: "",
        live_html: '<div class="card"><section>one</section><p>B</p></div>',
        frozen_total: 1,
        seed: true,
      }], "patch-msg", sandbox).ok).toBe(true);

      const card = sandbox.querySelector(".card");
      const sectionEl = sandbox.querySelector("section");
      expect(card).not.toBeNull();
      expect(sandbox.querySelector(".vcp-raw-html-container")).not.toBeNull();

      // 稳态帧：p 闭合冻结，span 活跃
      expect(applyFrame([{
        op: "patch_raw_html",
        id: "t0",
        frozen_html: "<p>B</p>",
        live_html: "<span>C</span>",
        frozen_total: 2,
        seed: false,
      }], "patch-msg", sandbox).ok).toBe(true);

      expect(sandbox.querySelector("section")).toBe(sectionEl); // 冻结节点对象身份不变
      expect(card?.childNodes.length).toBe(3); // section / p / span
      expect(card?.textContent).toBe("oneBC");

      // 稳态帧：span 闭合冻结，尾部文本活跃
      expect(applyFrame([{
        op: "patch_raw_html",
        id: "t0",
        frozen_html: "<span>C</span>",
        live_html: "tail",
        frozen_total: 3,
        seed: false,
      }], "patch-msg", sandbox).ok).toBe(true);

      expect(card?.childNodes.length).toBe(4);
      expect(card?.lastChild?.nodeType).toBe(Node.TEXT_NODE);
      expect(card?.lastChild?.nodeValue).toBe("tail");
      expect(card?.textContent).toBe("oneBCtail");
    } finally {
      cleanupRegistry("patch-msg");
      sandbox.remove();
    }
  });

  it("sanitizes patch_raw_html payloads and fails loudly on frontier desync", () => {
    const sandbox = document.createElement("div");
    document.body.appendChild(sandbox);

    try {
      expect(applyFrame([{
        op: "patch_raw_html",
        id: "t0",
        frozen_html: "",
        live_html: '<div class="card"><p>A</p></div>',
        frozen_total: 0,
        seed: true,
      }], "patch-sanitize", sandbox).ok).toBe(true);

      // live_html 中引用宿主能力的危险属性必须被剥除（与本项目的安全画像一致：
      // 良性 handler 保留，fetch/invoke 等宿主入口才剥除）
      expect(applyFrame([{
        op: "patch_raw_html",
        id: "t0",
        frozen_html: "",
        live_html: '<span onclick="fetch(\'/leak\')">ok</span>',
        frozen_total: 0,
        seed: false,
      }], "patch-sanitize", sandbox).ok).toBe(true);
      const liveSpan = sandbox.querySelector(".card > span");
      expect(liveSpan?.getAttribute("onclick")).toBeNull();
      expect(liveSpan?.textContent).toBe("ok");

      // 结构对不上（frozen_total 虚高）必须失败而非静默错位
      const bad = applyFrame([{
        op: "patch_raw_html",
        id: "t0",
        frozen_html: "",
        live_html: "<span>ok</span>",
        frozen_total: 99,
        seed: false,
      }], "patch-sanitize", sandbox);
      expect(bad.ok).toBe(false);
    } finally {
      cleanupRegistry("patch-sanitize");
      sandbox.remove();
    }
  });
});

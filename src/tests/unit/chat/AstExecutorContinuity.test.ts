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
});

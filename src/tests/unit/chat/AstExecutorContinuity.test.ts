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
});

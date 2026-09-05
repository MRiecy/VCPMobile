import { describe, it, expect, beforeEach } from "vitest";
import {
  getRegistry,
  getNode,
  setNode,
  deleteNode,
  cleanupRegistry,
  hasRegistry,
  getRegistrySize,
  clearAllRegistries,
  applyThoughtTailOp,
  cleanupThoughtTail,
  getThoughtTailState,
  createThoughtTailDriver,
  applyCodePatch,
  createCodePatchDriver,
  useDomRenderer,
} from "../../../core/dom-engine";

describe("DOM Engine Microkernel", () => {
  beforeEach(() => {
    clearAllRegistries();
    cleanupThoughtTail("msg-test-1");
    cleanupThoughtTail("msg-test-2");
    document.body.innerHTML = "";
  });

  describe("NodeRegistry", () => {
    it("should register, get, and delete nodes correctly", () => {
      const el = document.createElement("div");
      setNode("msg-1", "node-1", el);

      expect(hasRegistry("msg-1")).toBe(true);
      expect(getRegistry("msg-1")).toBeInstanceOf(Map);
      expect(getRegistrySize("msg-1")).toBe(1);
      expect(getNode("msg-1", "node-1")).toBe(el);

      const deleted = deleteNode("msg-1", "node-1");
      expect(deleted).toBe(true);
      expect(getNode("msg-1", "node-1")).toBeUndefined();
      expect(getRegistrySize("msg-1")).toBe(0);
    });

    it("should isolate registries across different messageIds", () => {
      const nodeA = document.createElement("span");
      const nodeB = document.createElement("p");

      setNode("msg-A", "same-id", nodeA);
      setNode("msg-B", "same-id", nodeB);

      expect(getNode("msg-A", "same-id")).toBe(nodeA);
      expect(getNode("msg-B", "same-id")).toBe(nodeB);

      const releasedA = cleanupRegistry("msg-A");
      expect(releasedA).toBe(1);
      expect(hasRegistry("msg-A")).toBe(false);
      expect(getNode("msg-A", "same-id")).toBeUndefined();
      expect(getNode("msg-B", "same-id")).toBe(nodeB);
    });

    it("should clear all registries on clearAllRegistries", () => {
      setNode("msg-1", "n1", document.createElement("div"));
      setNode("msg-2", "n2", document.createElement("div"));

      clearAllRegistries();
      expect(hasRegistry("msg-1")).toBe(false);
      expect(hasRegistry("msg-2")).toBe(false);
    });
  });

  describe("ThoughtTailExecutor", () => {
    it("should append text using native CharacterData.appendData without replacing text node", () => {
      const container = document.createElement("div");
      document.body.appendChild(container);

      // First chunk
      applyThoughtTailOp("msg-test-1", container, {
        op: "append",
        content: "Hello",
      });

      expect(container.textContent).toBe("Hello");
      const textNode = container.firstChild as Text;
      expect(textNode).toBeInstanceOf(Text);

      // Second chunk (incremental append)
      applyThoughtTailOp("msg-test-1", container, {
        op: "append",
        content: " World!",
      });

      expect(container.textContent).toBe("Hello World!");
      // CRITICAL: Text node reference MUST remain identical (CharacterData.appendData identity invariant)
      expect(container.firstChild).toBe(textNode);
      expect(container.childNodes.length).toBe(1);
    });

    it("should support replace operation and update state", () => {
      const container = document.createElement("div");
      document.body.appendChild(container);

      applyThoughtTailOp("msg-test-1", container, {
        op: "append",
        content: "Initial thought",
      });

      applyThoughtTailOp("msg-test-1", container, {
        op: "replace",
        content: "Replaced thought",
      });

      expect(container.textContent).toBe("Replaced thought");
      const state = getThoughtTailState("msg-test-1");
      expect(state?.lastAppliedContent).toBe("Replaced thought");
    });

    it("should clean up thought tail state and text node", () => {
      const container = document.createElement("div");
      applyThoughtTailOp("msg-test-1", container, {
        op: "append",
        content: "Thinking...",
      });

      expect(getThoughtTailState("msg-test-1")).toBeDefined();
      cleanupThoughtTail("msg-test-1");
      expect(getThoughtTailState("msg-test-1")).toBeUndefined();
    });

    it("should work through createThoughtTailDriver", () => {
      const container = document.createElement("div");
      const driver = createThoughtTailDriver("msg-test-1");
      driver.bindContainer(container);

      driver.applyOp({ op: "append", content: "Step 1: Analyzed." });
      driver.applyOp({ op: "append", content: " Step 2: Formulated." });
      expect(container.textContent).toBe("Step 1: Analyzed. Step 2: Formulated.");

      driver.applyOp({ op: "replace", content: "Final Summary." });
      expect(container.textContent).toBe("Final Summary.");

      driver.cleanup();
      expect(getThoughtTailState("msg-test-1")).toBeUndefined();
    });
  });

  describe("CodePatchExecutor", () => {
    function createMockCodeBlock(): {
      wrapper: HTMLElement;
      codeElement: HTMLElement;
      stableSpan: HTMLElement;
      activeSpan: HTMLElement;
    } {
      const wrapper = document.createElement("pre");
      const codeElement = document.createElement("code");
      codeElement.setAttribute("data-vcp-stream-code", "");

      const stableSpan = document.createElement("span");
      stableSpan.setAttribute("data-vcp-code-stable", "");

      const activeSpan = document.createElement("span");
      activeSpan.setAttribute("data-vcp-code-active", "");

      codeElement.appendChild(stableSpan);
      codeElement.appendChild(activeSpan);
      wrapper.appendChild(codeElement);
      document.body.appendChild(wrapper);

      return { wrapper, codeElement, stableSpan, activeSpan };
    }

    it("should append completedHtml to stable anchor and replace activeHtml in active anchor", () => {
      const { wrapper, stableSpan, activeSpan } = createMockCodeBlock();

      const result = applyCodePatch(
        wrapper,
        `<span class="line">const a = 1;</span>\n`,
        `<span class="line">const b =</span>`,
      );

      expect(result.ok).toBe(true);
      expect(stableSpan.innerHTML).toBe(`<span class="line">const a = 1;</span>\n`);
      expect(activeSpan.innerHTML).toBe(`<span class="line">const b =</span>`);

      // Second frame: second line completes, third line starts typing
      const result2 = applyCodePatch(
        wrapper,
        `<span class="line">const b = 2;</span>\n`,
        `<span class="line">console.</span>`,
      );

      expect(result2.ok).toBe(true);
      expect(stableSpan.innerHTML).toBe(
        `<span class="line">const a = 1;</span>\n<span class="line">const b = 2;</span>\n`,
      );
      expect(activeSpan.innerHTML).toBe(`<span class="line">console.</span>`);
    });

    it("should handle code element directly passed as targetNode", () => {
      const { codeElement, stableSpan, activeSpan } = createMockCodeBlock();

      const result = applyCodePatch(
        codeElement,
        `<span>line 1</span>`,
        `<span>line 2...</span>`,
      );

      expect(result.ok).toBe(true);
      expect(stableSpan.innerHTML).toBe(`<span>line 1</span>`);
      expect(activeSpan.innerHTML).toBe(`<span>line 2...</span>`);
    });

    it("should return error if code root or anchors are missing", () => {
      const emptyDiv = document.createElement("div");
      const res1 = applyCodePatch(emptyDiv, "foo", "bar");
      expect(res1.ok).toBe(false);
      expect(res1.reason).toContain("Code root");

      const brokenCode = document.createElement("code");
      brokenCode.setAttribute("data-vcp-stream-code", "");
      const res2 = applyCodePatch(brokenCode, "foo", "bar");
      expect(res2.ok).toBe(false);
      expect(res2.reason).toContain("Incremental code anchors");
    });

    it("should work through createCodePatchDriver", () => {
      const { wrapper, stableSpan, activeSpan } = createMockCodeBlock();
      const driver = createCodePatchDriver();

      const res = driver.applyPatch(
        wrapper,
        `<span>completed</span>`,
        `<span>typing</span>`,
      );
      expect(res.ok).toBe(true);
      expect(stableSpan.innerHTML).toBe(`<span>completed</span>`);
      expect(activeSpan.innerHTML).toBe(`<span>typing</span>`);
    });
  });

  describe("useDomRenderer Facade", () => {
    it("should orchestrate thought and registry cleanup on dispose", () => {
      const messageId = "msg-facade-1";
      const renderer = useDomRenderer(messageId);

      const container = document.createElement("div");
      renderer.thought.bindContainer(container);
      renderer.thought.applyOp({ op: "append", content: "Thinking in facade..." });
      expect(getThoughtTailState(messageId)?.lastAppliedContent).toBe("Thinking in facade...");

      setNode(messageId, "node-1", container);
      expect(hasRegistry(messageId)).toBe(true);

      renderer.dispose();

      expect(getThoughtTailState(messageId)).toBeUndefined();
      expect(hasRegistry(messageId)).toBe(false);
    });
  });
});

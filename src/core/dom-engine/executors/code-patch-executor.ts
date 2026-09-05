import type { CodePatchDriver, CodePatchResult } from "../types";

/**
 * 在目标 DOM 容器中查找流式代码块的稳定行锚点与活跃行锚点
 */
export function findCodeAnchors(
  codeRoot: HTMLElement,
): { stable: HTMLElement; active: HTMLElement } | null {
  const stable = codeRoot.querySelector<HTMLElement>("[data-vcp-code-stable]");
  const active = codeRoot.querySelector<HTMLElement>("[data-vcp-code-active]");
  if (stable && active) {
    return { stable, active };
  }
  return null;
}

/**
 * 对目标代码块执行双缓冲增量行级高亮补丁
 * 
 * 核心设计：
 * 1. Syntect 已在后端对代码片段完成 HTML 安全转义，前端在此只解析 <span> 片段；
 * 2. 已换行的完整代码行通过 documentFragment 追加进 [data-vcp-code-stable]，永久保留、零回流；
 * 3. 正在输入的末行通过 replaceChildren 原生置换进 [data-vcp-code-active]；
 * 4. 彻底消除每一帧整段代码重新 innerHTML 解析与高亮着色。
 */
export function applyCodePatch(
  targetNode: Node,
  completedHtml: string,
  activeHtml: string,
): CodePatchResult {
  const codeRoot = targetNode instanceof HTMLElement
    ? (
        targetNode.tagName === "CODE" && targetNode.hasAttribute("data-vcp-stream-code")
          ? targetNode
          : Array.from(targetNode.children).find((child) =>
              child instanceof HTMLElement
              && child.tagName === "CODE"
              && child.hasAttribute("data-vcp-stream-code")
            )
      )
    : undefined;

  if (!codeRoot || !(codeRoot instanceof HTMLElement)) {
    return {
      ok: false,
      reason: targetNode
        ? "Code root (<code data-vcp-stream-code>) not found"
        : "Target node is null/undefined",
    };
  }

  const anchors = findCodeAnchors(codeRoot);
  if (!anchors) {
    return {
      ok: false,
      reason: "Incremental code anchors ([data-vcp-code-stable] or [data-vcp-code-active]) not found",
    };
  }

  if (completedHtml) {
    const completedTemplate = document.createElement("template");
    completedTemplate.innerHTML = completedHtml;
    anchors.stable.appendChild(completedTemplate.content);
  }

  const activeTemplate = document.createElement("template");
  activeTemplate.innerHTML = activeHtml;
  anchors.active.replaceChildren(activeTemplate.content);

  return { ok: true };
}

/**
 * 实例化代码围栏增量补丁驱动
 */
export function createCodePatchDriver(): CodePatchDriver {
  return {
    applyPatch(targetNode: Node, completedHtml: string, activeHtml: string) {
      return applyCodePatch(targetNode, completedHtml, activeHtml);
    },
  };
}

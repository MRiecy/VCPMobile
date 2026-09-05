import type { TailTextOp } from "../types/chat";

export type ThoughtTailOpPayload =
  | TailTextOp
  | { op: "append"; content: string; previousHash?: string }
  | { op: "replace"; content: string }
  | { op: "clear" };

/**
 * 思维链 DOM 外科执行器契约
 */
export interface ThoughtTailDriver {
  /** 挂载或更新容器元素 */
  bindContainer(container: HTMLElement | null): void;
  /** 应用最新增量文本操作（append/replace/clear） */
  applyOp(op?: ThoughtTailOpPayload | null, fallbackContent?: string): void;
  /** 释放 DOM 引用与状态分片 */
  cleanup(): void;
}

/**
 * 代码围栏增量高亮（PatchCode）执行器契约
 */
export interface CodePatchResult {
  ok: boolean;
  reason?: string;
}

export interface CodePatchDriver {
  /**
   * 将增量高亮片段应用到目标代码块容器
   * @param targetNode 包含 <code data-vcp-stream-code> 的 DOM 节点
   * @param completedHtml 新增的完整高亮行 HTML
   * @param activeHtml 正在输入中的末行高亮 HTML
   */
  applyPatch(
    targetNode: Node,
    completedHtml: string,
    activeHtml: string,
  ): CodePatchResult;
}

/**
 * 单个消息的 DOM 渲染器统一上下文
 */
export interface DomRendererContext {
  readonly messageId: string;
  readonly thought: ThoughtTailDriver;
  readonly code: CodePatchDriver;
  dispose(): void;
}

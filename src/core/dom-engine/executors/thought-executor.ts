import type { ThoughtTailDriver, ThoughtTailOpPayload } from "../types";

export interface ThoughtTailState {
  container: HTMLElement;
  textNode: Text;
  lastAppliedContent: string;
}

const thoughtTailShards = new Map<string, ThoughtTailState>();

/**
 * 将思维链增量操作直接作用于底层 DOM，使用 CharacterData.appendData 进行微秒级手术式追加。
 * 彻底绕过 Vue 的 VDOM Diff、模板插值以及组件重渲染。
 */
export function applyThoughtTailOp(
  messageId: string,
  container: HTMLElement,
  op?: ThoughtTailOpPayload | null,
  fallbackContent?: string,
): void {
  const existing = thoughtTailShards.get(messageId);
  const containerChanged = !existing || existing.container !== container;

  // 1. 容器首次挂载或重新挂载（例如离开页面返回、重新挂载组件）
  if (containerChanged) {
    container.textContent = "";
    if (op?.op === "clear") {
      thoughtTailShards.delete(messageId);
      return;
    }

    const initialContent = op?.op === "replace"
      ? op.content
      : (fallbackContent ?? op?.content ?? "");
    const textNode = document.createTextNode(initialContent);
    container.appendChild(textNode);
    thoughtTailShards.set(messageId, {
      container,
      textNode,
      lastAppliedContent: initialContent,
    });
    return;
  }

  // 2. 已有活跃状态，执行对应 DOM 外科操作
  if (!op) return;

  switch (op.op) {
    case "append": {
      // 🚀 核心路径：原生 CharacterData.appendData，纳秒级增量文本追加
      existing.textNode.appendData(op.content);
      existing.lastAppliedContent += op.content;
      break;
    }

    case "replace": {
      if (existing.textNode.data !== op.content) {
        existing.textNode.data = op.content;
        existing.lastAppliedContent = op.content;
      }
      break;
    }

    case "clear": {
      container.textContent = "";
      thoughtTailShards.delete(messageId);
      break;
    }
  }
}

/**
 * 释放思维链 DOM 引用与状态分片，防止内存泄漏
 */
export function cleanupThoughtTail(messageId: string): void {
  const state = thoughtTailShards.get(messageId);
  if (state) {
    state.container.textContent = "";
    thoughtTailShards.delete(messageId);
  }
}

/**
 * 获取当前消息的思维链 DOM 状态（用于调试或单测断言）
 */
export function getThoughtTailState(messageId: string): ThoughtTailState | undefined {
  return thoughtTailShards.get(messageId);
}

/**
 * 实例化指定消息的思维链驱动对象
 */
export function createThoughtTailDriver(messageId: string): ThoughtTailDriver {
  let activeContainer: HTMLElement | null = null;

  return {
    bindContainer(container: HTMLElement | null) {
      activeContainer = container;
      if (!container) {
        cleanupThoughtTail(messageId);
      }
    },
    applyOp(op?: ThoughtTailOpPayload | null, fallbackContent?: string) {
      if (!activeContainer) return;
      applyThoughtTailOp(messageId, activeContainer, op, fallbackContent);
    },
    cleanup() {
      cleanupThoughtTail(messageId);
      activeContainer = null;
    },
  };
}

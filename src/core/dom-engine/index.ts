/**
 * DOM Engine 统一门面 (Facade)
 * 
 * 面向 Vue 视图层与整体渲染管线的统一接入入口。
 */

import { cleanupRegistry } from "./core/registry";
import { createThoughtTailDriver } from "./executors/thought-executor";
import { createCodePatchDriver } from "./executors/code-patch-executor";
import type { DomRendererContext } from "./types";

export * from "./types";
export * from "./core/registry";
export * from "./executors/thought-executor";
export * from "./executors/code-patch-executor";

/**
 * 为单个消息创建或获取 DOM 外科渲染器上下文
 */
export function useDomRenderer(messageId: string): DomRendererContext {
  const thought = createThoughtTailDriver(messageId);
  const code = createCodePatchDriver();

  return {
    messageId,
    thought,
    code,
    dispose() {
      thought.cleanup();
      cleanupRegistry(messageId);
    },
  };
}

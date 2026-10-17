import type { TailTextOp } from "../types/chat";

export type ThoughtTailOpPayload =
  | TailTextOp
  | { op: "append"; content: string; previousHash?: string }
  | { op: "replace"; content: string }
  | { op: "clear" };

/**
 * 代码围栏增量高亮（PatchCode）执行器契约
 */
export interface CodePatchResult {
  ok: boolean;
  reason?: string;
}

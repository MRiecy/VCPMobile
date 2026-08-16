/**
 * swipeController — 左边栏 Agent 行右滑状态控制器
 *
 * 从 AgentList.vue 提取的共享手势状态（原为组件本地 ref）：
 * - 教学引导的「右滑露出设置按钮」步骤需要以真实业务方式驱动同一状态，
 *   让真实行真正滑开（而非覆盖虚拟演示）；
 * - AgentList 手势逻辑（onTouchStart/Move/End）继续读写同一 ref，
 *   行为与提取前完全一致。
 */
import { ref } from 'vue';

/** 滑开落位距离（与 AgentList 原有 MAX_SWIPE 一致）。 */
export const MAX_SWIPE = 80;

export const activeSwipeId = ref<string | null>(null);
export const currentSwipeX = ref(0);

/** 教学引导用：让指定行平滑滑开（行自带 transition-all duration-300）。 */
export function swipeOpen(id: string): void {
  activeSwipeId.value = id;
  currentSwipeX.value = MAX_SWIPE;
}

/** 教学引导用：收起当前滑开的行（指定 id 时仅匹配该行；幂等）。 */
export function swipeClose(id?: string): void {
  if (id && activeSwipeId.value !== id) return;
  activeSwipeId.value = null;
  currentSwipeX.value = 0;
}

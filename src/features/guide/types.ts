/**
 * 教学引导（Coachmark）引擎类型定义
 *
 * 设计约束（见 plan/vcpmobile-guide-tour-research/）：
 * - 步骤推进仅靠卡片按钮（「下一步」/「我知道了」），不检测用户手势；
 * - predicates 必须为纯函数：只读响应式状态、无副作用；
 * - 指引仅 1–2 步，末步「我知道了」是唯一退出出口。
 */

/** 卡片相对目标的优先方位，Overlay 在空间不足时自动翻转。 */
export type GuidePlacement = 'top' | 'bottom' | 'left' | 'right';

/** 演示动画原语：长按 / 单击 / 右滑 / 纵向拖拽。 */
export type GuideDemo = 'press-hold' | 'tap' | 'swipe-right' | 'drag-vertical';

export interface GuideStep {
  /** 目标元素注册表 id；支持函数动态求值（如 sidebar 的「第一个 Agent 行」）。 */
  target: string | (() => string);
  title: string;
  content: string;
  /** 卡片首选方位，默认 'bottom'。 */
  placement?: GuidePlacement;
  /** 演示动画；缺省 = 纯说明步骤。 */
  demo?: GuideDemo;
  /**
   * 步骤开始时执行的真实业务动作（纯数据驱动：打开菜单/面板等安全操作，
   * 无数据副作用；异常被吞掉不阻塞教学）。
   */
  perform?: () => void;
  /**
   * perform 的延迟触发（ms，默认 0）：用于让手势演示先播放、真实业务在
   * 手势落定（如长按进度环走满 600ms）时再弹出。定时器为会话级——
   * 用户提前推进步骤时仍会触发，由后续步骤的 waitFor 接住；整场结束时清除。
   */
  performDelayMs?: number;
  /**
   * 整场结束时的收尾清理（幂等，可多次调用；步间不执行——
   * 步间清理由下一步 perform 显式负责）。
   */
  undo?: () => void;
  /** 可选步骤级门控；false 时该步保持等待直至超时。 */
  waitFor?: () => boolean;
  /** 目标等待超时（ms），超时静默越过该步。默认 3000。 */
  waitTimeoutMs?: number;
}

export interface GuideTrigger {
  /** 依赖的已完成指引 id 集合。 */
  requires?: string[];
  /** 状态条件，全部满足才可触发。 */
  predicates?: { name: string; check: () => boolean }[];
  /** 条件齐备后的稳定期（ms），防抖动。默认 600。 */
  settleMs?: number;
}

export interface GuideDefinition {
  id: string;
  /** 设置页回放列表展示用。 */
  title: string;
  description: string;
  /** 版本门控：introducedIn > lastSeenAppVersion 时入队提示（v1 指引均不填写）。 */
  introducedIn?: string;
  /** 触发规格；无 trigger = 仅回放/版本门控。 */
  trigger?: GuideTrigger;
  /** 回放前的准备钩子（如打开左边栏），纯数据驱动。 */
  prepare?: () => void;
  steps: GuideStep[];
}

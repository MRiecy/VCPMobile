/**
 * sidebar-gestures — 左边栏手势教学（右滑设置 + 按住拖动排序）
 *
 * 触发（全部满足才自动播）：左边栏可见（手机抽屉打开或平板 ≥1024px 常驻）
 * + agents ≥2 + 排位第一个 Agent 的行已挂载。
 * 高亮目标锁定「排位第一个 Agent」（研究第 6 轮裁决）。
 */
import { useMediaQuery } from '@vueuse/core';
import { defineGuide, hasTarget } from '../registry';
import { sortByOrder } from '../../agent/agentOrder';
import { useAssistantStore } from '../../../core/stores/assistant';
import { useSettingsStore } from '../../../core/stores/settings';
import { useLayoutStore } from '../../../core/stores/layout';

/** 平板断点常驻栏（与 useSidebarSwipe / AgentSidebar 的 1024px 断点一致）。 */
const isTabletMin1024 = useMediaQuery('(min-width: 1024px)');

/** 与 AgentList 渲染同源：agentOrder 优先、未知 id 置后，取排位第一个 Agent。 */
export const firstAgentId = (): string | null => {
  const assistantStore = useAssistantStore();
  const settingsStore = useSettingsStore();
  const sorted = sortByOrder(
    assistantStore.agents,
    settingsStore.settings?.agentOrder ?? [],
  );
  return sorted[0]?.id ?? null;
};

const firstAgentRowTarget = (): string => {
  const id = firstAgentId();
  return id ? `agent-row-${id}` : '';
};

defineGuide({
  id: 'sidebar-gestures',
  title: '左边栏手势',
  description: '右滑查看设置入口，按住拖动调整排序',
  prepare: () => {
    // 回放时确保左边栏可见（平板常驻栏下为无害 no-op）。
    useLayoutStore().setLeftDrawer(true);
  },
  trigger: {
    predicates: [
      {
        name: 'left-sidebar-visible',
        check: () => {
          const layoutStore = useLayoutStore();
          return layoutStore.leftDrawerOpen || isTabletMin1024.value;
        },
      },
      {
        name: 'agents-count-ge-2',
        check: () => useAssistantStore().agents.length >= 2,
      },
      {
        name: 'first-agent-row-mounted',
        check: () => {
          const id = firstAgentId();
          return id !== null && hasTarget(`agent-row-${id}`);
        },
      },
    ],
  },
  steps: [
    {
      target: firstAgentRowTarget,
      title: '右滑查看设置',
      content: '在第一个 Agent 上向右滑动，露出设置入口，点击即可进入对应设置页。',
      placement: 'right',
      demo: 'swipe-right',
    },
    {
      target: firstAgentRowTarget,
      title: '按住拖动排序',
      content: '按住第一个 Agent 约 0.2 秒后上下拖动，可调整排列顺序，松手落位。',
      placement: 'right',
      demo: 'drag-vertical',
    },
  ],
});

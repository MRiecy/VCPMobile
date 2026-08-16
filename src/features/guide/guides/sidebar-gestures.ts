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
import { swipeClose, swipeOpen } from '../../agent/swipeController';
import { useAssistantStore } from '../../../core/stores/assistant';
import { useSettingsStore } from '../../../core/stores/settings';
import { useLayoutStore } from '../../../core/stores/layout';
import { useOverlayStore } from '../../../core/stores/overlay';

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

const firstAgentSettingsTarget = (): string => {
  const id = firstAgentId();
  return id ? `agent-row-settings-${id}` : '';
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
        name: 'workspace-not-occluded',
        // 设置/日记中心等 SlidePage 页面栈会盖住左边栏与聊天主界面；
        // 目标被遮挡时不触发（触发规格见研究 02/03，本谓词为可达性兜底）。
        check: () => useOverlayStore().pageStack.length === 0,
      },
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
      content: '在第一个 Agent 上向右滑动，露出背后的设置入口。',
      placement: 'right',
      demo: 'swipe-right',
      // 真实业务：驱动真实行滑开（swipeController 即 AgentList 手势同源状态）。
      perform: () => {
        const id = firstAgentId();
        if (id) swipeOpen(id);
      },
      undo: swipeClose,
    },
    {
      target: firstAgentSettingsTarget,
      title: '设置入口',
      content: '滑动后露出的齿轮按钮，点击即可进入该 Agent 的设置页。',
      placement: 'right',
    },
    {
      target: firstAgentRowTarget,
      title: '按住拖动排序',
      content: '按住 Agent 约 0.2 秒后上下拖动，可调整排列顺序，松手落位。',
      placement: 'right',
      demo: 'drag-vertical',
      undo: swipeClose,
    },
  ],
});

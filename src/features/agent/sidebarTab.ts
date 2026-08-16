/**
 * sidebarTab — 左边栏「助理 / 话题」Tab 状态控制器
 *
 * 从 AgentSidebar.vue 提取的共享状态（原为组件本地 ref）：
 * - 教学引导回放/重置需要确保左边栏处于「助理」页，Agent 行才会挂载
 *   （话题页下 sidebar-gestures 的目标永不出现，指引会静默越过）。
 */
import { ref } from 'vue';

export type SidebarTab = 'agents' | 'topics';

export const sidebarTab = ref<SidebarTab>('agents');

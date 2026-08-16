/**
 * Agent/Group 列表排序共享函数
 *
 * 从 AgentList.vue 抽取，供列表渲染与 guide 定义（sidebar-gestures 的
 * 「排位第一个 Agent」判定）共用，L4 契约测试防分歧：
 * - order（agentOrder/groupOrder）优先；
 * - 未知 id 置后；
 * - order 为空时原样返回（引用不变）。
 */
export function sortByOrder<T extends { id: string }>(items: T[], order: string[]): T[] {
  if (order.length === 0) return items;
  return [...items].sort((a, b) => {
    const indexA = order.indexOf(a.id);
    const indexB = order.indexOf(b.id);
    if (indexA === -1 && indexB === -1) return 0;
    if (indexA === -1) return 1;
    if (indexB === -1) return -1;
    return indexA - indexB;
  });
}

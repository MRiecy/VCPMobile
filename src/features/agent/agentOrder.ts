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
  const ranks = new Map<string, number>();
  order.forEach((id, index) => {
    // indexOf used the first occurrence when a saved order contained duplicates.
    if (!ranks.has(id)) ranks.set(id, index);
  });
  return [...items].sort((a, b) => {
    const indexA = ranks.get(a.id);
    const indexB = ranks.get(b.id);
    if (indexA === undefined && indexB === undefined) return 0;
    if (indexA === undefined) return 1;
    if (indexB === undefined) return -1;
    return indexA - indexB;
  });
}

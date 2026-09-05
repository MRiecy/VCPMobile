/**
 * DOM Engine Node Registry (节点注册中心)
 * 
 * 采用按 messageId 分片的两层 Map 结构：
 * 第一层：messageId -> Map<nodeId, Node>
 * 第二层：nodeId -> 原生 DOM Node
 * 
 * 职责：
 * 1. 提供 O(1) 的原生 DOM 节点索引与寻址能力；
 * 2. 消息级别的内存隔离，防止多消息并发流式渲染时相互污染；
 * 3. 严格的生命周期清理，防止引用泄漏。
 */

const registryShards = new Map<string, Map<string, Node>>();

/**
 * 获取或初始化指定消息的节点注册表分片
 */
export function getRegistry(messageId: string): Map<string, Node> {
  let shard = registryShards.get(messageId);
  if (!shard) {
    shard = new Map();
    registryShards.set(messageId, shard);
  }
  return shard;
}

/**
 * 查找指定消息中特定 ID 的 DOM 节点
 */
export function getNode(messageId: string, nodeId: string): Node | undefined {
  return registryShards.get(messageId)?.get(nodeId);
}

/**
 * 注册或更新 DOM 节点引用
 */
export function setNode(messageId: string, nodeId: string, node: Node): void {
  getRegistry(messageId).set(nodeId, node);
}

/**
 * 移除指定 DOM 节点引用
 */
export function deleteNode(messageId: string, nodeId: string): boolean {
  return registryShards.get(messageId)?.delete(nodeId) ?? false;
}

/**
 * 释放指定消息的全部节点引用与注册表分片
 * @returns 释放的节点总数
 */
export function cleanupRegistry(messageId: string): number {
  const registry = registryShards.get(messageId);
  const size = registry ? registry.size : 0;
  registryShards.delete(messageId);
  return size;
}

/**
 * 检查指定消息是否存在活跃注册表
 */
export function hasRegistry(messageId: string): boolean {
  return registryShards.has(messageId);
}

/**
 * 获取当前消息注册的节点总数
 */
export function getRegistrySize(messageId: string): number {
  return registryShards.get(messageId)?.size ?? 0;
}

/**
 * 清空所有分片（仅供全局重置或测试套件 teardown）
 */
export function clearAllRegistries(): void {
  registryShards.clear();
}

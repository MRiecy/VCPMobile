/**
 * 指引注册表与目标元素注册表
 *
 * - `defineGuide()`：指引定义注册（重复 id 开发期告警）。
 * - 目标元素注册表：reactive Map，供 v-guide 指令写入/注销；
 *   同 id 支持多元素（v-for / 虚拟列表），步骤解析取首个有效元素。
 */
import { reactive } from 'vue';
import type { GuideDefinition } from './types';

const guideRegistry = new Map<string, GuideDefinition>();

export function defineGuide(def: GuideDefinition): GuideDefinition {
  if (guideRegistry.has(def.id)) {
    console.warn(`[Guide] Duplicate guide definition id: ${def.id}`);
  }
  guideRegistry.set(def.id, def);
  return def;
}

export function getGuide(id: string): GuideDefinition | undefined {
  return guideRegistry.get(id);
}

export function allGuides(): GuideDefinition[] {
  return [...guideRegistry.values()];
}

/** 目标元素注册表：id → 已挂载元素数组（挂载顺序）。 */
export const targetRegistry = reactive(new Map<string, HTMLElement[]>());

export function registerTarget(id: string, el: HTMLElement): void {
  const list = targetRegistry.get(id);
  if (list) {
    if (!list.includes(el)) list.push(el);
  } else {
    targetRegistry.set(id, [el]);
  }
}

export function unregisterTarget(id: string, el: HTMLElement): void {
  const list = targetRegistry.get(id);
  if (!list) return;
  const index = list.indexOf(el);
  if (index !== -1) list.splice(index, 1);
  if (list.length === 0) targetRegistry.delete(id);
}

export function hasTarget(id: string): boolean {
  return (targetRegistry.get(id)?.length ?? 0) > 0;
}

/**
 * 解析目标：返回首个已连接且渲染盒非零的元素（不包含视口判定，
 * 视口与稳定性检查由 GuideOverlay 几何层负责）。
 */
export function resolveTarget(id: string): HTMLElement | null {
  const list = targetRegistry.get(id);
  if (!list) return null;
  for (const el of list) {
    if (!el.isConnected) continue;
    const rect = el.getBoundingClientRect();
    if (rect.width > 0 && rect.height > 0) return el;
  }
  return null;
}

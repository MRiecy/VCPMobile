/**
 * scrollToMessage.ts — 消息 DOM 定位与高亮闪烁工具。
 *
 * 依赖消息渲染 DOM 上的 data-message-id 属性（ChatView.vue / MessageRenderer.vue）。
 * 服务于全局搜索结果跳转与 #msg- 锚点引用跳转（useMessageEvents.ts）。
 */

const FLASH_CLASS = 'vcp-message-anchor-flash';
const FLASH_DURATION_MS = 1500;

/**
 * 滚动定位到指定消息并短暂高亮。
 * @returns 是否找到目标元素（消息不在当前已加载窗口时返回 false，调用方应先加载锚点窗口）
 */
export function scrollToMessageById(msgId: string, options?: { flash?: boolean }): boolean {
  const el = document.querySelector<HTMLElement>(
    `[data-message-id="${CSS.escape(msgId)}"]`,
  );
  if (!el) return false;

  el.scrollIntoView({ behavior: 'smooth', block: 'center' });

  if (options?.flash !== false) {
    el.classList.remove(FLASH_CLASS);
    // 强制 reflow 以重启动画
    void el.offsetWidth;
    el.classList.add(FLASH_CLASS);
    window.setTimeout(() => el.classList.remove(FLASH_CLASS), FLASH_DURATION_MS);
  }
  return true;
}

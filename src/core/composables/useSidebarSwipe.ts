import { useSwipe } from '@vueuse/core';
import { useLayoutStore } from '../stores/layout';
import type { Ref } from 'vue';

export type SidebarSwipeType = 'global' | 'left' | 'right';

export interface SidebarSwipeOptions {
  type: SidebarSwipeType;
  onTabSwitch?: () => void;
}

/**
 * 递归判断发起事件的目标元素是否处于可滑动的展示区或具有滚动条的容器中，防止手势误触冲突
 */
function isScrollableArea(el: Element): boolean {
  // 1. 常见横向滚动容器类名或输入元素直接拦截 (排除 overflow-y-auto，防止干扰聊天主纵向滚动列表)
  if (
    el.closest('.vcp-scrollable') ||
    el.closest('.overflow-x-auto')
  ) {
    return true;
  }

  // 2. 表格相关元素直接免疫 (因移动端表格极其宽阔，横滚操作极其常见)
  if (
    el.closest('table') ||
    el.closest('td') ||
    el.closest('th') ||
    el.closest('.table-container') ||
    el.closest('.vcp-table')
  ) {
    return true;
  }

  // 3. 代码展示块等横向滚动区域免疫
  if (
    el.closest('pre') ||
    el.closest('code') ||
    el.closest('.vcp-code-block')
  ) {
    return true;
  }

  // 4. 输入框和文本域本身有自带滑动选择或默认拖拽行为，直接免疫
  const tagName = el.tagName.toLowerCase();
  if (tagName === 'textarea' || tagName === 'input') {
    return true;
  }

  // 5. 递归向上检测其祖先节点的 computed style 中是否有隐藏的滚动区
  let current: Element | null = el;
  while (current && current !== document.body) {
    const style = window.getComputedStyle(current);
    const overflowX = style.overflowX;

    // 仅侦测横向（X 轴）滚动溢出，因为侧边栏侧滑是水平手势，只与横向滚动条冲突
    const hasScrollX = (overflowX === 'auto' || overflowX === 'scroll') && current.scrollWidth > current.clientWidth;

    if (hasScrollX) {
      return true;
    }

    current = current.parentElement;
  }

  return false;
}

/**
 * 统一管理侧边栏滑动响应的组合式函数
 * 支持：
 * 1. global: 仅在侧边栏关闭时，从左滑向右开启左侧边栏，从右滑向左开启右侧边栏（避开滚动区域）
 * 2. left: 左侧边栏内部，向左滑关闭，或向右滑执行自定义 tab 切换行为
 * 3. right: 右侧边栏内部，向右滑关闭
 */
export function useSidebarSwipe(target: Ref<HTMLElement | null>, options: SidebarSwipeOptions) {
  const layoutStore = useLayoutStore();

  const { direction, lengthX, lengthY, isSwiping } = useSwipe(target, {
    threshold: options.type === 'global' ? 30 : 15,
    onSwipeEnd: (e: TouchEvent | MouseEvent) => {
      // 检查是否从受限区域发起
      if (e.target instanceof Element && e.target.closest('.no-swipe')) return;

      const absX = Math.abs(lengthX.value);
      const absY = Math.abs(lengthY.value);

      // 水平手势判定：角度在 30 度以内 (tan(30deg) ≈ 0.577)
      const isHorizontal = absX > 0 && absY / absX < 0.577;
      if (!isHorizontal) return;

      if (options.type === 'global') {
        // 避开滚动区域以防跟页面内滚动冲突，同时避开侧边栏内部以防跟侧边栏自身的手势发生事件冒泡冲突
        if (e.target instanceof Element) {
          if (
            e.target.closest('.vcp-scrollable') ||
            e.target.closest('.vcp-drawer') ||
            isScrollableArea(e.target)
          ) return;
        }

        if (!layoutStore.leftDrawerOpen && !layoutStore.rightDrawerOpen) {
          // 从左往右划 -> 开启左侧边栏 (需要一定位移以防误触)
          if (
            direction.value === 'right' &&
            absX > 60 &&
            !window.matchMedia('(min-width: 1024px)').matches
          ) {
            layoutStore.setLeftDrawer(true);
          }
          // 从右往左划 -> 开启右侧边栏
          else if (
            direction.value === 'left' &&
            absX > 60 &&
            !window.matchMedia('(min-width: 1280px)').matches
          ) {
            layoutStore.setRightDrawer(true);
          }
        }
      } else if (options.type === 'left') {
        if (layoutStore.leftDrawerOpen && !window.matchMedia('(min-width: 1024px)').matches) {
          // 向左滑 -> 关闭左侧边栏
          if (direction.value === 'left' && absX > 50) {
            layoutStore.setLeftDrawer(false);
          }
          // 向右滑 -> 智能切回助手列表 (或其他自定义 Tab 行为)
          else if (direction.value === 'right' && absX > 50) {
            options.onTabSwitch?.();
          }
        }
      } else if (options.type === 'right') {
        if (layoutStore.rightDrawerOpen && !window.matchMedia('(min-width: 1280px)').matches) {
          // 向右滑 -> 关闭右侧边栏
          if (direction.value === 'right' && absX > 50) {
            layoutStore.setRightDrawer(false);
          }
        }
      }
    },
  });

  return { direction, lengthX, lengthY, isSwiping };
}
export type UseSidebarSwipeReturn = ReturnType<typeof useSidebarSwipe>;

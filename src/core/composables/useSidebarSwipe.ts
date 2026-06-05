import { useSwipe } from '@vueuse/core';
import { useLayoutStore } from '../stores/layout';
import type { Ref } from 'vue';

export type SidebarSwipeType = 'global' | 'left' | 'right';

export interface SidebarSwipeOptions {
  type: SidebarSwipeType;
  onTabSwitch?: () => void;
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
        if (e.target instanceof Element && (e.target.closest('.vcp-scrollable') || e.target.closest('.vcp-drawer'))) return;

        if (!layoutStore.leftDrawerOpen && !layoutStore.rightDrawerOpen) {
          // 从左往右划 -> 开启左侧边栏 (需要一定位移以防误触)
          if (direction.value === 'right' && absX > 60) {
            layoutStore.setLeftDrawer(true);
          }
          // 从右往左划 -> 开启右侧边栏
          else if (direction.value === 'left' && absX > 60) {
            layoutStore.setRightDrawer(true);
          }
        }
      } else if (options.type === 'left') {
        if (layoutStore.leftDrawerOpen) {
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
        if (layoutStore.rightDrawerOpen) {
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

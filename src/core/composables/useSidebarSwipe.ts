import { useEventListener, useSwipe } from '@vueuse/core';
import { useLayoutStore } from '../stores/layout';
import { useVoiceCallStore } from '../../features/chat/voice/voiceCallStore';
import { computed, nextTick, onScopeDispose, watch, type Ref } from 'vue';

export type SidebarSwipeType = 'global' | 'left' | 'right';

export interface SidebarSwipeOptions {
  type: SidebarSwipeType;
  onTabSwitch?: () => void;
}

/**
 * 递归判断发起事件的目标元素是否处于可滑动的展示区或具有滚动条的容器中，防止手势误触冲突
 */
function isScrollableArea(el: Element, insideDrawer = false): boolean {
  // 1. 常见横向滚动容器类名或输入元素直接拦截 (排除 overflow-y-auto，防止干扰聊天主纵向滚动列表)
  if (
    (!insideDrawer && el.closest('.vcp-scrollable')) ||
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
  if (el.closest('input, textarea, select, [contenteditable]:not([contenteditable="false"]), [role="slider"]')) {
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

/** 抽屉的临时呈现归本手势所有；最终开关仍只写 layoutStore。 */
function useDrawerSwipe(target: Ref<HTMLElement | null>, options: SidebarSwipeOptions) {
  const layout = useLayoutStore();
  const voice = useVoiceCallStore();
  const sign = options.type === 'left' ? -1 : 1;
  const paneQuery = options.type === 'left' ? '(min-width: 1024px)' : '(min-width: 1280px)';
  const isOpen = () => options.type === 'left' ? layout.leftDrawerOpen : layout.rightDrawerOpen;
  type Gesture = {
    id: number;
    origin: Element;
    x: number;
    y: number;
    dx: number;
    width: number;
    initialOffset: number;
    offset: number;
    mode: 'pending' | 'drag' | 'tab';
    sampleX: number;
    sampleTime: number;
    lastX: number;
    lastTime: number;
    velocity: number;
  };
  let gesture: Gesture | null = null;
  let drawer: HTMLElement | null = null;
  let overlay: HTMLElement | null = null;
  let frame = 0;
  let endCleanup = 0;
  let revision = 0;
  let suppressClick = false;

  const clearMotion = () => {
    revision++;
    cancelAnimationFrame(frame);
    frame = 0;
    window.clearTimeout(endCleanup);
    endCleanup = 0;
    drawer?.classList.remove('is-dragging');
    drawer?.style.removeProperty('transform');
    overlay?.classList.remove('is-dragging');
    overlay?.style.removeProperty('opacity');
    drawer = null;
    overlay = null;
  };
  const cancel = () => {
    gesture = null;
    clearMotion();
  };
  const paint = () => {
    frame = 0;
    if (!gesture || gesture.mode !== 'drag' || !drawer) return;
    drawer.style.transform = `translate3d(${gesture.offset}px, 0, 0)`;
    if (overlay) overlay.style.opacity = String(1 - Math.abs(gesture.offset) / gesture.width);
  };

  const start = (event: TouchEvent) => {
    cancel();
    if (voice.active || !isOpen() || window.matchMedia(paneQuery).matches || event.touches.length !== 1) return;
    const origin = event.target;
    const element = target.value;
    if (!(origin instanceof Element) || !element || event.defaultPrevented || origin.closest('.no-swipe') || isScrollableArea(origin, true)) return;
    const touch = event.touches[0];
    gesture = {
      id: touch.identifier, origin, x: touch.clientX, y: touch.clientY, dx: 0,
      width: 0, initialOffset: 0, offset: 0, mode: 'pending',
      sampleX: touch.clientX, sampleTime: event.timeStamp,
      lastX: touch.clientX, lastTime: event.timeStamp, velocity: 0,
    };
  };

  const move = (event: TouchEvent) => {
    const current = gesture;
    if (!current) return;
    if (event.touches.length !== 1 || event.touches[0].identifier !== current.id || event.defaultPrevented || !event.cancelable || current.origin.closest('.no-swipe')) {
      cancel();
      return;
    }
    const touch = event.touches[0];
    current.dx = touch.clientX - current.x;
    if (current.mode === 'pending') {
      const dy = touch.clientY - current.y;
      if (Math.max(Math.abs(current.dx), Math.abs(dy)) < 10) return;
      // 一旦纵向滚动获胜，本轮不能再被横向移动夺走。
      if (Math.abs(dy) >= Math.abs(current.dx) * 0.577) {
        cancel();
        return;
      }
      if (sign * current.dx > 0) {
        current.mode = 'drag';
        drawer = target.value;
        if (!drawer?.parentElement) { cancel(); return; }
        // 横向手势获胜时才取当前动画位置，按住等待期间仍可自然入场/回弹。
        const rect = drawer.getBoundingClientRect();
        const parentRect = drawer.parentElement.getBoundingClientRect();
        if (rect.width <= 0) { cancel(); return; }
        const translation = sign === -1 ? rect.left - parentRect.left : rect.right - parentRect.right;
        current.width = rect.width;
        current.initialOffset = sign * Math.min(rect.width, Math.max(0, sign * translation));
        overlay = drawer?.parentElement?.querySelector<HTMLElement>(':scope > .vcp-drawer-overlay') ?? null;
        drawer?.classList.add('is-dragging');
        overlay?.classList.add('is-dragging');
      } else if (options.type === 'left' && options.onTabSwitch) {
        current.mode = 'tab';
      } else {
        cancel();
        return;
      }
      suppressClick = true;
    }
    if (event.cancelable) event.preventDefault();
    if (current.mode !== 'drag') return;
    // 最近一段轨迹决定甩手方向，超过 100ms 的停顿不继承早先速度。
    if (event.timeStamp - current.sampleTime > 100) {
      current.sampleX = current.lastX;
      current.sampleTime = current.lastTime;
    }
    const elapsed = event.timeStamp - current.sampleTime;
    current.velocity = elapsed > 0 ? sign * (touch.clientX - current.sampleX) / elapsed : 0;
    if ((touch.clientX - current.lastX) * (current.lastX - current.sampleX) < 0) {
      current.sampleX = current.lastX;
      current.sampleTime = current.lastTime;
      const recentElapsed = event.timeStamp - current.lastTime;
      current.velocity = recentElapsed > 0 ? sign * (touch.clientX - current.lastX) / recentElapsed : 0;
    }
    current.lastX = touch.clientX;
    current.lastTime = event.timeStamp;
    current.offset = sign * Math.min(current.width, Math.max(0, sign * (current.initialOffset + current.dx)));
    if (!frame) frame = requestAnimationFrame(paint);
  };

  const end = (event: TouchEvent) => {
    const current = gesture;
    if (!current) return;
    if (event.type === 'touchcancel' || event.touches.length !== 0 || current.origin.closest('.no-swipe')) {
      cancel();
      return;
    }
    if (current.mode === 'drag') {
      if (event.cancelable) event.preventDefault();
      const released = event.changedTouches[0];
      if (released?.identifier === current.id && released.clientX !== current.lastX) {
        const elapsed = event.timeStamp - current.lastTime;
        current.velocity = elapsed > 0 ? sign * (released.clientX - current.lastX) / elapsed : 0;
        current.dx = released.clientX - current.x;
        current.offset = sign * Math.min(current.width, Math.max(0, sign * (current.initialOffset + current.dx)));
        current.lastTime = event.timeStamp;
      }
      cancelAnimationFrame(frame);
      paint();
      // 只在交接时结算样式；逐帧路径不读布局，避免 read/write 往返。
      if (drawer) void window.getComputedStyle(drawer).transform;
      const distance = Math.abs(current.offset);
      const fling = sign * current.dx >= 20 && current.velocity >= 0.5 && event.timeStamp - current.lastTime <= 100;
      gesture = null;
      const settlingRevision = revision;
      if (distance >= current.width * 0.25 || fling) {
        if (options.type === 'left') layout.setLeftDrawer(false);
        else layout.setRightDrawer(false);
      }
      // 等待 Vue 提交 is-open 后，把当前位置交给同一套 CSS 退场/回弹曲线。
      void nextTick(() => {
        if (revision === settlingRevision) clearMotion();
      });
    } else {
      gesture = null;
      if (current.mode === 'tab' && current.dx > 50) options.onTabSwitch?.();
    }
  };

  // 维持子组件的冒泡仲裁：行编辑、通知卡右滑与 Sortable 可先消费触摸。
  useEventListener(target, 'touchstart', start, { passive: true });
  useEventListener(target, 'touchmove', move, { passive: false });
  useEventListener(target, ['touchend', 'touchcancel'], end, { passive: false });
  useEventListener(document, 'touchstart', (event: TouchEvent) => {
    suppressClick = false;
    if (gesture && event.touches.length !== 1) cancel();
  }, { capture: true, passive: true });
  useEventListener(document, ['touchend', 'touchcancel'], () => {
    // 子级可能 stopPropagation(end)：只清理遗留候选，不替子级提交手势。
    // 原生事件在 listener 之间也能执行微任务，必须等整个事件任务结束。
    const ending = gesture;
    if (!ending) return;
    window.clearTimeout(endCleanup);
    endCleanup = window.setTimeout(() => {
      endCleanup = 0;
      if (ending && gesture === ending) cancel();
    }, 0);
  }, { capture: true, passive: true });
  useEventListener(document, 'click', (event: MouseEvent) => {
    if (!suppressClick || event.detail === 0) return;
    suppressClick = false;
    event.preventDefault();
    event.stopImmediatePropagation();
  }, { capture: true });
  useEventListener(window, 'resize', cancel);
  watch([() => voice.active, () => layout.leftDrawerOpen, () => layout.rightDrawerOpen, target], () => {
    if (gesture) cancel();
  }, { flush: 'sync' });
  onScopeDispose(cancel);
}

/**
 * 统一管理侧边栏滑动响应的组合式函数
 * 支持：
 * 1. global: 仅在侧边栏关闭时，从左滑向右开启左侧边栏，从右滑向左开启右侧边栏（避开滚动区域）
 * 2. left: 左侧边栏内部，向左滑关闭，或向右滑执行自定义 tab 切换行为
 * 3. right: 右侧边栏内部，向右滑关闭
 */
export function useSidebarSwipe(target: Ref<HTMLElement | null>, options: SidebarSwipeOptions) {
  if (options.type !== 'global') {
    useDrawerSwipe(target, options);
    return;
  }
  const layoutStore = useLayoutStore();
  const voiceCallStore = useVoiceCallStore();
  const swipeTarget = computed(() => voiceCallStore.active ? null : target.value);
  let gestureAllowed = false;

  watch(() => voiceCallStore.active, (active) => {
    if (active) gestureAllowed = false;
  }, { flush: 'sync' });

  const { direction, lengthX, lengthY } = useSwipe(swipeTarget, {
    threshold: 30,
    onSwipeStart: () => {
      gestureAllowed = !voiceCallStore.active;
    },
    onSwipeEnd: (e: TouchEvent | MouseEvent) => {
      const canHandle = gestureAllowed && !voiceCallStore.active;
      gestureAllowed = false;
      if (!canHandle || e.type === 'touchcancel') return;

      // 检查是否从受限区域发起
      if (e.target instanceof Element && e.target.closest('.no-swipe')) return;

      const absX = Math.abs(lengthX.value);
      const absY = Math.abs(lengthY.value);

      // 水平手势判定：角度在 30 度以内 (tan(30deg) ≈ 0.577)
      const isHorizontal = absX > 0 && absY / absX < 0.577;
      if (!isHorizontal) return;

      // 全局开栏保留原有松手语义，不接管侧栏与内容横向滚动。
      if (e.target instanceof Element && (
        e.target.closest('.vcp-drawer, .vcp-drawer-overlay') || isScrollableArea(e.target)
      )) return;

      if (!layoutStore.leftDrawerOpen && !layoutStore.rightDrawerOpen) {
        if (direction.value === 'right' && absX > 60 && !window.matchMedia('(min-width: 1024px)').matches) {
          layoutStore.setLeftDrawer(true);
        } else if (direction.value === 'left' && absX > 60 && !window.matchMedia('(min-width: 1280px)').matches) {
          layoutStore.setRightDrawer(true);
        }
      }
    },
  });
}

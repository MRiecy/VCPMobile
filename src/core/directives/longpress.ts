import type { Directive, DirectiveBinding } from 'vue';

export const vLongpress: Directive = {
  mounted(el: HTMLElement, binding: DirectiveBinding) {
    if (typeof binding.value !== 'function') {
      console.warn('v-longpress requires a function value');
      return;
    }

    const callback = binding.value;
    const delay = 600; // 长按触发时间 ms
    const suppressClick = binding.modifiers['suppress-click'] === true;
    let pressTimer: number | null = null;
    let suppressionExpiryTimer: number | null = null;
    let isTouchMoved = false;
    let didLongPress = false;
    let lastLongPressAt: number | null = null;

    const clearSuppressionExpiry = () => {
      if (suppressionExpiryTimer !== null) {
        clearTimeout(suppressionExpiryTimer);
        suppressionExpiryTimer = null;
      }
    };

    const armSuppressionExpiry = () => {
      if (!suppressClick || !didLongPress) return;
      clearSuppressionExpiry();
      suppressionExpiryTimer = window.setTimeout(() => {
        didLongPress = false;
        suppressionExpiryTimer = null;
      }, 700);
    };

    // 同一触摸长按可能同时产生 timer 与 contextmenu，只允许回调一次。
    const executeLongPress = (e: Event) => {
      const now = Date.now();
      if (lastLongPressAt !== null && now - lastLongPressAt < 800) return;
      lastLongPressAt = now;
      didLongPress = suppressClick;
      callback(e);
    };

    const start = (e: Event) => {
      // 如果是鼠标事件且不是左键，跳过（右键由 contextmenu 处理）
      if (e.type === 'mousedown' && (e as MouseEvent).button !== 0) {
        return;
      }
      clearSuppressionExpiry();
      didLongPress = false;
      isTouchMoved = false;

      if (pressTimer === null) {
        pressTimer = window.setTimeout(() => {
          pressTimer = null;
          if (!isTouchMoved) {
            executeLongPress(e);
          }
        }, delay);
      }
    };

    const end = () => {
      cancel();
      armSuppressionExpiry();
    };

    const cancel = () => {
      if (pressTimer !== null) {
        clearTimeout(pressTimer);
        pressTimer = null;
      }
    };

    const move = (e: Event) => {
      // 容忍轻微的手指抖动
      if (e.type === 'touchmove') {
        // 此处可以加入坐标计算来容忍位移，但最简单的是只要触发了 move 就认为要滑动
        isTouchMoved = true;
      } else {
        isTouchMoved = true;
      }
      cancel();
    };

    // --- 绑定事件 ---

    // Touch events (移动端)
    el.addEventListener('touchstart', start, { passive: true });
    el.addEventListener('touchend', end);
    el.addEventListener('touchmove', move, { passive: true });
    el.addEventListener('touchcancel', end);

    // Mouse events (桌面端模拟长按)
    el.addEventListener('mousedown', start);
    el.addEventListener('mouseup', end);
    el.addEventListener('mousemove', move);
    el.addEventListener('mouseleave', end);

    const onClickCapture = (e: MouseEvent) => {
      if (!suppressClick || !didLongPress) return;
      didLongPress = false;
      clearSuppressionExpiry();
      e.preventDefault();
      e.stopPropagation();
      e.stopImmediatePropagation();
    };
    el.addEventListener('click', onClickCapture, true);

    // Context menu (桌面端原生右键)
    const onContextMenu = (e: Event) => {
      e.preventDefault(); // 拦截原生右键菜单
      cancel(); // 取消可能正在进行的长按计时
      executeLongPress(e);
      armSuppressionExpiry();
    };
    el.addEventListener('contextmenu', onContextMenu);

    // 保存清理函数
    (el as any)._longpressCleanup = () => {
      el.removeEventListener('touchstart', start);
      el.removeEventListener('touchend', end);
      el.removeEventListener('touchmove', move);
      el.removeEventListener('touchcancel', end);
      el.removeEventListener('mousedown', start);
      el.removeEventListener('mouseup', end);
      el.removeEventListener('mousemove', move);
      el.removeEventListener('mouseleave', end);
      el.removeEventListener('click', onClickCapture, true);
      el.removeEventListener('contextmenu', onContextMenu);
      cancel();
      clearSuppressionExpiry();
    };
  },
  unmounted(el: HTMLElement) {
    if ((el as any)._longpressCleanup) {
      (el as any)._longpressCleanup();
    }
  }
};

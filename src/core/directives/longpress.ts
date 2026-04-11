import type { Directive, DirectiveBinding } from 'vue';

export const vLongpress: Directive = {
  mounted(el: HTMLElement, binding: DirectiveBinding) {
    if (typeof binding.value !== 'function') {
      console.warn('v-longpress requires a function value');
      return;
    }

    const callback = binding.value;
    const delay = 600; // 长按触发时间 ms
    let pressTimer: number | null = null;
    let isTouchMoved = false;

    // 触发长按逻辑
    const executeLongPress = (e: Event) => {
      // 在 WebView 中触发震动反馈 (物理感知)
      if (navigator.vibrate) {
        navigator.vibrate(50);
      }
      callback(e);
    };

    const start = (e: Event) => {
      // 如果是鼠标事件且不是左键，跳过（右键由 contextmenu 处理）
      if (e.type === 'mousedown' && (e as MouseEvent).button !== 0) {
        return;
      }
      isTouchMoved = false;

      if (pressTimer === null) {
        pressTimer = window.setTimeout(() => {
          if (!isTouchMoved) {
            executeLongPress(e);
          }
        }, delay);
      }
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
    el.addEventListener('touchend', cancel);
    el.addEventListener('touchmove', move, { passive: true });
    el.addEventListener('touchcancel', cancel);

    // Mouse events (桌面端模拟长按)
    el.addEventListener('mousedown', start);
    el.addEventListener('mouseup', cancel);
    el.addEventListener('mousemove', move);
    el.addEventListener('mouseleave', cancel);

    // Context menu (桌面端原生右键)
    const onContextMenu = (e: Event) => {
      e.preventDefault(); // 拦截原生右键菜单
      cancel(); // 取消可能正在进行的长按计时
      executeLongPress(e);
    };
    el.addEventListener('contextmenu', onContextMenu);

    // 保存清理函数
    (el as any)._longpressCleanup = () => {
      el.removeEventListener('touchstart', start);
      el.removeEventListener('touchend', cancel);
      el.removeEventListener('touchmove', move);
      el.removeEventListener('touchcancel', cancel);
      el.removeEventListener('mousedown', start);
      el.removeEventListener('mouseup', cancel);
      el.removeEventListener('mousemove', move);
      el.removeEventListener('mouseleave', cancel);
      el.removeEventListener('contextmenu', onContextMenu);
    };
  },
  unmounted(el: HTMLElement) {
    if ((el as any)._longpressCleanup) {
      (el as any)._longpressCleanup();
    }
  }
};
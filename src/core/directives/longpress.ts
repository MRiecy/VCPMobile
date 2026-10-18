import type { Directive, DirectiveBinding } from 'vue';
import { MERMAID_CONTENT_SELECTOR } from '../utils/astRenderer';

const CONTROLS = 'input, textarea, select, a, button, summary, [role="button"], [role="link"], [contenteditable]:not([contenteditable="false"])';
const MESSAGE_CONTROLS = `${CONTROLS}, pre, [data-longpress-native], [data-vcp-button], [onclick], [oncontextmenu], [ontouchstart], [onpointerdown], video, audio, iframe`;

type Callback = (event: Event) => void;
type LongpressElement = HTMLElement & { _longpressCleanup?: () => void };

interface Options {
  callback: Callback | null;
  suppressClick: boolean;
  messageSurface: boolean;
}

interface TouchGesture {
  pointerId: number;
  x: number;
  y: number;
  target: Element;
  startEvent: PointerEvent;
  timer: number | null;
  fired: boolean;
  cancelled: boolean;
}

interface Controller {
  update(binding: DirectiveBinding): void;
  dispose(): void;
}

const controllers = new WeakMap<HTMLElement, Controller>();

function targetElement(event: Event): Element | null {
  return event.target instanceof Element ? event.target
    : event.target instanceof Node ? event.target.parentElement : null;
}

function ownsGesture(el: HTMLElement, event: Event, messageSurface: boolean): boolean {
  const target = targetElement(event);
  if (!target || (target !== el && !el.contains(target))) return false;
  // A button owns its own gesture even when another part of the page has selected text.
  if (el.matches('button, a, [role="button"]')) return true;

  const selection = window.getSelection();
  if (!messageSurface && selection && !selection.isCollapsed
    && ((selection.anchorNode && el.contains(selection.anchorNode))
      || (selection.focusNode && el.contains(selection.focusNode)))) return false;

  const owner = target.closest(messageSurface ? MESSAGE_CONTROLS
    : `${CONTROLS}, img, svg, canvas, video, audio, iframe, .select-text, .vcp-markdown-block, .ast-render-container`);
  if (owner && owner !== el && el.contains(owner)) return false;
  if (messageSurface) {
    // Generated media has its own handlers; avatars/header SVGs belong to the message.
    const content = target.closest('[data-message-content]');
    if (content && el.contains(content)) {
      const diagram = target.closest(MERMAID_CONTENT_SELECTOR);
      if (diagram && content.contains(diagram)) return false;
      const image = target.closest('img, image, svg, canvas');
      if (image && content.contains(image) && !image.classList.contains('vcp-emoticon')) return false;
    }
  }
  return true;
}

function readOptions(binding: DirectiveBinding): Options {
  const callback = typeof binding.value === 'function' ? binding.value as Callback : null;
  if (!callback) console.warn('v-longpress requires a function value');
  return {
    callback,
    suppressClick: binding.modifiers['suppress-click'] === true,
    messageSurface: binding.modifiers.message === true,
  };
}

function createController(el: HTMLElement, binding: DirectiveBinding): Controller {
  let options = readOptions(binding);
  let gesture: TouchGesture | null = null;
  let disposed = false;
  let suppressNextClick = false;
  let suppressionExpiry: number | null = null;

  const clearSuppression = () => {
    suppressNextClick = false;
    if (suppressionExpiry !== null) window.clearTimeout(suppressionExpiry);
    suppressionExpiry = null;
  };

  const expireSuppressionAfterRelease = () => {
    if (!suppressNextClick) return;
    if (suppressionExpiry !== null) window.clearTimeout(suppressionExpiry);
    suppressionExpiry = window.setTimeout(clearSuppression, 700);
  };

  const clearTimer = (current: TouchGesture) => {
    if (current.timer !== null) window.clearTimeout(current.timer);
    current.timer = null;
  };

  const stopTracking = () => {
    document.removeEventListener('pointerdown', onAdditionalPointer, true);
    document.removeEventListener('pointermove', onPointerMove, true);
    document.removeEventListener('pointerup', onPointerEnd, true);
    document.removeEventListener('pointercancel', onPointerEnd, true);
    document.removeEventListener('scroll', onScroll, true);
    document.removeEventListener('visibilitychange', onVisibilityChange);
    window.removeEventListener('blur', endGesture);
  };

  function endGesture() {
    if (gesture) clearTimer(gesture);
    gesture = null;
    stopTracking();
    expireSuppressionAfterRelease();
  }

  const cancelPending = () => {
    if (!gesture || gesture.fired) return;
    gesture.cancelled = true;
    clearTimer(gesture);
  };

  const fire = (current: TouchGesture, event: Event) => {
    if (disposed || gesture !== current || current.cancelled || current.fired || !options.callback
      || !ownsGesture(el, event, options.messageSurface)) return;
    clearTimer(current);
    current.fired = true;
    if (options.suppressClick) suppressNextClick = true;
    if (options.messageSurface) {
      const selection = window.getSelection();
      if (selection?.anchorNode && selection.focusNode
        && el.contains(selection.anchorNode) && el.contains(selection.focusNode)) selection.removeAllRanges();
    }
    options.callback(event);
  };

  function onAdditionalPointer(event: PointerEvent) {
    if (gesture && event.pointerType === 'touch' && event.pointerId !== gesture.pointerId) cancelPending();
  }

  function onPointerMove(event: PointerEvent) {
    if (!gesture || event.pointerId !== gesture.pointerId || event.pointerType !== 'touch') return;
    if (Math.hypot(event.clientX - gesture.x, event.clientY - gesture.y) > 10) cancelPending();
  }

  function onPointerEnd(event: PointerEvent) {
    if (gesture && event.pointerId === gesture.pointerId && event.pointerType === 'touch') endGesture();
  }

  function onScroll(event: Event) {
    if (!gesture || gesture.fired) return;
    // Only motion of the pressed content's scroll ancestry can cancel this hold.
    // Message-list auto-scroll must not cancel a sibling header button.
    if (event.target === document
      || (event.target instanceof Element && event.target.contains(gesture.target))) cancelPending();
  }

  function onVisibilityChange() {
    if (document.hidden) endGesture();
  }

  const onPointerDown = (event: PointerEvent) => {
    // This directive is a mobile touch interaction. Compatibility mouse events
    // emitted by Android around contextmenu never enter the gesture state machine.
    if (event.pointerType !== 'touch') return;
    if (!event.isPrimary) { cancelPending(); return; }
    endGesture();
    clearSuppression();
    if (!options.callback || !ownsGesture(el, event, options.messageSurface)) return;
    const target = targetElement(event);
    if (!target) return;
    const current: TouchGesture = {
      pointerId: event.pointerId, x: event.clientX, y: event.clientY,
      target, startEvent: event, timer: null, fired: false, cancelled: false,
    };
    gesture = current;
    current.timer = window.setTimeout(() => fire(current, current.startEvent), 600);
    document.addEventListener('pointerdown', onAdditionalPointer, { capture: true, passive: true });
    document.addEventListener('pointermove', onPointerMove, { capture: true, passive: true });
    document.addEventListener('pointerup', onPointerEnd, { capture: true, passive: true });
    document.addEventListener('pointercancel', onPointerEnd, { capture: true, passive: true });
    document.addEventListener('scroll', onScroll, { capture: true, passive: true });
    document.addEventListener('visibilitychange', onVisibilityChange);
    window.addEventListener('blur', endGesture);
  };

  const onContextMenu = (event: Event) => {
    const current = gesture;
    if (!current || current.cancelled || !ownsGesture(el, event, options.messageSurface)) return;
    event.preventDefault();
    // Android can emit this before our timer. The current gesture owns both
    // signals, so either ordering invokes the action once. System contextmenu
    // pointer IDs need not match the touch pointer that is still held.
    fire(current, event);
  };

  const onClickCapture = (event: MouseEvent) => {
    if (!suppressNextClick) return;
    clearSuppression();
    event.preventDefault();
    event.stopPropagation();
    event.stopImmediatePropagation();
  };

  const onSelectStart = (event: Event) => {
    if (options.messageSurface && ownsGesture(el, event, true)) event.preventDefault();
  };

  el.addEventListener('pointerdown', onPointerDown, { passive: true });
  el.addEventListener('contextmenu', onContextMenu);
  el.addEventListener('click', onClickCapture, true);
  el.addEventListener('selectstart', onSelectStart);

  return {
    update(nextBinding) {
      const next = readOptions(nextBinding);
      if (!next.callback || next.messageSurface !== options.messageSurface
        || next.suppressClick !== options.suppressClick) {
        endGesture();
        clearSuppression();
      }
      options = next;
    },
    dispose() {
      if (disposed) return;
      disposed = true;
      endGesture();
      clearSuppression();
      el.removeEventListener('pointerdown', onPointerDown);
      el.removeEventListener('contextmenu', onContextMenu);
      el.removeEventListener('click', onClickCapture, true);
      el.removeEventListener('selectstart', onSelectStart);
    },
  };
}

export const vLongpress: Directive = {
  mounted(el: LongpressElement, binding: DirectiveBinding) {
    // Also retires an old installation if the same element is mounted again during development.
    el._longpressCleanup?.();
    const controller = createController(el, binding);
    controllers.set(el, controller);
    el._longpressCleanup = controller.dispose;
  },
  updated(el: LongpressElement, binding: DirectiveBinding) {
    controllers.get(el)?.update(binding);
  },
  unmounted(el: LongpressElement) {
    el._longpressCleanup?.();
    delete el._longpressCleanup;
    controllers.delete(el);
  },
};

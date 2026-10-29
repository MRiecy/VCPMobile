import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, disposePinia, setActivePinia, type Pinia } from 'pinia';
import { defineComponent, h, nextTick, reactive, ref } from 'vue';
import { mount, type VueWrapper } from '@vue/test-utils';
import { useSidebarSwipe, type SidebarSwipeType } from '../../../core/composables/useSidebarSwipe';
import { useLayoutStore } from '../../../core/stores/layout';

const context = vi.hoisted(() => ({
  voice: {} as { active: boolean },
  registerModal: vi.fn(),
  unregisterModal: vi.fn(),
}));

vi.mock('../../../features/chat/voice/voiceCallStore', () => ({
  useVoiceCallStore: () => context.voice,
}));
vi.mock('../../../core/composables/useModalHistory', () => ({
  useModalHistory: () => ({
    registerModal: context.registerModal,
    unregisterModal: context.unregisterModal,
  }),
}));

let pinia: Pinia;
let wrapper: VueWrapper | undefined;
let viewportWidth: number;
let frameId: number;
let frames: Map<number, FrameRequestCallback>;

beforeEach(() => {
  vi.useFakeTimers();
  pinia = createPinia();
  setActivePinia(pinia);
  context.voice = reactive({ active: false });
  viewportWidth = 400;
  frameId = 0;
  frames = new Map();
  vi.spyOn(window, 'requestAnimationFrame').mockImplementation(callback => {
    frames.set(++frameId, callback);
    return frameId;
  });
  vi.spyOn(window, 'cancelAnimationFrame').mockImplementation(id => {
    frames.delete(id);
  });
  vi.spyOn(window, 'matchMedia').mockImplementation(query => {
    const events = new EventTarget();
    return {
      get matches() {
        const minimum = /min-width:\s*(\d+)px/.exec(query);
        return minimum !== null && viewportWidth >= Number(minimum[1]);
      },
      media: query,
      onchange: null,
      addListener: vi.fn(),
      removeListener: vi.fn(),
      addEventListener: events.addEventListener.bind(events),
      removeEventListener: events.removeEventListener.bind(events),
      dispatchEvent: events.dispatchEvent.bind(events),
    };
  });
});

afterEach(() => {
  wrapper?.unmount();
  wrapper = undefined;
  disposePinia(pinia);
  vi.useRealTimers();
});

async function renderFrame() {
  const pending = [...frames.values()];
  frames.clear();
  for (const callback of pending) callback(performance.now());
  await nextTick();
}

function point(target: Element, x: number, y = 100, identifier = 1): Touch {
  return {
    identifier, target, clientX: x, clientY: y,
    screenX: x, screenY: y, pageX: x, pageY: y,
    radiusX: 1, radiusY: 1, rotationAngle: 0, force: 1,
  };
}

function touch(
  target: Element,
  type: 'touchstart' | 'touchmove' | 'touchend' | 'touchcancel',
  x: number,
  time: number,
  y = 100,
  extraTouches: Touch[] = [],
  cancelable = true,
) {
  const current = point(target, x, y);
  const touches = type === 'touchend' || type === 'touchcancel' ? [] : [current, ...extraTouches];
  const event = new TouchEvent(type, {
    bubbles: true,
    cancelable,
    touches,
    targetTouches: touches,
    changedTouches: [current],
  });
  Object.defineProperty(event, 'timeStamp', { value: time });
  target.dispatchEvent(event);
  return event;
}

async function mountSurface(type: SidebarSwipeType = 'left') {
  const layout = useLayoutStore();
  const onTabSwitch = vi.fn();
  const onClick = vi.fn();
  if (type === 'left') layout.setLeftDrawer(true);
  if (type === 'right') layout.setRightDrawer(true);
  wrapper = mount(defineComponent({
    setup() {
      const surface = ref<HTMLElement | null>(null);
      useSidebarSwipe(surface, { type, onTabSwitch });
      return () => h('div', [
        h('div', { class: 'vcp-drawer-overlay' }),
        h('div', {
          ref: surface,
          class: ['gesture-surface', type === 'global' ? '' : `vcp-drawer vcp-drawer-${type}`],
        }, [
          h('div', { class: 'vcp-scrollable' }, [h('div', { class: 'row', onClick }, 'Topic')]),
          h('input', { class: 'editor' }),
          h('div', { class: 'no-swipe' }, [h('span', { class: 'excluded-child' }, 'Excluded')]),
          h('div', { contenteditable: 'true', class: 'editable' }, [h('span', 'Text')]),
          h('div', { class: 'overflow-x-auto' }, [h('span', { class: 'horizontal-content' }, 'Wide')]),
        ]),
      ]);
    },
  }), { attachTo: document.body });
  await nextTick();
  const surface = wrapper.get('.gesture-surface').element as HTMLElement;
  const overlay = wrapper.get('.vcp-drawer-overlay').element as HTMLElement;
  const row = wrapper.get('.row').element as HTMLElement;
  Object.defineProperties(surface, {
    offsetWidth: { configurable: true, value: 300 },
    clientWidth: { configurable: true, value: 300 },
  });
  vi.spyOn(surface.parentElement!, 'getBoundingClientRect').mockReturnValue(new DOMRect(0, 0, 400, 800));
  vi.spyOn(surface, 'getBoundingClientRect').mockReturnValue(new DOMRect(type === 'right' ? 100 : 0, 0, 300, 800));
  const isOpen = () => type === 'right' ? layout.rightDrawerOpen : layout.leftDrawerOpen;
  return { surface, overlay, row, layout, isOpen, onClick, onTabSwitch };
}

function translationX(surface: HTMLElement) {
  const value = /translate(?:3d|X)?\(\s*(-?[\d.]+)/.exec(surface.style.transform);
  return value === null ? 0 : Number(value[1]);
}

async function finishMotion(surface: HTMLElement) {
  await nextTick();
  await renderFrame();
  await renderFrame();
  const event = new Event('transitionend', { bubbles: true });
  Object.defineProperty(event, 'propertyName', { value: 'transform' });
  surface.dispatchEvent(event);
  await vi.advanceTimersByTimeAsync(1000);
  await renderFrame();
  await renderFrame();
}

describe('interactive sidebar closing', () => {
  it.each([
    ['left', 200, 110, -90],
    ['right', 100, 190, 90],
  ] as const)('follows a %s drawer before release and fades its shared overlay', async (type, start, end, displacement) => {
    const { surface, overlay, row, isOpen } = await mountSurface(type);
    touch(row, 'touchstart', start, 0);
    const move = touch(row, 'touchmove', end, 300);
    await renderFrame();

    expect(translationX(surface)).toBe(displacement);
    expect(Number(overlay.style.opacity)).toBeCloseTo(0.7);
    expect(move.defaultPrevented).toBe(true);
    expect(isOpen()).toBe(true);

    touch(row, 'touchend', end, 400);
    await nextTick();
    expect(isOpen()).toBe(false);
  });

  it('returns a short slow drag to the open position and retains normal taps', async () => {
    const { surface, row, overlay, isOpen, onClick } = await mountSurface();
    row.click();
    expect(onClick).toHaveBeenCalledOnce();
    touch(row, 'touchstart', 200, 0);
    touch(row, 'touchmove', 150, 300);
    await renderFrame();
    expect(translationX(surface)).toBe(-50);
    touch(row, 'touchend', 150, 500);
    row.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true, detail: 1 }));
    expect(onClick).toHaveBeenCalledOnce();
    await finishMotion(surface);
    expect(isOpen()).toBe(true);
    expect(translationX(surface)).toBe(0);
    expect(overlay.style.opacity === '' || Number(overlay.style.opacity) === 1).toBe(true);

    touch(row, 'touchstart', 200, 700);
    touch(row, 'touchend', 200, 710);
    row.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true, detail: 1 }));
    expect(onClick).toHaveBeenCalledTimes(2);
  });

  it.each([
    ['left', -40, 200, 180, -60],
    ['right', 140, 100, 120, 60],
  ] as const)('uses the visible %s drawer position when direction locks, after an animation advances under the held finger', async (type, left, start, end, displacement) => {
    const { surface, overlay, row, isOpen } = await mountSurface(type);
    const initialLeft = type === 'left' ? left - 60 : left + 60;
    vi.mocked(surface.getBoundingClientRect).mockReturnValue(new DOMRect(initialLeft, 0, 300, 800));
    touch(row, 'touchstart', start, 0);
    // Entry or rebound continues while a stationary finger has not claimed a direction.
    vi.mocked(surface.getBoundingClientRect).mockReturnValue(new DOMRect(left, 0, 300, 800));
    touch(row, 'touchmove', end, 200);
    await renderFrame();
    expect(translationX(surface)).toBe(displacement);
    expect(Number(overlay.style.opacity)).toBeCloseTo(0.8);
    touch(row, 'touchend', end, 400);
    await finishMotion(surface);
    expect(isOpen()).toBe(true);
  });

  it('includes the final changed touch position when a slow release crosses the close distance', async () => {
    const { surface, row, isOpen } = await mountSurface();
    touch(row, 'touchstart', 200, 0);
    touch(row, 'touchmove', 150, 300);
    await renderFrame();
    expect(translationX(surface)).toBe(-50);
    expect(isOpen()).toBe(true);

    // The final 30px arrives only in changedTouches; the 300ms interval is not a fling.
    touch(row, 'touchend', 120, 600);
    expect(isOpen()).toBe(false);
  });

  it('immediately releases an active drag when the browser makes touchmove non-cancelable', async () => {
    const { surface, overlay, row, isOpen } = await mountSurface();
    touch(row, 'touchstart', 200, 0);
    touch(row, 'touchmove', 160, 200);
    await renderFrame();
    expect(translationX(surface)).toBe(-40);

    const move = touch(row, 'touchmove', 80, 250, 100, [], false);
    expect(move.defaultPrevented).toBe(false);
    expect(surface.style.transform).toBe('');
    expect(overlay.style.opacity).toBe('');
    expect(isOpen()).toBe(true);
    touch(row, 'touchend', 80, 300);
    await renderFrame();
    expect(isOpen()).toBe(true);
    expect(surface.style.transform).toBe('');
  });

  it('closes a short fast fling but does not reuse that speed after a pause', async () => {
    const { surface, row, layout, isOpen } = await mountSurface();
    touch(row, 'touchstart', 200, 0);
    touch(row, 'touchmove', 170, 30);
    touch(row, 'touchend', 160, 40);
    await nextTick();
    expect(isOpen()).toBe(false);
    await finishMotion(surface);

    layout.setLeftDrawer(true);
    await nextTick();
    touch(row, 'touchstart', 200, 1000);
    touch(row, 'touchmove', 160, 1040);
    await renderFrame();
    touch(row, 'touchend', 160, 1250);
    await finishMotion(surface);
    expect(isOpen()).toBe(true);
  });

  it('locks a vertical scroll and never steals it after later horizontal movement', async () => {
    const { surface, row, isOpen } = await mountSurface();
    touch(row, 'touchstart', 200, 0);
    const vertical = touch(row, 'touchmove', 198, 20, 125);
    const horizontal = touch(row, 'touchmove', 90, 200, 130);
    await renderFrame();
    touch(row, 'touchend', 90, 250, 130);
    expect(vertical.defaultPrevented).toBe(false);
    expect(horizontal.defaultPrevented).toBe(false);
    expect(translationX(surface)).toBe(0);
    expect(isOpen()).toBe(true);
  });

  it.each(['.editor', '.excluded-child', '.editable span', '.horizontal-content'])('leaves %s gestures with their owner', async selector => {
    const { surface, isOpen } = await mountSurface();
    const target = wrapper!.get(selector).element;
    touch(target, 'touchstart', 200, 0);
    const move = touch(target, 'touchmove', 80, 200);
    await renderFrame();
    touch(target, 'touchend', 80, 250);
    expect(move.defaultPrevented).toBe(false);
    expect(translationX(surface)).toBe(0);
    expect(isOpen()).toBe(true);
  });

  it('respects a child that consumes its own swipe and accepts the next independent gesture', async () => {
    const { surface, row, isOpen } = await mountSurface();
    const consume = (event: Event) => event.stopPropagation();
    row.addEventListener('touchmove', consume);
    row.addEventListener('touchend', consume);
    touch(row, 'touchstart', 200, 0);
    touch(row, 'touchmove', 80, 200);
    touch(row, 'touchend', 80, 250);
    await renderFrame();
    expect(translationX(surface)).toBe(0);
    expect(isOpen()).toBe(true);
    row.removeEventListener('touchmove', consume);
    row.removeEventListener('touchend', consume);
    touch(row, 'touchstart', 200, 1000);
    touch(row, 'touchmove', 80, 1200);
    touch(row, 'touchend', 80, 1250);
    expect(isOpen()).toBe(false);
  });

  it('cleans up a drag after a child consumes touchend without committing a close for it', async () => {
    const { surface, overlay, row, isOpen } = await mountSurface();
    touch(row, 'touchstart', 200, 0);
    touch(row, 'touchmove', 100, 200);
    await renderFrame();
    expect(translationX(surface)).toBe(-100);
    row.addEventListener('touchend', event => event.stopPropagation(), { once: true });
    touch(row, 'touchend', 100, 250);
    await vi.advanceTimersByTimeAsync(0);
    await renderFrame();
    expect(isOpen()).toBe(true);
    expect(surface.style.transform).toBe('');
    expect(overlay.style.opacity).toBe('');
    expect(frames.size).toBe(0);
  });

  it('cancels touchcancel and multi-touch without committing a close', async () => {
    const { surface, row, isOpen } = await mountSurface();
    touch(row, 'touchstart', 200, 0);
    touch(row, 'touchmove', 100, 200);
    await renderFrame();
    touch(row, 'touchcancel', 100, 220);
    await finishMotion(surface);
    expect(isOpen()).toBe(true);
    expect(translationX(surface)).toBe(0);

    touch(row, 'touchstart', 200, 1000);
    touch(row, 'touchmove', 100, 1200);
    await renderFrame();
    touch(row, 'touchstart', 100, 1220, 100, [point(row, 140, 110, 2)]);
    touch(row, 'touchmove', 70, 1240);
    touch(row, 'touchend', 70, 1260);
    await finishMotion(surface);
    expect(isOpen()).toBe(true);
    expect(translationX(surface)).toBe(0);
  });

  it.each(['voice', 'resize', 'external-close'] as const)('discards an in-flight drag interrupted by %s', async reason => {
    const { surface, overlay, row, layout, isOpen } = await mountSurface();
    touch(row, 'touchstart', 200, 0);
    touch(row, 'touchmove', 100, 200);
    await renderFrame();
    if (reason === 'voice') context.voice.active = true;
    if (reason === 'resize') {
      viewportWidth = 1100;
      window.dispatchEvent(new Event('resize'));
    }
    if (reason === 'external-close') layout.setLeftDrawer(false);
    await nextTick();
    await finishMotion(surface);
    context.voice.active = false;
    viewportWidth = 400;
    window.dispatchEvent(new Event('resize'));
    await nextTick();
    touch(row, 'touchend', 50, 500);
    expect(isOpen()).toBe(reason !== 'external-close');
    expect(surface.style.transform).toBe('');
    expect(overlay.style.opacity).toBe('');
  });

  it('releases pending frames and listeners on unmount', async () => {
    const { surface, row, overlay, layout } = await mountSurface();
    touch(row, 'touchstart', 200, 0);
    touch(row, 'touchmove', 100, 200);
    wrapper!.unmount();
    wrapper = undefined;
    await renderFrame();
    touch(row, 'touchend', 100, 250);
    touch(surface, 'touchstart', 200, 1000);
    touch(surface, 'touchmove', 50, 1200);
    touch(surface, 'touchend', 50, 1250);
    await finishMotion(surface);
    expect(layout.leftDrawerOpen).toBe(true);
    expect(surface.style.transform).toBe('');
    expect(overlay.style.opacity).toBe('');
    expect(frames.size).toBe(0);
  });

  it.each([
    ['left', 1100, false],
    ['right', 1100, true],
    ['right', 1300, false],
  ] as const)('only drags an overlay %s drawer at viewport width %i', async (type, width, canClose) => {
    viewportWidth = width;
    const { surface, row, isOpen } = await mountSurface(type);
    const end = type === 'left' ? 80 : 320;
    touch(row, 'touchstart', 200, 0);
    touch(row, 'touchmove', end, 200);
    await renderFrame();
    if (!canClose) expect(translationX(surface)).toBe(0);
    touch(row, 'touchend', end, 250);
    expect(isOpen()).toBe(!canClose);
  });

  it('retains right-swipe tab switching without moving the left drawer', async () => {
    const { surface, row, onTabSwitch, isOpen } = await mountSurface();
    touch(row, 'touchstart', 100, 0);
    touch(row, 'touchmove', 180, 200);
    await renderFrame();
    touch(row, 'touchend', 180, 250);
    expect(translationX(surface)).toBe(0);
    expect(isOpen()).toBe(true);
    expect(onTabSwitch).toHaveBeenCalledOnce();
  });
});

describe('global opening gestures', () => {
  it.each([
    [20, 110, 'leftDrawerOpen'],
    [200, 110, 'rightDrawerOpen'],
  ] as const)('opens after release from %i to %i', async (start, end, property) => {
    const { surface, layout } = await mountSurface('global');
    touch(surface, 'touchstart', start, 0);
    touch(surface, 'touchmove', end, 200);
    await renderFrame();
    expect(layout[property]).toBe(false);
    touch(surface, 'touchend', end, 250);
    expect(layout[property]).toBe(true);
  });

  it('retains scroll-surface exclusion and ignores a gesture interrupted by a call', async () => {
    const { surface, row, layout } = await mountSurface('global');
    touch(row, 'touchstart', 20, 0);
    touch(row, 'touchmove', 110, 200);
    touch(row, 'touchend', 110, 250);
    expect(layout.leftDrawerOpen).toBe(false);

    touch(surface, 'touchstart', 20, 1000);
    touch(surface, 'touchmove', 110, 1200);
    context.voice.active = true;
    await nextTick();
    context.voice.active = false;
    await nextTick();
    touch(surface, 'touchend', 110, 1250);
    expect(layout.leftDrawerOpen).toBe(false);
  });
});

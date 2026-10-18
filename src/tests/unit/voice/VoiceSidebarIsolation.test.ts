import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, disposePinia, setActivePinia, type Pinia } from 'pinia';
import { defineComponent, h, nextTick, reactive, ref } from 'vue';
import { mount, type VueWrapper } from '@vue/test-utils';
import { useLayoutStore } from '../../../core/stores/layout';
import { useSidebarSwipe } from '../../../core/composables/useSidebarSwipe';

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

beforeEach(() => {
  pinia = createPinia();
  setActivePinia(pinia);
  context.voice = reactive({ active: false });
});

afterEach(() => {
  wrapper?.unmount();
  wrapper = undefined;
  disposePinia(pinia);
});

const mountSwipeSurface = async () => {
  wrapper = mount(defineComponent({
    setup() {
      const surface = ref<HTMLElement | null>(null);
      useSidebarSwipe(surface, { type: 'global' });
      return () => h('div', { ref: surface });
    },
  }), { attachTo: document.body });
  await nextTick();
  return wrapper.element;
};

const touch = (surface: Element, type: string, x: number) => {
  const event = new Event(type, { bubbles: true });
  Object.defineProperty(event, 'touches', {
    value: type === 'touchend' ? [] : [{ clientX: x, clientY: 30 }],
  });
  surface.dispatchEvent(event);
};

const swipeRight = (surface: Element) => {
  touch(surface, 'touchstart', 20);
  touch(surface, 'touchmove', 110);
  touch(surface, 'touchend', 110);
};

describe('voice call sidebar isolation', () => {
  it('refuses new drawers during a call while retaining the close path', () => {
    const layout = useLayoutStore();
    layout.setLeftDrawer(true);
    expect(layout.leftDrawerOpen).toBe(true);

    context.voice.active = true;
    layout.setLeftDrawer(false);
    layout.setLeftDrawer(true);
    layout.toggleRightDrawer();
    expect(layout.leftDrawerOpen).toBe(false);
    expect(layout.rightDrawerOpen).toBe(false);
    expect(context.registerModal).toHaveBeenCalledTimes(1);
    expect(context.unregisterModal).toHaveBeenCalledWith('LeftDrawer');

    context.voice.active = false;
    layout.setRightDrawer(true);
    expect(layout.rightDrawerOpen).toBe(true);
  });

  it('does not start a sidebar gesture while the call is active and resumes after hangup', async () => {
    const layout = useLayoutStore();
    context.voice.active = true;
    const surface = await mountSwipeSurface();
    swipeRight(surface);
    expect(layout.leftDrawerOpen).toBe(false);

    context.voice.active = false;
    await nextTick();
    swipeRight(surface);
    expect(layout.leftDrawerOpen).toBe(true);
  });

  it('discards a gesture interrupted by the call even if its touchend arrives after hangup', async () => {
    const layout = useLayoutStore();
    const surface = await mountSwipeSurface();
    touch(surface, 'touchstart', 20);
    touch(surface, 'touchmove', 110);
    context.voice.active = true;
    await nextTick();
    context.voice.active = false;
    await nextTick();
    touch(surface, 'touchend', 110);
    expect(layout.leftDrawerOpen).toBe(false);

    swipeRight(surface);
    expect(layout.leftDrawerOpen).toBe(true);
  });
});

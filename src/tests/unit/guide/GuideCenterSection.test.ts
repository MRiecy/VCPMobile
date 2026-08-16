import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia, getActivePinia } from 'pinia';
import { mount } from '@vue/test-utils';
import '@/features/guide/guides';
import GuideCenterSection from '@/features/guide/components/GuideCenterSection.vue';
import { useGuideStore } from '@/features/guide/stores/guideStore';
import { useOverlayStore } from '@/core/stores/overlay';
import { useNotificationStore } from '@/core/stores/notification';

vi.mock('@tauri-apps/api/app', () => ({
  getVersion: vi.fn(() => Promise.resolve('1.1.4')),
}));

beforeEach(() => {
  setActivePinia(createPinia());
});

afterEach(() => {
  vi.useRealTimers();
  const pinia = getActivePinia() as unknown as { _e?: { stop: () => void } } | null;
  pinia?._e?.stop();
});

describe('GuideCenterSection', () => {
  it('lists all registered guides with completion status', () => {
    const guideStore = useGuideStore();
    guideStore.completed = ['sidebar-gestures'];
    const wrapper = mount(GuideCenterSection);

    const entries = wrapper.findAll('.guide-entry');
    expect(entries).toHaveLength(4);
    expect(wrapper.text()).toContain('已完成');
    expect(wrapper.text()).toContain('未完成');
    expect(wrapper.find('.guide-reset').exists()).toBe(true);
  });

  it('resets progress through the confirm flow and toasts', async () => {
    const guideStore = useGuideStore();
    guideStore.completed = ['sidebar-gestures', 'theme-longpress', 'plus-longpress', 'diary-longpress'];
    const overlayStore = useOverlayStore();
    const confirmSpy = vi.spyOn(overlayStore, 'showConfirm').mockResolvedValue(true);
    const notificationStore = useNotificationStore();

    const wrapper = mount(GuideCenterSection);
    await wrapper.find('.guide-reset').trigger('click');

    expect(confirmSpy).toHaveBeenCalledTimes(1);
    expect(confirmSpy.mock.calls[0]?.[0]?.isDanger).toBe(true);
    expect(guideStore.completed).toEqual([]);
    expect(notificationStore.activeToasts.length).toBeGreaterThan(0);
  });

  it('keeps progress untouched when the confirm is cancelled', async () => {
    const guideStore = useGuideStore();
    guideStore.completed = ['sidebar-gestures'];
    const overlayStore = useOverlayStore();
    vi.spyOn(overlayStore, 'showConfirm').mockResolvedValue(false);

    const wrapper = mount(GuideCenterSection);
    await wrapper.find('.guide-reset').trigger('click');

    expect(guideStore.completed).toEqual(['sidebar-gestures']);
  });
});

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia, getActivePinia } from 'pinia';
import { mount } from '@vue/test-utils';
import '@/features/guide/guides';
import GuideCenterSection from '@/features/guide/components/GuideCenterSection.vue';
import { useGuideStore } from '@/features/guide/stores/guideStore';
import { useOverlayStore } from '@/core/stores/overlay';
import { useLayoutStore } from '@/core/stores/layout';
import { useNotificationStore } from '@/core/stores/notification';
import { sidebarTab } from '@/features/agent/sidebarTab';

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

  it('resets progress and returns to a triggerable environment', async () => {
    const guideStore = useGuideStore();
    guideStore.completed = ['sidebar-gestures', 'theme-longpress', 'plus-longpress', 'diary-longpress'];
    const overlayStore = useOverlayStore();
    const confirmSpy = vi.spyOn(overlayStore, 'showConfirm').mockResolvedValue(true);
    const notificationStore = useNotificationStore();
    // 模拟从设置页（页面栈非空）且右抽屉打开的状态执行重置
    overlayStore.pageStack = [{ type: 'settings', modalId: 'Page:settings:' }] as never;
    const layoutStore = useLayoutStore();
    layoutStore.setRightDrawer(true);
    sidebarTab.value = 'topics';

    const wrapper = mount(GuideCenterSection);
    await wrapper.find('.guide-reset').trigger('click');

    expect(confirmSpy).toHaveBeenCalledTimes(1);
    expect(confirmSpy.mock.calls[0]?.[0]?.isDanger).toBe(true);
    expect(guideStore.completed).toEqual([]);
    // 回到可触发环境：清空页面栈 + 左抽屉打开（自动互斥关闭右抽屉）+ 助理 Tab
    expect(overlayStore.pageStack).toEqual([]);
    expect(layoutStore.leftDrawerOpen).toBe(true);
    expect(layoutStore.rightDrawerOpen).toBe(false);
    expect(sidebarTab.value).toBe('agents');
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

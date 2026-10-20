import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createApp, nextTick } from 'vue';
import { createPinia, disposePinia, setActivePinia, type Pinia } from 'pinia';
import piniaPluginPersistedstate from 'pinia-plugin-persistedstate';
import { mount, type VueWrapper } from '@vue/test-utils';

const platform = vi.hoisted(() => ({ isAndroid: true, isIOS: false }));
vi.mock('@/core/utils/platform', () => platform);
vi.mock('@tauri-apps/api/app', () => ({
  getVersion: vi.fn(() => Promise.resolve('1.1.4')),
}));

const guideId = 'android-webview-compatibility';
const piniaInstances: Pinia[] = [];
const wrappers: VueWrapper[] = [];

function createStoreScope() {
  const pinia = createPinia();
  pinia.use(piniaPluginPersistedstate);
  createApp({}).use(pinia);
  setActivePinia(pinia);
  piniaInstances.push(pinia);
  return pinia;
}

beforeEach(() => {
  vi.resetModules();
  vi.useFakeTimers();
  localStorage.clear();
  platform.isAndroid = true;
  platform.isIOS = false;
  createStoreScope();
});

afterEach(() => {
  wrappers.splice(0).forEach(wrapper => wrapper.unmount());
  piniaInstances.splice(0).forEach(pinia => disposePinia(pinia));
  vi.clearAllTimers();
  vi.useRealTimers();
});

describe('Android WebView compatibility guide', () => {
  it('waits for READY, remembers acknowledgment across stores and remains replayable from help', async () => {
    await import('@/features/guide/guides');
    const { useGuideStore, DEFAULT_SETTLE_MS, REPLAY_PREPARE_DELAY_MS } = await import('@/features/guide/stores/guideStore');
    const { useAppLifecycleStore } = await import('@/core/stores/appLifecycle');
    const { useOverlayStore } = await import('@/core/stores/overlay');
    const { default: GuideCenterSection } = await import('@/features/guide/components/GuideCenterSection.vue');
    const lifecycle = useAppLifecycleStore();
    const store = useGuideStore();

    lifecycle.state = 'PERMISSIONS';
    store.init();
    await nextTick();
    vi.advanceTimersByTime(DEFAULT_SETTLE_MS * 2);
    expect(store.activeGuideId).toBeNull();
    lifecycle.state = 'PRELOADING';
    await nextTick();
    vi.advanceTimersByTime(DEFAULT_SETTLE_MS * 2);
    expect(store.activeGuideId).toBeNull();
    lifecycle.state = 'READY';
    await nextTick();
    vi.advanceTimersByTime(DEFAULT_SETTLE_MS);
    expect(store.activeGuideId).toBe(guideId);
    expect(store.activeStep?.target).toBeNull();
    store.finish();
    await nextTick();
    expect(store.isCompleted(guideId)).toBe(true);

    disposePinia(piniaInstances.shift()!);
    createStoreScope();
    const restored = useGuideStore();
    useAppLifecycleStore().state = 'READY';
    restored.init();
    await nextTick();
    vi.advanceTimersByTime(DEFAULT_SETTLE_MS * 2);
    expect(restored.isCompleted(guideId)).toBe(true);
    expect(restored.activeGuideId).toBeNull();

    useOverlayStore().openSettings();
    const wrapper = mount(GuideCenterSection);
    wrappers.push(wrapper);
    const entry = wrapper.findAll('.guide-entry').find(item => item.text().includes('Android WebView 兼容说明'));
    expect(entry).toBeDefined();
    expect(entry!.text()).toContain('已完成');
    await entry!.get('button').trigger('click');
    expect(useOverlayStore().isSettingsOpen).toBe(false);
    vi.advanceTimersByTime(REPLAY_PREPARE_DELAY_MS);
    expect(restored.activeGuideId).toBe(guideId);
    restored.finish();
  });

  it('does not register or list the Android notice on iOS', async () => {
    platform.isAndroid = false;
    platform.isIOS = true;
    await import('@/features/guide/guides');
    const { getGuide } = await import('@/features/guide/registry');
    const { useGuideStore, DEFAULT_SETTLE_MS } = await import('@/features/guide/stores/guideStore');
    const { useAppLifecycleStore } = await import('@/core/stores/appLifecycle');
    const { default: GuideCenterSection } = await import('@/features/guide/components/GuideCenterSection.vue');
    const store = useGuideStore();
    useAppLifecycleStore().state = 'READY';
    store.init();
    await nextTick();
    vi.advanceTimersByTime(DEFAULT_SETTLE_MS * 2);
    expect(getGuide(guideId)).toBeUndefined();
    expect(store.activeGuideId).toBeNull();
    const wrapper = mount(GuideCenterSection);
    wrappers.push(wrapper);
    expect(wrapper.text()).not.toContain('Android WebView');
  });
});

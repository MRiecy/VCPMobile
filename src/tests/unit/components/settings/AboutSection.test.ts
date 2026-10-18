import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { nextTick } from 'vue';
import { createPinia, setActivePinia } from 'pinia';
import { mount, type VueWrapper } from '@vue/test-utils';
import AboutSection from '@/features/settings/components/AboutSection.vue';
import { useDeveloperSettingsStore } from '@/features/settings/developer/store';

const actions = vi.hoisted(() => ({ openUrl: vi.fn(), notify: vi.fn() }));
vi.mock('/vcpmobile.svg', () => ({ default: '/vcpmobile.svg' }));
vi.mock('@tauri-apps/api/app', () => ({ getVersion: vi.fn().mockResolvedValue('1.1.6') }));
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: actions.openUrl }));
vi.mock('@/core/stores/theme', () => ({ useThemeStore: () => ({ isDarkResolved: true }) }));
vi.mock('@/core/stores/notification', () => ({ useNotificationStore: () => ({ addNotification: actions.notify }) }));

describe('AboutSection page animation lifetime', () => {
  let wrapper: VueWrapper | null;

  beforeEach(async () => {
    const storage = new Map<string, string>();
    vi.stubGlobal('localStorage', {
      getItem: vi.fn((key: string) => storage.get(key) ?? null),
      setItem: vi.fn((key: string, value: string) => { storage.set(key, value); }),
      clear: vi.fn(() => storage.clear()),
    });
    setActivePinia(createPinia());
    vi.useFakeTimers();
    wrapper = mount(AboutSection, { global: { stubs: { UpdateSection: true } } });
    await vi.advanceTimersByTimeAsync(0);
  });

  afterEach(() => {
    wrapper?.unmount();
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  async function tapFeatures(times = 1, intervalMs = 100) {
    for (let index = 0; index < times; index += 1) {
      await wrapper!.findAll('.settings-row').find((row) => row.text().includes('功能介绍'))!.trigger('click');
      await vi.advanceTimersByTimeAsync(intervalMs);
    }
  }

  it('unlocks on six consecutive taps without navigating until the next tap', async () => {
    const developer = useDeveloperSettingsStore();
    await tapFeatures(5);
    expect(developer.enabled).toBe(false);
    expect(actions.notify).toHaveBeenCalledTimes(1);
    await tapFeatures();
    expect(developer.enabled).toBe(true);
    expect(localStorage.getItem('vcp-developer-mode')).toBe('true');
    expect(wrapper!.emitted('open-developer')).toBeUndefined();
    expect(actions.notify).toHaveBeenLastCalledWith(expect.objectContaining({ type: 'success', title: '已开启开发者模式' }));
    await tapFeatures();
    expect(wrapper!.emitted('open-developer')).toHaveLength(1);
  });

  it('resets a slow sequence and accepts successive taps up to 1.2 seconds apart', async () => {
    const developer = useDeveloperSettingsStore();
    await tapFeatures(5, 0);
    await vi.advanceTimersByTimeAsync(1201);
    await tapFeatures(1, 1200);
    expect(developer.enabled).toBe(false);
    await tapFeatures(5, 1200);
    expect(developer.enabled).toBe(true);
    expect(wrapper!.emitted('open-developer')).toBeUndefined();
  });

  it('clears the tap sequence when the about page becomes inactive', async () => {
    const developer = useDeveloperSettingsStore();
    await tapFeatures(5);
    await wrapper!.setProps({ active: false });
    await tapFeatures(2);
    await wrapper!.setProps({ active: true });
    await tapFeatures();
    expect(developer.enabled).toBe(false);
    await tapFeatures(5);
    expect(developer.enabled).toBe(true);
  });

  it('keeps the entry locked when persistence fails and permits a later fresh attempt', async () => {
    const developer = useDeveloperSettingsStore();
    const write = vi.spyOn(localStorage, 'setItem').mockImplementation(() => { throw new Error('storage unavailable'); });
    await tapFeatures(6);
    expect(developer.enabled).toBe(false);
    expect(actions.notify).toHaveBeenLastCalledWith(expect.objectContaining({ type: 'error', title: '开发者模式开启失败' }));
    await tapFeatures();
    expect(wrapper!.emitted('open-developer')).toBeUndefined();
    write.mockRestore();
    await tapFeatures(5);
    expect(developer.enabled).toBe(true);
    expect(wrapper!.emitted('open-developer')).toBeUndefined();
  });

  function logo() {
    return wrapper!.get('img[alt="VCPMobile"]').element.parentElement!;
  }

  async function pressLogo() {
    logo().parentElement!.dispatchEvent(new MouseEvent('mousedown', { bubbles: true }));
    await nextTick();
  }

  it('defaults to active and cancels the initial wait when the page hides', async () => {
    const initialStyle = logo().getAttribute('style');
    await pressLogo();
    expect(vi.getTimerCount()).toBeGreaterThan(0);
    await wrapper!.setProps({ active: false });
    await vi.advanceTimersByTimeAsync(30_000);
    expect(logo().getAttribute('style')).toBe(initialStyle);
    expect(vi.getTimerCount()).toBe(0);
  });

  it('stops a sequence already in motion and rejects hidden interactions', async () => {
    await pressLogo();
    await vi.advanceTimersByTimeAsync(800);
    expect(logo().getAttribute('style')).toContain('rotateY(180deg)');
    await wrapper!.setProps({ active: false });
    const stoppedStyle = logo().getAttribute('style');
    const hitbox = logo().parentElement!;
    hitbox.dispatchEvent(new MouseEvent('mousedown', { bubbles: true }));
    hitbox.dispatchEvent(new MouseEvent('mousemove', { bubbles: true, clientX: 120, clientY: 80 }));
    hitbox.dispatchEvent(new MouseEvent('mouseleave', { bubbles: true }));
    for (const row of wrapper!.findAll('.settings-row')) await row.trigger('click');
    await wrapper!.get('button').trigger('click');
    await vi.advanceTimersByTimeAsync(30_000);
    expect(logo().getAttribute('style')).toBe(stoppedStyle);
    expect(vi.getTimerCount()).toBe(0);
    expect(actions.openUrl).not.toHaveBeenCalled();
    expect(actions.notify).not.toHaveBeenCalled();
    expect(wrapper!.emitted('back')).toBeUndefined();
  });

  it('releases its active timer on unmount instead of scheduling later stages', async () => {
    await pressLogo();
    await vi.advanceTimersByTimeAsync(800);
    expect(vi.getTimerCount()).toBe(1);
    wrapper!.unmount();
    wrapper = null;
    await vi.advanceTimersByTimeAsync(30_000);
    expect(vi.getTimerCount()).toBe(0);
  });

  it('allows a new interaction after reactivation without reviving the old session', async () => {
    await pressLogo();
    await wrapper!.setProps({ active: false });
    await wrapper!.setProps({ active: true });
    await vi.advanceTimersByTimeAsync(800);
    expect(logo().getAttribute('style')).toContain('rotateY(0deg)');
    await pressLogo();
    await vi.advanceTimersByTimeAsync(800);
    expect(logo().getAttribute('style')).toContain('rotateY(180deg)');
  });
});

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { nextTick } from 'vue';
import { mount, type VueWrapper } from '@vue/test-utils';
import AboutSection from '@/features/settings/components/AboutSection.vue';

const actions = vi.hoisted(() => ({ openUrl: vi.fn(), notify: vi.fn() }));
vi.mock('/vcpmobile.svg', () => ({ default: '/vcpmobile.svg' }));
vi.mock('@tauri-apps/api/app', () => ({ getVersion: vi.fn().mockResolvedValue('1.1.6') }));
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: actions.openUrl }));
vi.mock('@/core/stores/theme', () => ({ useThemeStore: () => ({ isDarkResolved: true }) }));
vi.mock('@/core/stores/notification', () => ({ useNotificationStore: () => ({ addNotification: actions.notify }) }));

describe('AboutSection page animation lifetime', () => {
  let wrapper: VueWrapper | null;

  beforeEach(async () => {
    vi.useFakeTimers();
    wrapper = mount(AboutSection, { global: { stubs: { UpdateSection: true } } });
    await vi.advanceTimersByTimeAsync(0);
  });

  afterEach(() => {
    wrapper?.unmount();
    vi.useRealTimers();
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

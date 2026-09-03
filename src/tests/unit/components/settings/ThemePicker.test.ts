import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushPromises, mount, type VueWrapper } from '@vue/test-utils';
import { createPinia, disposePinia, setActivePinia, type Pinia } from 'pinia';
import ThemePicker from '@/features/settings/ThemePicker.vue';
import { useThemeStore } from '@/core/stores/theme';

describe('ThemePicker draft, preview and presentation controls', () => {
  let pinia: Pinia;
  let wrapper: VueWrapper | null;

  beforeEach(() => {
    localStorage.clear();
    localStorage.setItem('vcp-theme-name', 'themes-bear-holiday.ts');
    document.documentElement.style.cssText = '';
    pinia = createPinia();
    setActivePinia(pinia);
    wrapper = null;
  });

  afterEach(() => {
    wrapper?.unmount();
    disposePinia(pinia);
  });

  it('previews a draft locally and applies it only after explicit confirmation', async () => {
    const store = useThemeStore();
    const applySpy = vi.spyOn(store, 'applyThemeFile');
    wrapper = mount(ThemePicker, { global: { plugins: [pinia] } });
    await flushPromises();

    const originalTheme = store.currentTheme;
    const originalStoredTheme = localStorage.getItem('vcp-theme-name');
    const target = wrapper.find('[data-theme-key="themes-simple-bw.ts"]');
    expect(target.exists()).toBe(true);

    await target.trigger('click');

    expect(applySpy).not.toHaveBeenCalled();
    expect(store.currentTheme).toBe(originalTheme);
    expect(localStorage.getItem('vcp-theme-name')).toBe(originalStoredTheme);
    expect(wrapper.find('[data-variant="dark"]').attributes('style')).toContain('--preview-bg: #1c1c1e');
    expect(wrapper.find('[data-variant="light"]').attributes('style')).toContain('--preview-bg: #f4f6f8');
    expect(wrapper.get('[data-testid="theme-apply-button"]').text()).toBe('确认应用主题');

    await wrapper.get('[data-testid="theme-apply-button"]').trigger('click');
    await flushPromises();

    expect(applySpy).toHaveBeenCalledTimes(1);
    expect(applySpy).toHaveBeenCalledWith('themes-simple-bw.ts');
    expect(store.currentTheme).toBe('themes-simple-bw.ts');
    expect(localStorage.getItem('vcp-theme-name')).toBe('themes-simple-bw.ts');
    expect(wrapper.get('[data-testid="theme-apply-button"]').text()).toBe('主题已应用');
    expect(wrapper.get('[data-testid="theme-apply-button"]').attributes()).toHaveProperty('disabled');
  });

  it('keeps dark and light wallpapers isolated inside the preview panes', async () => {
    wrapper = mount(ThemePicker, { global: { plugins: [pinia] } });
    await flushPromises();

    const darkStyle = wrapper.get('[data-variant="dark"]').attributes('style');
    const lightStyle = wrapper.get('[data-variant="light"]').attributes('style');

    expect(darkStyle).toContain('/wallpaper/forest_night.webp');
    expect(lightStyle).toContain('/wallpaper/watermelon_day.webp');
    expect(darkStyle).not.toBe(lightStyle);
    expect(document.documentElement.style.getPropertyValue('--chat-wallpaper-dark')).toBe('');
  });

  it('discards an unconfirmed theme draft when the page unmounts', async () => {
    const store = useThemeStore();
    const applySpy = vi.spyOn(store, 'applyThemeFile');
    wrapper = mount(ThemePicker, { global: { plugins: [pinia] } });
    await flushPromises();

    await wrapper.get('[data-theme-key="themes-simple-bw.ts"]').trigger('click');
    wrapper.unmount();
    wrapper = null;

    expect(applySpy).not.toHaveBeenCalled();
    expect(store.currentTheme).toBe('themes-bear-holiday.ts');
    expect(localStorage.getItem('vcp-theme-name')).toBe('themes-bear-holiday.ts');
  });

  it('keeps the draft available for retry when the explicit theme commit fails', async () => {
    const store = useThemeStore();
    const applyThemeFile = store.applyThemeFile.bind(store);
    const applySpy = vi.spyOn(store, 'applyThemeFile')
      .mockResolvedValueOnce({ ok: false, themeKey: null, error: 'storage unavailable' })
      .mockImplementation(applyThemeFile);
    wrapper = mount(ThemePicker, { global: { plugins: [pinia] } });
    await flushPromises();

    await wrapper.get('[data-theme-key="themes-simple-bw.ts"]').trigger('click');
    await wrapper.get('[data-testid="theme-apply-button"]').trigger('click');
    await flushPromises();

    expect(store.currentTheme).toBe('themes-bear-holiday.ts');
    expect(wrapper.text()).toContain('storage unavailable');
    expect(wrapper.get('[data-testid="theme-apply-button"]').text()).toBe('重试应用主题');

    await wrapper.get('[data-testid="theme-apply-button"]').trigger('click');
    await flushPromises();

    expect(applySpy).toHaveBeenCalledTimes(2);
    expect(store.currentTheme).toBe('themes-simple-bw.ts');
    expect(wrapper.find('[role="alert"]').exists()).toBe(false);
  });

  it('exposes the second three-mode entry and persists a selection immediately', async () => {
    const store = useThemeStore();
    wrapper = mount(ThemePicker, {
      props: { section: 'rendering' },
      global: { plugins: [pinia] },
    });
    await flushPromises();

    expect(wrapper.find('[data-testid="theme-carousel"]').exists()).toBe(false);
    const options = wrapper.findAll('[data-presentation-value]');
    expect(options.map((option) => option.attributes('data-presentation-value'))).toEqual([
      'bubble',
      'panel',
      'immersive',
    ]);
    expect(wrapper.get('[data-presentation-value="bubble"]').attributes('aria-checked')).toBe('true');

    await wrapper.get('[data-presentation-value="panel"]').trigger('click');

    expect(store.presentationMode).toBe('panel');
    expect(localStorage.getItem('vcp-chat-presentation-mode')).toBe('panel');
    expect(wrapper.get('[data-presentation-value="panel"]').attributes('aria-checked')).toBe('true');
    expect(wrapper.text()).toContain('消息呈现与内容宽度');
    expect(wrapper.text()).toContain('点选后立即生效');
    expect(store.smoothStreamingEnabled).toBe(false);

    await wrapper.get('[data-testid="smooth-streaming-switch"]').trigger('click');

    expect(store.smoothStreamingEnabled).toBe(true);
    expect(localStorage.getItem('vcp-smooth-streaming-enabled')).toBe('true');
    expect(wrapper.text()).toContain('不改变回复内容');
    expect(wrapper.get('[data-testid="smooth-streaming-switch"]').attributes('role')).toBe('switch');
    expect(wrapper.get('[data-testid="smooth-streaming-switch"]').attributes('aria-checked')).toBe('true');
  });

  it('keeps message rendering controls out of the theme-only section', async () => {
    wrapper = mount(ThemePicker, {
      props: { section: 'theme' },
      global: { plugins: [pinia] },
    });
    await flushPromises();

    expect(wrapper.find('[data-testid="theme-carousel"]').exists()).toBe(true);
    expect(wrapper.find('[data-testid="presentation-settings"]').exists()).toBe(false);
  });
});

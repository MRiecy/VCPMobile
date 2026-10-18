import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushPromises, mount, type VueWrapper } from '@vue/test-utils';
import { createPinia, disposePinia, setActivePinia, type Pinia } from 'pinia';
import ThemePicker from '@/features/settings/ThemePicker.vue';
import { useThemeStore } from '@/core/stores/theme';
import ChatPreferencesPage from '@/features/chat/settings/ChatPreferencesPage.vue';
import { useChatPreferencesStore } from '@/features/chat/preferences/store';
import { handlePresentationSelection, openPresentationMenu } from '@/features/chat/presentationMenu';
import { useOverlayStore } from '@/core/stores/overlay';

describe('ThemePicker draft, preview and presentation controls', () => {
  let pinia: Pinia;
  let wrapper: VueWrapper | null;

  beforeEach(() => {
    const storage = window.localStorage;
    vi.stubGlobal('localStorage', {
      getItem: storage.getItem.bind(storage), setItem: storage.setItem.bind(storage),
      removeItem: storage.removeItem.bind(storage), clear: storage.clear.bind(storage),
    });
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
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
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


  it('shares the presentation owner with the chat shortcut in both directions', async () => {
    wrapper = mount(ChatPreferencesPage, { global: { plugins: [pinia] } });
    await wrapper.get('[data-presentation-value="panel"]').trigger('click');
    openPresentationMenu();
    expect(useOverlayStore().contextMenuConfig?.actions.find((action) => action.selected)?.label).toBe('统一');

    handlePresentationSelection('immersive');
    await flushPromises();
    expect(wrapper.get('[data-presentation-value="immersive"]').attributes('aria-checked')).toBe('true');
    expect(localStorage.getItem('vcp-chat-presentation-mode')).toBe('immersive');
  });

  it('persists feedback and folding commands and keeps the old controls when a write fails', async () => {
    const store = useChatPreferencesStore();
    wrapper = mount(ChatPreferencesPage, { global: { plugins: [pinia] } });
    await wrapper.get('[data-testid="message-haptics-switch"]').trigger('click');
    await wrapper.get('[data-testid="collapse-tools-switch"]').trigger('click');
    expect(store.messageHapticsEnabled).toBe(false);
    expect(store.collapseToolsWhileStreaming).toBe(true);

    vi.spyOn(localStorage, 'setItem').mockImplementation(() => { throw new Error('quota exceeded'); });
    await wrapper.get('[data-testid="message-haptics-switch"]').trigger('click');
    expect(store.messageHapticsEnabled).toBe(false);
    expect(wrapper.get('[data-testid="message-haptics-switch"]').attributes('aria-checked')).toBe('false');
    expect(wrapper.get('[role="alert"]').text()).toContain('消息震动设置保存失败');
  });

  it('does not change preferences while its page is inactive', async () => {
    wrapper = mount(ChatPreferencesPage, { props: { active: false }, global: { plugins: [pinia] } });
    await wrapper.get('[data-presentation-value="panel"]').trigger('click');
    await wrapper.get('[data-testid="message-haptics-switch"]').trigger('click');
    expect(useChatPreferencesStore().presentationMode).toBe('bubble');
    expect(useChatPreferencesStore().messageHapticsEnabled).toBe(true);
  });

});

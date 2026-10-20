import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import {
  resolveThemeWallpaperUrl,
  normalizeThemePreferencePatch,
  THEME_PREFERENCE_DEFAULTS,
  useThemeStore,
} from '@/core/stores/theme';
import { useChatPreferencesStore } from '@/features/chat/preferences/store';
import { CHAT_PREFERENCE_DEFAULTS, normalizeChatPreferencePatch } from '@/features/chat/preferences/model';

describe('theme and chat presentation store contracts', () => {
  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  beforeEach(() => {
    // Happy DOM binds Storage methods through a Proxy. A plain facade keeps
    // failure spies reversible while retaining its actual backing storage.
    const storage = window.localStorage;
    vi.stubGlobal('localStorage', {
      getItem: storage.getItem.bind(storage), setItem: storage.setItem.bind(storage),
      removeItem: storage.removeItem.bind(storage), clear: storage.clear.bind(storage),
    });
    localStorage.clear();
    document.documentElement.style.cssText = '';
    document.documentElement.className = '';
    document.body.className = '';
    setActivePinia(createPinia());
  });


  it('falls back to bubble and repairs an invalid persisted presentation value', () => {
    localStorage.setItem('vcp-chat-presentation-mode', 'magazine');
    const store = useChatPreferencesStore();

    expect(store.presentationMode).toBe('bubble');
    expect(localStorage.getItem('vcp-chat-presentation-mode')).toBe('bubble');
    store.$dispose();
  });



  it('keeps the previous smooth streaming preference when persistence fails', () => {
    const store = useChatPreferencesStore();
    const originalSetItem = localStorage.setItem.bind(localStorage);
    vi.spyOn(localStorage, 'setItem').mockImplementation((key, value) => {
      if (key === 'vcp-smooth-streaming-enabled') throw new Error('quota exceeded');
      return originalSetItem(key, value);
    });

    const result = store.setSmoothStreamingEnabled(true);

    expect(result.ok).toBe(false);
    expect(result.changed).toBe(false);
    expect(result.enabled).toBe(false);
    expect(store.smoothStreamingEnabled).toBe(false);
    store.$dispose();
  });

  it('keeps the previous presentation mode when local persistence fails', () => {
    const store = useChatPreferencesStore();
    expect(store.setPresentationMode('panel').ok).toBe(true);

    const originalSetItem = localStorage.setItem.bind(localStorage);
    vi.spyOn(localStorage, 'setItem').mockImplementation((key, value) => {
      if (key === 'vcp-chat-presentation-mode') throw new Error('quota exceeded');
      return originalSetItem(key, value);
    });

    const result = store.setPresentationMode('immersive');

    expect(result.ok).toBe(false);
    expect(result.changed).toBe(false);
    expect(store.presentationMode).toBe('panel');
    expect(localStorage.getItem('vcp-chat-presentation-mode')).toBe('panel');
    store.$dispose();
  });


  it.each([
    ['vcp-message-haptics-enabled', 'messageHapticsEnabled', 'setMessageHapticsEnabled', false, true],
    ['vcp-collapse-tools-while-streaming', 'collapseToolsWhileStreaming', 'setCollapseToolsWhileStreaming', true, false],
  ] as const)('does not publish %s when storage rejects a change', (key, field, setter, next, previous) => {
    const store = useChatPreferencesStore();
    const originalSetItem = localStorage.setItem.bind(localStorage);
    vi.spyOn(localStorage, 'setItem').mockImplementation((storageKey, value) => {
      if (storageKey === key) throw new Error('quota exceeded');
      return originalSetItem(storageKey, value);
    });
    expect(store[setter](next)).toMatchObject({ ok: false, changed: false, enabled: previous });
    expect(store[field]).toBe(previous);
    expect(localStorage.getItem(key)).toBeNull();
    store.$dispose();
  });


  it('uses safe defaults without trying to repair values that could not be read', () => {
    vi.spyOn(localStorage, 'getItem').mockImplementation(() => { throw new Error('access denied'); });
    const write = vi.spyOn(localStorage, 'setItem');
    const store = useChatPreferencesStore();
    expect(store.presentationMode).toBe('bubble');
    expect(store.smoothStreamingEnabled).toBe(false);
    expect(store.messageHapticsEnabled).toBe(true);
    expect(store.collapseToolsWhileStreaming).toBe(false);
    expect(write).not.toHaveBeenCalled();
    store.$dispose();
  });


  it('accepts only bundled wallpaper basenames and maps them to WebP', () => {
    expect(resolveThemeWallpaperUrl("'forest_night.webp'")).toBe('/wallpaper/forest_night.webp');
    expect(resolveThemeWallpaperUrl("url('wallpaper-mountain-daybreak.png')")).toBe(
      '/wallpaper/wallpaper-mountain-daybreak.webp',
    );
    expect(resolveThemeWallpaperUrl('none')).toBeNull();
    expect(resolveThemeWallpaperUrl('../secret.webp')).toBeNull();
    expect(resolveThemeWallpaperUrl('https://example.com/remote.webp')).toBeNull();
    expect(resolveThemeWallpaperUrl('javascript:alert.webp')).toBeNull();
    expect(resolveThemeWallpaperUrl('theme").webp')).toBeNull();
  });

  it('validates before applying and persists only a canonical TS theme key', async () => {
    const store = useThemeStore();
    const initialTheme = store.currentTheme;
    const initialCss = document.documentElement.style.cssText;

    const invalidResult = await store.applyThemeFile('../missing.css');
    expect(invalidResult.ok).toBe(false);
    expect(store.currentTheme).toBe(initialTheme);
    expect(document.documentElement.style.cssText).toBe(initialCss);

    const result = await store.applyThemeFile('themes-simple-bw.ts');
    expect(result).toEqual({ ok: true, themeKey: 'themes-simple-bw.ts' });
    expect(store.currentTheme).toBe('themes-simple-bw.ts');
    expect(store.currentThemeInfo?.fileName).toBe('themes-simple-bw.ts');
    expect(localStorage.getItem('vcp-theme-name')).toBe('themes-simple-bw.ts');
    // 默认模式为 light（星渊雪境亮色默认主题）：注入的是 simple-bw 的亮色变量
    expect(document.documentElement.style.getPropertyValue('--primary-bg')).toBe('#f4f6f8');
    store.$dispose();
  });

  it('uses owner defaults for invalid imported leaves while preserving present siblings and absent fields', async () => {
    expect(normalizeThemePreferencePatch({ themeKey: 'themes黑白简约.css', mode: 'dark', ignored: true }))
      .toEqual({ themeKey: THEME_PREFERENCE_DEFAULTS.themeKey, mode: 'dark' });
    expect(normalizeThemePreferencePatch({ mode: 'sepia' }))
      .toEqual({ mode: THEME_PREFERENCE_DEFAULTS.mode });
    expect(normalizeChatPreferencePatch({ presentationMode: 'magazine', messageHapticsEnabled: false, smoothStreamingEnabled: 'true' }))
      .toEqual({ presentationMode: 'bubble', messageHapticsEnabled: false, smoothStreamingEnabled: false });
    expect(normalizeChatPreferencePatch({ ignored: true })).toEqual({});
    expect(normalizeChatPreferencePatch(null)).toEqual(CHAT_PREFERENCE_DEFAULTS);

    const store = useThemeStore();
    await store.restorePreferences({ themeKey: 'themes-simple-bw.ts', mode: 'dark' });
    await store.restorePreferences({ mode: 'invalid' });
    expect(store.currentTheme).toBe('themes-simple-bw.ts');
    expect(store.mode).toBe(THEME_PREFERENCE_DEFAULTS.mode);
    await store.restorePreferences({ themeKey: 'themes-simple-bw.css' });
    expect(store.currentTheme).toBe(THEME_PREFERENCE_DEFAULTS.themeKey);
    store.$dispose();
  });

  it('rolls refs, root variables and the persisted key back when commit fails', async () => {
    const store = useThemeStore();
    await store.applyThemeFile('themes-simple-bw.ts');
    const previousTheme = store.currentTheme;
    const previousBackground = document.documentElement.style.getPropertyValue('--primary-bg');

    const originalSetItem = localStorage.setItem.bind(localStorage);
    vi.spyOn(localStorage, 'setItem').mockImplementation((key, value) => {
      if (key === 'vcp-theme-name') throw new Error('storage unavailable');
      return originalSetItem(key, value);
    });

    const result = await store.applyThemeFile('themes-bear-holiday.ts');

    expect(result.ok).toBe(false);
    expect(store.currentTheme).toBe(previousTheme);
    expect(document.documentElement.style.getPropertyValue('--primary-bg')).toBe(previousBackground);
    expect(localStorage.getItem('vcp-theme-name')).toBe(previousTheme);
    store.$dispose();
  });


  it('restores theme and mode together without the interactive mode debounce', async () => {
    const store = useThemeStore();
    store.setMode('dark');
    const result = await store.restorePreferences({ themeKey: 'themes-simple-bw.ts', mode: 'system' });
    expect(result.ok).toBe(true);
    expect(store.mode).toBe('system');
    expect(store.isDarkResolved).toBe(false);
    expect(localStorage.getItem('vcp-theme-mode')).toBe('system');
    expect(localStorage.getItem('vcp-theme-name')).toBe('themes-simple-bw.ts');
    store.$dispose();

    setActivePinia(createPinia());
    const reopened = useThemeStore();
    expect(reopened.mode).toBe('system');
    expect(reopened.currentTheme).toBe('themes-simple-bw.ts');
    reopened.$dispose();
  });

  it('restores the previous theme, mode, DOM and storage when the second preference write fails', async () => {
    const store = useThemeStore();
    await store.restorePreferences({ themeKey: 'themes-simple-bw.ts', mode: 'dark' });
    const style = document.documentElement.style;
    const before = Object.fromEntries(Array.from(style).map(key => [key, style.getPropertyValue(key)]));
    const originalWrite = localStorage.setItem.bind(localStorage);
    vi.spyOn(localStorage, 'setItem').mockImplementation((key, value) => {
      if (key === 'vcp-theme-mode' && value === 'system') throw new Error('mode storage full');
      originalWrite(key, value);
    });
    const result = await store.restorePreferences({ themeKey: 'themes-bear-holiday.ts', mode: 'system' });
    expect(result).toMatchObject({ ok: false, error: expect.stringContaining('mode storage full') });
    expect(store.currentTheme).toBe('themes-simple-bw.ts');
    expect(store.mode).toBe('dark');
    expect(store.isDarkResolved).toBe(true);
    expect(document.documentElement.classList.contains('dark')).toBe(true);
    expect(Object.fromEntries(Array.from(style).map(key => [key, style.getPropertyValue(key)]))).toEqual(before);
    expect(localStorage.getItem('vcp-theme-name')).toBe('themes-simple-bw.ts');
    expect(localStorage.getItem('vcp-theme-mode')).toBe('dark');
    store.$dispose();
  });
});

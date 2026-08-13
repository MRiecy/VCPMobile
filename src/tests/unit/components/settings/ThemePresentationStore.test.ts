import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import {
  normalizeChatPresentationMode,
  normalizeThemeModuleKey,
  resolveThemeWallpaperUrl,
  useThemeStore,
} from '@/core/stores/theme';

describe('theme and chat presentation store contracts', () => {
  beforeEach(() => {
    localStorage.clear();
    document.documentElement.style.cssText = '';
    document.documentElement.className = '';
    document.body.className = '';
    setActivePinia(createPinia());
  });

  it.each(['bubble', 'panel', 'immersive'] as const)(
    'restores the legal presentation value %s',
    (mode) => {
      localStorage.setItem('vcp-chat-presentation-mode', mode);
      const store = useThemeStore();
      expect(store.presentationMode).toBe(mode);
      store.$dispose();
    },
  );

  it('falls back to bubble and repairs an invalid persisted presentation value', () => {
    localStorage.setItem('vcp-chat-presentation-mode', 'magazine');
    const store = useThemeStore();

    expect(store.presentationMode).toBe('bubble');
    expect(localStorage.getItem('vcp-chat-presentation-mode')).toBe('bubble');
    expect(normalizeChatPresentationMode(null)).toBe('bubble');
    store.$dispose();
  });

  it('does not write or reflow when setting the current presentation value', () => {
    const store = useThemeStore();
    const setItem = vi.spyOn(localStorage, 'setItem');

    const result = store.setPresentationMode('bubble');

    expect(result).toEqual({ ok: true, changed: false, mode: 'bubble' });
    expect(setItem).not.toHaveBeenCalledWith('vcp-chat-presentation-mode', expect.anything());
    store.$dispose();
  });

  it('keeps the previous presentation mode when local persistence fails', () => {
    const store = useThemeStore();
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

  it('normalizes legacy CSS theme identities to the compiled TS module key', () => {
    expect(normalizeThemeModuleKey('themes-bear-holiday.css')).toBe('themes-bear-holiday.ts');
    expect(normalizeThemeModuleKey('themes熊熊假日.css')).toBe('themes-bear-holiday.ts');
    expect(normalizeThemeModuleKey('themes-bear-holiday.ts')).toBe('themes-bear-holiday.ts');
    expect(normalizeThemeModuleKey('../themes-bear-holiday.ts')).toBeNull();
    expect(normalizeThemeModuleKey('https://example.com/theme.ts')).toBeNull();
    expect(normalizeThemeModuleKey('missing.ts')).toBeNull();
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
    expect(resolveThemeWallpaperUrl('theme\").webp')).toBeNull();
  });

  it('validates before applying and persists only a canonical TS theme key', async () => {
    const store = useThemeStore();
    const initialTheme = store.currentTheme;
    const initialCss = document.documentElement.style.cssText;

    const invalidResult = await store.applyThemeFile('../missing.css');
    expect(invalidResult.ok).toBe(false);
    expect(store.currentTheme).toBe(initialTheme);
    expect(document.documentElement.style.cssText).toBe(initialCss);

    const result = await store.applyThemeFile('themes-simple-bw.css');
    expect(result).toEqual({ ok: true, themeKey: 'themes-simple-bw.ts' });
    expect(store.currentTheme).toBe('themes-simple-bw.ts');
    expect(store.currentThemeInfo?.fileName).toBe('themes-simple-bw.ts');
    expect(localStorage.getItem('vcp-theme-name')).toBe('themes-simple-bw.ts');
    expect(document.documentElement.style.getPropertyValue('--primary-bg')).toBe('#1c1c1e');
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

  it('enumerates only TS theme modules for the picker', async () => {
    const store = useThemeStore();
    await store.fetchThemes();

    expect(store.availableThemes).toHaveLength(13);
    expect(store.availableThemes.every((theme) => theme.fileName.endsWith('.ts'))).toBe(true);
    store.$dispose();
  });
});

import { defineStore, acceptHMRUpdate } from 'pinia';
import { onScopeDispose, ref, watch } from 'vue';

export type ThemeMode = 'light' | 'dark' | 'system';

export interface ThemePreferences {
  themeKey: string;
  mode: ThemeMode;
}

export interface ThemeApplyResult {
  ok: boolean;
  themeKey: string | null;
  error?: string;
}

export interface ThemeInfo {
  fileName: string;
  name: string;
  variables: {
    dark: Record<string, string>;
    light: Record<string, string>;
  };
}

const DEFAULT_THEME = 'themes-star-abyss.ts';
const THEME_NAME_STORAGE_KEY = 'vcp-theme-name';

export const THEME_PREFERENCE_DEFAULTS: Readonly<ThemePreferences> = {
  themeKey: DEFAULT_THEME,
  mode: 'light',
};

interface ThemeModule {
  meta: { fileName: string; name: string };
  variables: { dark: Record<string, string>; light: Record<string, string> };
  extraCss?: string;
}

// Vite dynamic imports for TS theme modules (one per theme, static pre-compiled)
const themeModules = import.meta.glob('../../assets/themes/*.ts', { eager: true }) as Record<string, ThemeModule>;

const themeModulesByKey = new Map<string, ThemeModule>();
for (const [path, mod] of Object.entries(themeModules)) {
  const key = path.split(/[\\/]/).pop() || '';
  if (key) themeModulesByKey.set(key, mod);
}

export const normalizeThemeModuleKey = (value: unknown): string | null => {
  return typeof value === 'string' && themeModulesByKey.has(value) ? value : null;
};

export const normalizeThemeMode = (value: unknown): ThemeMode =>
  value === 'light' || value === 'dark' || value === 'system'
    ? value
    : THEME_PREFERENCE_DEFAULTS.mode;

/** 已提供的坏值回退；缺失字段不进入补丁，避免覆盖当前偏好。 */
export const normalizeThemePreferencePatch = (value: unknown): Partial<ThemePreferences> => {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) {
    return { ...THEME_PREFERENCE_DEFAULTS };
  }
  const source = value as Record<string, unknown>;
  const patch: Partial<ThemePreferences> = {};
  if (Object.prototype.hasOwnProperty.call(source, 'themeKey')) {
    patch.themeKey = normalizeThemeModuleKey(source.themeKey) ?? THEME_PREFERENCE_DEFAULTS.themeKey;
  }
  if (Object.prototype.hasOwnProperty.call(source, 'mode')) {
    patch.mode = normalizeThemeMode(source.mode);
  }
  return patch;
};

export const resolveThemeWallpaperUrl = (value: unknown): string | null => {
  if (typeof value !== 'string') return null;

  let candidate = value.trim();
  if (!candidate || candidate.toLowerCase() === 'none') return null;

  const urlMatch = candidate.match(/^url\((.*)\)$/i);
  if (urlMatch) candidate = urlMatch[1].trim();
  if (
    (candidate.startsWith('"') && candidate.endsWith('"')) ||
    (candidate.startsWith("'") && candidate.endsWith("'"))
  ) {
    candidate = candidate.slice(1, -1).trim();
  }

  if (
    !candidate ||
    candidate.includes('/') ||
    candidate.includes('\\') ||
    candidate.includes('..') ||
    candidate.includes(':') ||
    candidate.includes('?') ||
    candidate.includes('#') ||
    /[\u0000-\u001f\u007f]/.test(candidate)
  ) {
    return null;
  }

  const extension = candidate.match(/\.(webp|png|jpe?g)$/i);
  if (!extension) return null;
  const basename = candidate.slice(0, -extension[0].length);
  if (!basename || !/^[\p{L}\p{N}_-]+$/u.test(basename)) return null;
  return `/wallpaper/${basename}.webp`;
};

const findThemeModule = (fileName: unknown): { key: string; module: ThemeModule } | null => {
  const key = normalizeThemeModuleKey(fileName);
  if (!key) return null;
  const mod = themeModulesByKey.get(key);
  return mod ? { key, module: mod } : null;
};

const errorMessage = (error: unknown): string => {
  return error instanceof Error ? error.message : String(error);
};

export const useThemeStore = defineStore('theme', () => {
  const readStoredValue = (key: string): string | null => {
    try {
      return localStorage.getItem(key);
    } catch (error) {
      console.warn(`[themeStore] Failed to read ${key}:`, error);
      return null;
    }
  };

  const storedMode = readStoredValue('vcp-theme-mode');
  // 缺失或异常模式使用主题所属领域的默认值。
  const initialMode = normalizeThemeMode(storedMode);
  const mode = ref<ThemeMode>(initialMode);
  // 初值与默认模式（light）保持一致；store 创建时 mode 的 immediate watch
  // 会同步执行 applyTheme 将其纠正为真实值。
  const isDarkResolved = ref(false);
  const lastModeSwitchAt = ref(0);
  const MODE_SWITCH_DEBOUNCE_MS = 420;

  const initialTheme = normalizeThemeModuleKey(readStoredValue(THEME_NAME_STORAGE_KEY)) || DEFAULT_THEME;
  const currentTheme = ref(initialTheme);

  const availableThemes = ref<ThemeInfo[]>([]);
  const themeThumbnails = ref<Record<string, string>>({});
  const currentThemeInfo = ref<ThemeInfo | null>(null);
  const lastAppliedVarKeys = ref<string[]>([]);
  let currentThemeModule: ThemeModule | null = null;
  let isInitializing = true;
  let restoringMode = false;

  const triggerThemeSwitchTransition = () => {
    if (isInitializing) return;
    document.documentElement.classList.add('theme-switching');
    setTimeout(() => {
      document.documentElement.classList.remove('theme-switching');
    }, 400);
  };


  const injectVariables = (vars: Record<string, string>) => {
    // Clear stale variables from previous theme to avoid mixed state
    for (const key of lastAppliedVarKeys.value) {
      document.documentElement.style.removeProperty(key);
    }
    for (const [key, value] of Object.entries(vars)) {
      document.documentElement.style.setProperty(key, value);
    }
    lastAppliedVarKeys.value = Object.keys(vars);
    // 首帧占位底色（index.html 的 boot-dark）使命完成：真实主题变量已就绪，
    // body 背景职责移交 var(--primary-bg)。幂等，后续主题切换时为无操作。
    document.documentElement.classList.remove('boot-dark');
  };

  const fetchThemes = async () => {
    const themes: ThemeInfo[] = [];

    for (const [path, mod] of Object.entries(themeModules)) {
      try {
        const fileName = path.split(/[\\/]/).pop() || '';

        themes.push({
          fileName,
          name: mod.meta.name,
          variables: mod.variables,
        });
      } catch (e) {
        console.error(`Failed to load theme module: ${path}`, e);
      }
    }

    availableThemes.value = themes;

    // Build a lightweight fallback thumbnail cache once after themes are loaded.
    // The live preview resolves dark/light wallpapers independently.
    const thumbs: Record<string, string> = {};
    for (const theme of themes) {
      const darkWp = theme.variables?.dark?.['--chat-wallpaper-dark'];
      const lightWp = theme.variables?.light?.['--chat-wallpaper-light'];
      const thumbnail = resolveThemeWallpaperUrl(darkWp) || resolveThemeWallpaperUrl(lightWp);
      if (thumbnail) thumbs[theme.fileName] = thumbnail;
    }
    themeThumbnails.value = thumbs;
  };

  const applyThemeSelection = async (fileName: string, restoredMode?: ThemeMode): Promise<ThemeApplyResult> => {
    const resolved = findThemeModule(fileName);
    if (!resolved) {
      const error = `Theme module not found: ${String(fileName)}`;
      console.warn(error);
      return { ok: false, themeKey: null, error };
    }

    const { key: themeKey, module: mod } = resolved;
    if (
      !mod.meta ||
      typeof mod.meta.name !== 'string' ||
      !mod.variables ||
      !mod.variables.dark ||
      !mod.variables.light
    ) {
      const error = `Theme module is invalid: ${themeKey}`;
      console.warn(error);
      return { ok: false, themeKey: null, error };
    }

    let previousStoredTheme: string | null;
    let previousStoredMode: string | null = null;
    try {
      previousStoredTheme = localStorage.getItem(THEME_NAME_STORAGE_KEY);
      if (restoredMode !== undefined) previousStoredMode = localStorage.getItem('vcp-theme-mode');
    } catch (error) {
      return {
        ok: false,
        themeKey: null,
        error: `无法读取主题偏好：${errorMessage(error)}`,
      };
    }

    const previousTheme = currentTheme.value;
    const previousThemeInfo = currentThemeInfo.value;
    const previousThemeModule = currentThemeModule;
    const previousMode = mode.value;
    const previousDark = isDarkResolved.value;
    const previousDarkClass = document.documentElement.classList.contains('dark');
    const previousLightClass = document.body.classList.contains('light-theme');
    const previousVarKeys = [...lastAppliedVarKeys.value];
    const nextDark = restoredMode === undefined ? previousDark : restoredMode === 'dark'
      || (restoredMode === 'system' && window.matchMedia('(prefers-color-scheme: dark)').matches);
    const nextVars = nextDark ? mod.variables.dark : mod.variables.light;
    const affectedVarKeys = Array.from(new Set([...previousVarKeys, ...Object.keys(nextVars)]));
    const previousRootVariables = affectedVarKeys.map((property) => ({
      property,
      value: document.documentElement.style.getPropertyValue(property),
      priority: document.documentElement.style.getPropertyPriority(property),
    }));
    const existingStyleTag = document.getElementById('vcp-custom-theme');
    const previousStyleText = existingStyleTag?.textContent ?? '';
    let createdStyleTag = false;
    let themePersisted = false;
    let modePersisted = false;

    try {
      triggerThemeSwitchTransition();
      console.log('[themeStore] Loaded module for', themeKey, mod.meta.name);

      injectVariables(nextVars);
      if (restoredMode !== undefined) {
        isDarkResolved.value = nextDark;
        document.documentElement.classList.toggle('dark', nextDark);
        document.body.classList.toggle('light-theme', !nextDark);
      }

      currentThemeModule = mod;
      currentThemeInfo.value = {
        fileName: themeKey,
        name: mod.meta.name,
        variables: mod.variables,
      };
      currentTheme.value = themeKey;

      // Inject extra CSS rules (non-variable styles like .tool-bubble)
      let styleTag = document.getElementById('vcp-custom-theme');
      if (!styleTag) {
        styleTag = document.createElement('style');
        styleTag.id = 'vcp-custom-theme';
        document.head.appendChild(styleTag);
        createdStyleTag = true;
      }
      styleTag.textContent = mod.extraCss || '';

      // Persist only after the complete visual state has been prepared. A storage
      // failure rolls the DOM and refs back to the previously applied theme.
      localStorage.setItem(THEME_NAME_STORAGE_KEY, themeKey);
      themePersisted = true;
      if (restoredMode !== undefined) {
        localStorage.setItem('vcp-theme-mode', restoredMode);
        modePersisted = true;
        restoringMode = true;
        try { mode.value = restoredMode; } finally { restoringMode = false; }
      }
      return { ok: true, themeKey };
    } catch (error) {
      console.error('Failed to apply theme file:', error);

      for (const snapshot of previousRootVariables) {
        if (snapshot.value) {
          document.documentElement.style.setProperty(
            snapshot.property,
            snapshot.value,
            snapshot.priority,
          );
        } else {
          document.documentElement.style.removeProperty(snapshot.property);
        }
      }
      lastAppliedVarKeys.value = previousVarKeys;
      currentTheme.value = previousTheme;
      currentThemeInfo.value = previousThemeInfo;
      currentThemeModule = previousThemeModule;
      isDarkResolved.value = previousDark;
      document.documentElement.classList.toggle('dark', previousDarkClass);
      document.body.classList.toggle('light-theme', previousLightClass);
      restoringMode = true;
      try { mode.value = previousMode; } finally { restoringMode = false; }

      const styleTag = document.getElementById('vcp-custom-theme');
      if (createdStyleTag) {
        styleTag?.remove();
      } else if (styleTag) {
        styleTag.textContent = previousStyleText;
      }

      let rollbackFailure = '';
      try {
        if (themePersisted) {
          if (previousStoredTheme === null) localStorage.removeItem(THEME_NAME_STORAGE_KEY);
          else localStorage.setItem(THEME_NAME_STORAGE_KEY, previousStoredTheme);
        }
        if (modePersisted) {
          if (previousStoredMode === null) localStorage.removeItem('vcp-theme-mode');
          else localStorage.setItem('vcp-theme-mode', previousStoredMode);
        }
      } catch (rollbackError) {
        console.error('[themeStore] Failed to restore persisted theme:', rollbackError);
        rollbackFailure = `；恢复原主题存储失败：${errorMessage(rollbackError)}`;
      }

      return {
        ok: false,
        themeKey: null,
        error: `主题应用失败：${errorMessage(error)}${rollbackFailure}`,
      };
    }
  };

  const applyThemeFile = (fileName: string) => applyThemeSelection(fileName);

  // Backup restore is a persisted operation, independent of tap debouncing.
  const restorePreferences = (candidate: unknown): Promise<ThemeApplyResult> => {
    const preferences = normalizeThemePreferencePatch(candidate);
    if (!Object.keys(preferences).length) return Promise.resolve({ ok: true, themeKey: currentTheme.value });
    return applyThemeSelection(preferences.themeKey ?? currentTheme.value, preferences.mode);
  };

  const initTheme = async () => {
    const savedTheme = normalizeThemeModuleKey(readStoredValue(THEME_NAME_STORAGE_KEY)) || DEFAULT_THEME;

    try {
      // 1. 优先只加载当前主题，确保背景和基础样式瞬间呈现
      const result = await applyThemeFile(savedTheme);
      if (!result.ok && savedTheme !== DEFAULT_THEME) {
        await applyThemeFile(DEFAULT_THEME);
      }

      // 2. 优雅地在浏览器空闲时再扫描全量主题元数据
      const idleCallback = (window as any).requestIdleCallback || ((cb: any) => setTimeout(cb, 1000));
      idleCallback(() => {
        fetchThemes().catch(console.error);
      });
    } finally {
      isInitializing = false;
    }
  };

  const applyTheme = (newMode: ThemeMode) => {
    triggerThemeSwitchTransition();
    const isDark =
      newMode === 'dark' ||
      (newMode === 'system' && window.matchMedia('(prefers-color-scheme: dark)').matches);

    isDarkResolved.value = isDark;
    document.documentElement.classList.toggle('dark', isDark);
    document.body.classList.toggle('light-theme', !isDark);
    localStorage.setItem('vcp-theme-mode', newMode);

    // Re-inject variables for the new mode if a theme is already loaded
    if (currentThemeModule) {
      const vars = isDark ? currentThemeModule.variables.dark : currentThemeModule.variables.light;
      injectVariables(vars);
    }
  };

  watch(mode, (newMode) => {
    if (!restoringMode) applyTheme(newMode);
  }, { immediate: true, flush: 'sync' });

  // Listen for system theme changes
  const mediaQuery = window.matchMedia('(prefers-color-scheme: dark)');
  const handleMediaChange = () => {
    if (mode.value === 'system') {
      applyTheme('system');
    }
  };
  mediaQuery.addEventListener('change', handleMediaChange);

  const setMode = (newMode: ThemeMode) => {
    const now = Date.now();
    if (now - lastModeSwitchAt.value < MODE_SWITCH_DEBOUNCE_MS) {
      return;
    }

    if (mode.value === newMode) {
      return;
    }

    lastModeSwitchAt.value = now;
    mode.value = newMode;
  };

  const toggleTheme = () => {
    // Use the resolved state to decide the next mode,
    // this ensures that the first click always produces a visual change
    // even if the current mode is 'system'.
    setMode(isDarkResolved.value ? 'light' : 'dark');
  };

  onScopeDispose(() => {
    mediaQuery.removeEventListener('change', handleMediaChange);
  });

  // Vite HMR: 当主题 TS 文件修改时，Vite 会热更新该模块并冒泡到 theme.ts。
  // 我们通过拦截更新并重新执行 applyThemeFile 来实现样式的实时无刷新生效。
  // 通过 import.meta.hot.data.isHMR 区分首次初始化与后续热重载，防止在普通启动时与生命周期并行竞争
  if (import.meta.hot) {
    const hotData = import.meta.hot.data;
    if (hotData?.isHMR) {
      setTimeout(() => {
        console.log('[themeStore] HMR reload triggered, re-applying theme:', currentTheme.value);
        if (currentTheme.value) {
          void applyThemeFile(currentTheme.value);
        }
      }, 100);
    }
    if (hotData) hotData.isHMR = true;
  }

  return {
    mode,
    isDarkResolved,
    currentTheme,
    currentThemeInfo,
    availableThemes,
    themeThumbnails,
    fetchThemes,
    applyThemeFile,
    restorePreferences,
    initTheme,
    toggleTheme,
    setMode,
  };
});

if (import.meta.hot) {
  import.meta.hot.accept(acceptHMRUpdate(useThemeStore, import.meta.hot));
}

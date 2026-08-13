import { defineStore, acceptHMRUpdate } from 'pinia';
import { onScopeDispose, ref, watch } from 'vue';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

export type ThemeMode = 'light' | 'dark' | 'system';

export const CHAT_PRESENTATION_MODES = ['bubble', 'panel', 'immersive'] as const;
export type ChatPresentationMode = (typeof CHAT_PRESENTATION_MODES)[number];

export interface ChatPresentationOption {
  value: ChatPresentationMode;
  label: string;
  title: string;
  description: string;
}

export const CHAT_PRESENTATION_OPTIONS: readonly ChatPresentationOption[] = [
  {
    value: 'bubble',
    label: '气泡',
    title: '气泡模式',
    description: '保留左右气泡、头像与紧凑内容宽度。',
  },
  {
    value: 'panel',
    label: '统一',
    title: '统一模式',
    description: '消息共用连续全宽表面，以细分隔线区分。',
  },
  {
    value: 'immersive',
    label: '刊物',
    title: '刊物模式',
    description: '隐藏头像，使用居中的长文阅读宽度。',
  },
] as const;

export interface PresentationChangeResult {
  ok: boolean;
  changed: boolean;
  mode: ChatPresentationMode;
  error?: string;
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

const DEFAULT_THEME = 'themes-bear-holiday.ts';
const THEME_NAME_STORAGE_KEY = 'vcp-theme-name';
const PRESENTATION_STORAGE_KEY = 'vcp-chat-presentation-mode';

const LEGACY_THEME_MAP: Record<string, string> = {
  'themes冰火魔歌.css': 'themes-ice-fire.css',
  'themes瓷与锦.css': 'themes-porcelain-brocade.css',
  'themes绯红天穹.css': 'themes-crimson-sky.css',
  'themes黑白简约.css': 'themes-simple-bw.css',
  'themes静谧森岭.css': 'themes-quiet-forest.css',
  'themes卡提西亚.css': 'themes-cartethyia.css',
  'themes霓虹咖啡.css': 'themes-neon-coffee.css',
  'themes童趣梦境.css': 'themes-childhood-dream.css',
  'themes星咏与狼嗥.css': 'themes-star-wolf.css',
  'themes星渊雪境.css': 'themes-star-abyss.css',
  'themes熊熊假日.css': 'themes-bear-holiday.css',
  'themes雪境晨昏.css': 'themes-snow-morning.css',
  'themes夜樱猫语.css': 'themes-night-sakura-cat.css'
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

export const normalizeChatPresentationMode = (value: unknown): ChatPresentationMode => {
  return CHAT_PRESENTATION_MODES.includes(value as ChatPresentationMode)
    ? (value as ChatPresentationMode)
    : 'bubble';
};

export const normalizeThemeModuleKey = (value: unknown): string | null => {
  if (typeof value !== 'string') return null;
  const input = value.trim();
  if (!input || input.includes('/') || input.includes('\\') || input.includes('\0')) {
    return null;
  }

  const legacyMapped = LEGACY_THEME_MAP[input] || input;
  const key = legacyMapped.endsWith('.css')
    ? `${legacyMapped.slice(0, -4)}.ts`
    : legacyMapped;

  if (!key.endsWith('.ts') || !themeModulesByKey.has(key)) return null;
  return key;
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
  const initialMode: ThemeMode = storedMode === 'light' || storedMode === 'system'
    ? storedMode
    : 'dark';
  const mode = ref<ThemeMode>(initialMode);
  const isDarkResolved = ref(true);
  const lastModeSwitchAt = ref(0);
  const MODE_SWITCH_DEBOUNCE_MS = 420;

  const storedPresentationMode = readStoredValue(PRESENTATION_STORAGE_KEY);
  const initialPresentationMode = normalizeChatPresentationMode(storedPresentationMode);
  if (storedPresentationMode !== null && storedPresentationMode !== initialPresentationMode) {
    try {
      localStorage.setItem(PRESENTATION_STORAGE_KEY, initialPresentationMode);
    } catch (error) {
      console.warn('[themeStore] Failed to repair presentation preference:', error);
    }
  }
  const presentationMode = ref<ChatPresentationMode>(initialPresentationMode);

  const initialTheme = normalizeThemeModuleKey(readStoredValue(THEME_NAME_STORAGE_KEY)) || DEFAULT_THEME;
  const currentTheme = ref(initialTheme);

  const availableThemes = ref<ThemeInfo[]>([]);
  const themeThumbnails = ref<Record<string, string>>({});
  const currentThemeInfo = ref<ThemeInfo | null>(null);
  const lastAppliedVarKeys = ref<string[]>([]);
  let currentThemeModule: ThemeModule | null = null;
  let isInitializing = true;

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

  const applyThemeFile = async (fileName: string): Promise<ThemeApplyResult> => {
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
    try {
      previousStoredTheme = localStorage.getItem(THEME_NAME_STORAGE_KEY);
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
    const previousVarKeys = [...lastAppliedVarKeys.value];
    const nextVars = isDarkResolved.value ? mod.variables.dark : mod.variables.light;
    const affectedVarKeys = Array.from(new Set([...previousVarKeys, ...Object.keys(nextVars)]));
    const previousRootVariables = affectedVarKeys.map((property) => ({
      property,
      value: document.documentElement.style.getPropertyValue(property),
      priority: document.documentElement.style.getPropertyPriority(property),
    }));
    const existingStyleTag = document.getElementById('vcp-custom-theme');
    const previousStyleText = existingStyleTag?.textContent ?? '';
    let createdStyleTag = false;

    try {
      triggerThemeSwitchTransition();
      console.log('[themeStore] Loaded module for', themeKey, mod.meta.name);

      injectVariables(nextVars);

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

      const styleTag = document.getElementById('vcp-custom-theme');
      if (createdStyleTag) {
        styleTag?.remove();
      } else if (styleTag) {
        styleTag.textContent = previousStyleText;
      }

      try {
        if (previousStoredTheme === null) {
          localStorage.removeItem(THEME_NAME_STORAGE_KEY);
        } else {
          localStorage.setItem(THEME_NAME_STORAGE_KEY, previousStoredTheme);
        }
      } catch (rollbackError) {
        console.error('[themeStore] Failed to restore persisted theme:', rollbackError);
      }

      return {
        ok: false,
        themeKey: null,
        error: `主题应用失败：${errorMessage(error)}`,
      };
    }
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
    applyTheme(newMode);
  }, { immediate: true });

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

  const setPresentationMode = (candidate: unknown): PresentationChangeResult => {
    const nextMode = normalizeChatPresentationMode(candidate);
    if (presentationMode.value === nextMode) {
      return { ok: true, changed: false, mode: nextMode };
    }

    try {
      localStorage.setItem(PRESENTATION_STORAGE_KEY, nextMode);
      presentationMode.value = nextMode;
      return { ok: true, changed: true, mode: nextMode };
    } catch (error) {
      console.error('[themeStore] Failed to persist presentation mode:', error);
      return {
        ok: false,
        changed: false,
        mode: presentationMode.value,
        error: `消息呈现设置保存失败：${errorMessage(error)}`,
      };
    }
  };

  const toggleTheme = () => {
    // Use the resolved state to decide the next mode,
    // this ensures that the first click always produces a visual change
    // even if the current mode is 'system'.
    setMode(isDarkResolved.value ? 'light' : 'dark');
  };

  // Listen for theme updates from backend
  // Store the promise so onScopeDispose can clean up even if the listener
  // hasn't resolved yet (avoids dangling listeners on hot reload / scope disposal)
  const unlistenThemePromise = listen('onThemeUpdated', (event) => {
    const themeKey = normalizeThemeModuleKey(event.payload);
    if (themeKey && themeKey !== currentTheme.value) {
      void applyThemeFile(themeKey);
    }
  });

  onScopeDispose(() => {
    mediaQuery.removeEventListener('change', handleMediaChange);
    unlistenThemePromise.then((fn: UnlistenFn) => fn()).catch(() => {});
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
    presentationMode,
    currentTheme,
    currentThemeInfo,
    availableThemes,
    themeThumbnails,
    fetchThemes,
    applyThemeFile,
    initTheme,
    toggleTheme,
    setMode,
    setPresentationMode,
  };
});

if (import.meta.hot) {
  import.meta.hot.accept(acceptHMRUpdate(useThemeStore, import.meta.hot));
}

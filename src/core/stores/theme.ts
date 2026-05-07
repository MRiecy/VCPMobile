import { defineStore } from 'pinia';
import { onScopeDispose, ref, watch } from 'vue';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

export type ThemeMode = 'light' | 'dark' | 'system';

export interface ThemeInfo {
  fileName: string;
  name: string;
  variables: {
    dark: Record<string, string>;
    light: Record<string, string>;
  };
}

const DEFAULT_THEME = 'themes-bear-holiday.css';

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

// Vite dynamic imports for TS theme modules (one per theme, lazy-loaded)
const themeModules = import.meta.glob('../../assets/themes/*.ts') as Record<string, () => Promise<ThemeModule>>;

const fileNameToLoader = new Map<string, () => Promise<ThemeModule>>();

const findThemeLoader = (fileName: string): (() => Promise<ThemeModule>) | undefined => {
  const tsFileName = fileName.replace('.css', '.ts');

  // 1. Try cache first
  const cached = fileNameToLoader.get(tsFileName);
  if (cached) return cached;

  // 2. Fall back to scanning themeModules keys (handle Windows backslashes)
  for (const [path, loader] of Object.entries(themeModules)) {
    const keyFileName = path.split(/[\\/]/).pop() || '';
    if (keyFileName === tsFileName) {
      fileNameToLoader.set(tsFileName, loader);
      return loader;
    }
  }

  return undefined;
};

export const useThemeStore = defineStore('theme', () => {
  const mode = ref<ThemeMode>((localStorage.getItem('vcp-theme-mode') as ThemeMode) || 'dark');
  const isDarkResolved = ref(true);
  const lastModeSwitchAt = ref(0);
  const MODE_SWITCH_DEBOUNCE_MS = 100;

  let initialTheme = localStorage.getItem('vcp-theme-name');
  if (initialTheme && LEGACY_THEME_MAP[initialTheme]) {
    initialTheme = LEGACY_THEME_MAP[initialTheme];
    localStorage.setItem('vcp-theme-name', initialTheme);
  }
  const currentTheme = ref(initialTheme || DEFAULT_THEME);

  const availableThemes = ref<ThemeInfo[]>([]);
  const currentThemeInfo = ref<ThemeInfo | null>(null);
  const lastAppliedVarKeys = ref<string[]>([]);
  let currentThemeModule: ThemeModule | null = null;

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

    for (const [path, loadModule] of Object.entries(themeModules)) {
      try {
        const mod = await loadModule();
        const fileName = path.split(/[\\/]/).pop() || '';

        if (fileName) {
          fileNameToLoader.set(fileName, loadModule);
        }

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
  };

  const applyThemeFile = async (fileName: string) => {
    try {
      currentTheme.value = fileName;
      localStorage.setItem('vcp-theme-name', fileName);

      const loadModule = findThemeLoader(fileName);

      if (!loadModule) {
        console.warn('Theme module not found:', fileName);
        return;
      }

      const mod = await loadModule();
      currentThemeModule = mod;
      currentThemeInfo.value = {
        fileName,
        name: mod.meta.name,
        variables: mod.variables,
      };

      const vars = isDarkResolved.value ? mod.variables.dark : mod.variables.light;
      injectVariables(vars);

      // Inject extra CSS rules (non-variable styles like .tool-bubble)
      let styleTag = document.getElementById('vcp-custom-theme');
      if (!styleTag) {
        styleTag = document.createElement('style');
        styleTag.id = 'vcp-custom-theme';
        document.head.appendChild(styleTag);
      }
      styleTag.textContent = mod.extraCss || '';
    } catch (error) {
      console.error('Failed to apply theme file:', error);
    }
  };

  const initTheme = async () => {
    const savedTheme = localStorage.getItem('vcp-theme-name') || DEFAULT_THEME;
    
    // 1. 优先只加载当前主题，确保背景和基础样式瞬间呈现
    await applyThemeFile(savedTheme);

    // 2. 优雅地在浏览器空闲时再扫描全量主题元数据
    const idleCallback = (window as any).requestIdleCallback || ((cb: any) => setTimeout(cb, 1000));
    idleCallback(() => {
      fetchThemes().catch(console.error);
    });
  };

  const applyTheme = (newMode: ThemeMode) => {
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
    const fileName = event.payload as string;
    if (fileName !== currentTheme.value) {
      applyThemeFile(fileName);
    }
  });

  onScopeDispose(() => {
    mediaQuery.removeEventListener('change', handleMediaChange);
    unlistenThemePromise.then((fn: UnlistenFn) => fn()).catch(() => {});
  });

  // Vite HMR: auto-reload theme when TS module is edited
  if (import.meta.hot) {
    import.meta.hot.accept(() => {
      const savedTheme = localStorage.getItem('vcp-theme-name') || DEFAULT_THEME;
      applyThemeFile(savedTheme);
    });
  }

  return {
    mode,
    isDarkResolved,
    currentTheme,
    currentThemeInfo,
    availableThemes,
    fetchThemes,
    applyThemeFile,
    initTheme,
    toggleTheme,
    setMode,
  };
});

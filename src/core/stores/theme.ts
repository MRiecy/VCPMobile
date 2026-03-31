import { defineStore } from 'pinia';
import { ref, watch } from 'vue';
import { listen } from '@tauri-apps/api/event';

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

const THEME_DISPLAY_NAMES: Record<string, string> = {
  'themes-ice-fire.css': '冰火魔歌',
  'themes-porcelain-brocade.css': '瓷与锦',
  'themes-crimson-sky.css': '绯红天穹',
  'themes-simple-bw.css': '黑白简约',
  'themes-quiet-forest.css': '静谧森岭',
  'themes-cartethyia.css': '卡提西亚',
  'themes-neon-coffee.css': '霓虹咖啡',
  'themes-childhood-dream.css': '童趣梦境',
  'themes-star-wolf.css': '星咏与狼嗥',
  'themes-star-abyss.css': '星渊雪境',
  'themes-bear-holiday.css': '熊熊假日',
  'themes-snow-morning.css': '雪境晨昏',
  'themes-night-sakura-cat.css': '夜樱猫语'
};

// Vite static asset imports
// ?raw gets the exact source text (unprocessed) for parsing variables and names
const rawThemes = import.meta.glob('../../assets/themes/*.css', { query: '?raw', eager: true, import: 'default' }) as Record<string, string>;
// ?inline gets the Vite-processed CSS (with valid asset URLs)
const inlineThemes = import.meta.glob('../../assets/themes/*.css', { query: '?inline', eager: true, import: 'default' }) as Record<string, string>;

export const useThemeStore = defineStore('theme', () => {
  const mode = ref<ThemeMode>((localStorage.getItem('vcp-theme-mode') as ThemeMode) || 'system');
  const isDarkResolved = ref(true);
  const lastModeSwitchAt = ref(0);
  const MODE_SWITCH_DEBOUNCE_MS = 500;
  
  let initialTheme = localStorage.getItem('vcp-theme-name');
  if (initialTheme && LEGACY_THEME_MAP[initialTheme]) {
    initialTheme = LEGACY_THEME_MAP[initialTheme];
    localStorage.setItem('vcp-theme-name', initialTheme);
  }
  const currentTheme = ref(initialTheme || DEFAULT_THEME);
  
  const availableThemes = ref<ThemeInfo[]>([]);

  const fetchThemes = async () => {
    // Parse themes purely on frontend
    const themes: ThemeInfo[] = [];
    
    for (const [path, content] of Object.entries(rawThemes)) {
      const fileName = path.split('/').pop() || '';
      
      const name = THEME_DISPLAY_NAMES[fileName] || fileName.replace('.css', '').replace('themes-', '');
      
      const extractVariables = (scopeRegex: RegExp) => {
        const variables: Record<string, string> = {};
        const scopeMatch = content.match(scopeRegex);
        if (scopeMatch && scopeMatch[1]) {
          const varRegex = /(--[\w-]+)\s*:\s*(.*?);/g;
          let match;
          while ((match = varRegex.exec(scopeMatch[1])) !== null) {
            variables[match[1]] = match[2].trim();
          }
        }
        return variables;
      };

      const rootScopeRegex = /:root\s*\{([\s\S]*?)\}/;
      const lightThemeScopeRegex = /body\.light-theme\s*\{([\s\S]*?)\}/;

      themes.push({
        fileName,
        name,
        variables: {
          dark: extractVariables(rootScopeRegex),
          light: extractVariables(lightThemeScopeRegex)
        }
      });
    }
    
    availableThemes.value = themes;
  };

  const applyThemeFile = async (fileName: string) => {
    try {
      currentTheme.value = fileName;
      localStorage.setItem('vcp-theme-name', fileName);
      
      const themePath = `../../assets/themes/${fileName}`;
      const css = inlineThemes[themePath];
      
      if (!css) {
        console.warn('Theme CSS not found in bundle:', fileName);
        return;
      }
      
      let styleTag = document.getElementById('vcp-custom-theme');
      if (!styleTag) {
        styleTag = document.createElement('style');
        styleTag.id = 'vcp-custom-theme';
        document.head.appendChild(styleTag);
      }
      styleTag.textContent = css;
      
      // Re-apply current mode to the new CSS rules
      applyTheme(mode.value);
    } catch (error) {
      console.error('Failed to apply theme file:', error);
    }
  };

  const initTheme = async () => {
    await fetchThemes();
    const savedTheme = localStorage.getItem('vcp-theme-name');
    if (savedTheme && inlineThemes[`../../assets/themes/${savedTheme}`]) {
      await applyThemeFile(savedTheme);
    } else {
      await applyThemeFile(DEFAULT_THEME);
    }
  };

  const applyTheme = (newMode: ThemeMode) => {
    const isDark =
      newMode === 'dark' ||
      (newMode === 'system' && window.matchMedia('(prefers-color-scheme: dark)').matches);

    isDarkResolved.value = isDark;
    document.documentElement.classList.toggle('dark', isDark);
    document.body.classList.toggle('light-theme', !isDark);
    localStorage.setItem('vcp-theme-mode', newMode);
  };

  watch(mode, (newMode) => {
    applyTheme(newMode);
  }, { immediate: true });

  // Listen for system theme changes
  window.matchMedia('(prefers-color-scheme: dark)').addEventListener('change', () => {
    if (mode.value === 'system') {
      applyTheme('system');
    }
  });

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
    setMode(mode.value === 'light' ? 'dark' : 'light');
  };
  // Listen for theme updates from backend
  listen('onThemeUpdated', (event) => {
    const fileName = event.payload as string;
    if (fileName !== currentTheme.value) {
      applyThemeFile(fileName);
    }
  });

  return {
    mode,
    isDarkResolved,
    currentTheme,
    availableThemes,
    fetchThemes,
    applyThemeFile,
    initTheme,
    toggleTheme,
    setMode,
  };
});

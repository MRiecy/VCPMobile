<script setup lang="ts">
import { onMounted, ref } from 'vue';
import { useThemeStore, type ThemeInfo } from '../../core/stores/theme';

// 为 layout 提供默认值，防止由于未传参导致的初始化失败
const props = withDefaults(defineProps<{
  layout?: 'horizontal' | 'grid'
}>(), {
  layout: 'grid'
});

const themeStore = useThemeStore();
const thumbnails = ref<Record<string, string>>({});

const loadThumbnails = async () => {
  if (!themeStore.availableThemes || themeStore.availableThemes.length === 0) return;

  for (const theme of themeStore.availableThemes) {
    const darkWallpaper = theme.variables?.dark?.['--chat-wallpaper-dark'];
    const lightWallpaper = theme.variables?.light?.['--chat-wallpaper-light'];
    let rawPath = darkWallpaper || lightWallpaper;

    if (rawPath && rawPath !== 'none') {
      try {
        // extract filename from url('../assets/wallpaper/xxx.png') or url('xxx.png')
        const match = rawPath.match(/url\(['"]?(.*?)['"]?\)/);
        let filename = match ? match[1] : rawPath;

        // Robust cleaning
        filename = filename.replace(/^.*[\\\/]/, '').replace(/['"]/g, '');
        filename = filename.split('.')[0] + '.jpg';

        thumbnails.value[theme.fileName] = `/wallpaper/${filename}`;
      } catch (e) {
        console.error('Failed to load thumbnail for', theme.fileName, e);
      }
    }
  }
};

onMounted(async () => {
  try {
    // Note: themeStore.fetchThemes() is now synchronous since it reads from Vite glob
    await themeStore.fetchThemes();
    await loadThumbnails();
  } catch (e) {
    console.error('[ThemePicker] Initialization failed:', e);
  }
});

const selectTheme = (theme: ThemeInfo) => {
  themeStore.applyThemeFile(theme.fileName);
};
</script>

<template>
  <div :class="props.layout === 'horizontal' ? 'flex overflow-x-auto space-x-4 pb-4 snap-x' : 'grid grid-cols-2 gap-4'">
    <div v-for="theme in themeStore.availableThemes" :key="theme.fileName" @click="selectTheme(theme)"
      class="relative flex-shrink-0 cursor-pointer rounded-xl overflow-hidden border-2 transition-all group" :class="[
        props.layout === 'horizontal' ? 'w-48 h-32 snap-start' : 'aspect-video',
        themeStore.currentTheme === theme.fileName ? 'border-primary shadow-lg scale-95' : 'border-transparent opacity-80'
      ]">
      <!-- Preview Image -->
      <div class="absolute inset-0 bg-cover bg-center transition-transform duration-500 group-hover:scale-110"
        :style="{ backgroundImage: thumbnails[theme.fileName] ? `url(${thumbnails[theme.fileName]})` : 'none' }">
        <div v-if="!thumbnails[theme.fileName]"
          class="w-full h-full flex items-center justify-center bg-gray-200 dark:bg-gray-800">
          <span class="text-xs text-gray-400">{{ theme.name }}</span>
        </div>
      </div>

      <!-- Overlays for visualization -->
      <div class="absolute inset-0 bg-black/20 group-hover:bg-black/10 transition-colors"></div>

      <!-- Theme Name Tag -->
      <div class="absolute bottom-2 left-2 right-2 bg-black/40 backdrop-blur-md rounded-lg px-2 py-1">
        <p class="text-[10px] text-white font-medium truncate text-center">{{ theme.name }}</p>
      </div>

      <!-- Selection Indicator -->
      <div v-if="themeStore.currentTheme === theme.fileName"
        class="absolute top-2 left-2 bg-primary text-white p-1 rounded-full shadow-sm z-10 flex items-center justify-center">
        <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="4"
          stroke-linecap="round" stroke-linejoin="round">
          <polyline points="20 6 9 17 4 12"></polyline>
        </svg>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* Hide scrollbar but allow scrolling */
.overflow-x-auto {
  scrollbar-width: none;
  -ms-overflow-style: none;
}

.overflow-y-auto::-webkit-scrollbar {
  display: none;
}
</style>

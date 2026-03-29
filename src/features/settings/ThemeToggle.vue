<script setup lang="ts">
import { useThemeStore } from '../../core/stores/theme';
import { Sun, Moon, Laptop } from 'lucide-vue-next';

const themeStore = useThemeStore();
</script>

<template>
  <div class="flex p-1 bg-gray-100 dark:bg-gray-700 rounded-xl w-fit">
    <button
      @click="themeStore.setMode('light')"
      class="flex-center p-2 rounded-lg transition-all"
      :class="themeStore.mode === 'light' ? 'bg-white dark:bg-gray-600 shadow-sm text-primary' : 'text-gray-500'"
    >
      <Sun :size="20" />
    </button>
    <button
      @click="themeStore.setMode('dark')"
      class="flex-center p-2 rounded-lg transition-all"
      :class="themeStore.mode === 'dark' ? 'bg-white dark:bg-gray-600 shadow-sm text-primary' : 'text-gray-500'"
    >
      <Moon :size="20" />
    </button>
    <button
      @click="themeStore.setMode('system')"
      class="flex-center p-2 rounded-lg transition-all"
      :class="themeStore.mode === 'system' ? 'bg-white dark:bg-gray-600 shadow-sm text-primary' : 'text-gray-500'"
    >
      <Laptop :size="20" />
    </button>
  </div>
</template>

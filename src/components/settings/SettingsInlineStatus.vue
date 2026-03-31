<script setup lang="ts">
defineProps<{
  type?: 'success' | 'error' | 'loading' | 'info';
  message: string;
  mono?: boolean;
}>();
</script>

<template>
  <div 
    class="settings-status text-[10px] font-medium transition-all duration-300"
    :class="[
      type === 'success' ? 'text-green-500 dark:text-green-400' : '',
      type === 'error' ? 'text-red-500 dark:text-red-400' : '',
      type === 'loading' ? 'text-blue-500 dark:text-blue-400 animate-pulse' : '',
      type === 'info' ? 'text-primary-text opacity-40' : '',
      mono ? 'font-mono' : ''
    ]"
  >
    <div class="flex items-center gap-1.5">
      <div v-if="type === 'loading'" class="w-1.5 h-1.5 rounded-full bg-current animate-ping"></div>
      <span class="truncate">{{ message }}</span>
    </div>
  </div>
</template>

<script setup lang="ts">
defineProps<{
  variant?: 'primary' | 'secondary' | 'danger' | 'ghost';
  disabled?: boolean;
  loading?: boolean;
  icon?: any;
  fullWidth?: boolean;
  size?: 'sm' | 'md' | 'lg';
}>();

const emit = defineEmits<{
  (e: 'click'): void;
}>();
</script>

<template>
  <button 
    class="settings-button flex items-center justify-center gap-2 transition-all duration-200 active:scale-95 disabled:opacity-40 disabled:pointer-events-none font-bold tracking-wider"
    :class="[
      fullWidth ? 'w-full' : 'px-5',
      size === 'sm' ? 'py-2 text-[11px] rounded-xl' : (size === 'lg' ? 'py-4 text-[14px] rounded-2xl' : 'py-2.5 text-[12px] rounded-xl'),
      variant === 'primary' ? 'bg-blue-600 hover:bg-blue-500 text-white shadow-lg shadow-blue-900/20' : '',
      variant === 'secondary' ? 'bg-black/5 dark:bg-white/10 hover:bg-black/10 dark:hover:bg-white/20 text-primary-text' : '',
      variant === 'danger' ? 'bg-red-500/10 hover:bg-red-500/20 text-red-500' : '',
      variant === 'ghost' ? 'bg-transparent hover:bg-black/5 dark:hover:bg-white/5 text-primary-text opacity-60 hover:opacity-100' : ''
    ]"
    :disabled="disabled || loading"
    @click="emit('click')"
  >
    <div v-if="loading" class="w-3 h-3 rounded-full border-2 border-current border-t-transparent animate-spin shrink-0"></div>
    <component v-else-if="icon" :is="icon" :size="size === 'sm' ? 14 : 16" class="shrink-0" />
    <slot></slot>
  </button>
</template>

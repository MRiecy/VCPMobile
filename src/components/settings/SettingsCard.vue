<script setup lang="ts">
defineProps<{
  variant?: 'default' | 'glass' | 'danger' | 'warning';
  noPadding?: boolean;
}>();
</script>

<template>
  <div 
    class="settings-card overflow-hidden transition-all duration-300"
    :class="[
      variant === 'glass' ? 'bg-white/5 backdrop-blur-xl border-white/10' : 'bg-black/5 dark:bg-white/5 border-black/5 dark:border-white/10',
      variant === 'danger' ? 'border-red-500/20 bg-red-500/5' : '',
      variant === 'warning' ? 'border-yellow-500/20 bg-yellow-500/5' : '',
      'border rounded-2xl shadow-sm',
      noPadding ? '' : 'p-4'
    ]"
  >
    <slot></slot>
  </div>
</template>

<style scoped>
.settings-card {
  /* 确保在黑白模式下也有足够的对比度 */
  box-shadow: 0 2px 8px -2px rgba(0, 0, 0, 0.05);
}
</style>

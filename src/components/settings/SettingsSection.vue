<script setup lang="ts">
defineProps<{
  title: string;
  description?: string;
  accentColor?: string;
}>();
</script>

<template>
  <section class="space-y-3">
    <div class="flex items-center gap-2 px-1">
      <div 
        class="w-1 h-4 rounded-full shrink-0" 
        :class="accentColor || 'bg-blue-500'"
      ></div>
      <div class="flex flex-col min-w-0">
        <h3 class="text-[11px] font-black uppercase tracking-[0.15em] opacity-50 dark:opacity-40 truncate">
          {{ title }}
        </h3>
        <p v-if="description" class="text-[9px] opacity-30 leading-tight mt-0.5">
          {{ description }}
        </p>
      </div>
    </div>
    <slot></slot>
  </section>
</template>

<script setup lang="ts">
import { useSlots } from 'vue';

defineProps<{
  title: string;
  description?: string;
  icon?: any;
  clickable?: boolean;
  danger?: boolean;
  disabled?: boolean;
}>();

const emit = defineEmits<{
  (e: 'click'): void;
}>();

const slots = useSlots();
</script>

<template>
  <div 
    class="settings-row flex items-center justify-between py-3.5 px-1 transition-all duration-200"
    :class="[
      clickable && !disabled ? 'cursor-pointer active:scale-[0.98] active:bg-black/5 dark:active:bg-white/5 rounded-xl' : '',
      disabled ? 'opacity-40 pointer-events-none' : ''
    ]"
    @click="clickable && !disabled && emit('click')"
  >
    <div class="flex items-center gap-3 min-w-0 flex-1">
      <div v-if="icon" class="shrink-0 opacity-60">
        <component :is="icon" :size="18" />
      </div>
      <div class="flex flex-col min-w-0">
        <span 
          class="text-[14px] font-semibold truncate"
          :class="danger ? 'text-red-500' : 'text-primary-text'"
        >
          {{ title }}
        </span>
        <span v-if="description" class="text-[10px] opacity-40 leading-tight mt-0.5">
          {{ description }}
        </span>
      </div>
    </div>
    
    <div class="flex items-center gap-2 shrink-0 ml-4">
      <slot name="action"></slot>
      <div v-if="clickable && !slots.action" class="opacity-20">
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="m9 18 6-6-6-6"/></svg>
      </div>
    </div>
  </div>
</template>


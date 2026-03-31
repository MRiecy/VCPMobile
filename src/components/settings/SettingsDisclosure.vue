<script setup lang="ts">
import { ref } from 'vue';

const props = defineProps<{
  title: string;
  description?: string;
  icon?: any;
  defaultOpen?: boolean;
  accentColor?: string;
}>();

const isOpen = ref(props.defaultOpen || false);

const toggle = () => {
  isOpen.value = !isOpen.value;
};
</script>

<template>
  <div 
    class="settings-disclosure border border-black/5 dark:border-white/10 rounded-2xl overflow-hidden transition-all duration-300"
    :class="[
      isOpen ? 'bg-black/[0.02] dark:bg-white/[0.02] shadow-sm' : 'bg-transparent'
    ]"
  >
    <button 
      @click="toggle"
      class="w-full flex items-center justify-between p-4 hover:bg-black/5 dark:hover:bg-white/5 transition-colors"
    >
      <div class="flex items-center gap-3 min-w-0">
        <div 
          v-if="icon" 
          class="shrink-0 opacity-60"
          :class="accentColor"
        >
          <component :is="icon" :size="18" />
        </div>
        <div class="flex flex-col items-start min-w-0">
          <span class="text-[14px] font-bold truncate text-primary-text">
            {{ title }}
          </span>
          <span v-if="description" class="text-[10px] opacity-40 leading-tight mt-0.5 text-left">
            {{ description }}
          </span>
        </div>
      </div>
      
      <div 
        class="shrink-0 opacity-30 transition-transform duration-300"
        :class="{ 'rotate-180': isOpen }"
      >
        <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="m6 9 6 6 6-6"/></svg>
      </div>
    </button>
    
    <Transition
      enter-active-class="transition-all duration-300 ease-out"
      leave-active-class="transition-all duration-200 ease-in"
      enter-from-class="max-h-0 opacity-0"
      enter-to-class="max-h-[2000px] opacity-100"
      leave-from-class="max-h-[2000px] opacity-100"
      leave-to-class="max-h-0 opacity-0"
    >
      <div v-if="isOpen" class="px-4 pb-4 overflow-hidden">
        <div class="pt-2 border-t border-black/5 dark:border-white/5">
          <slot></slot>
        </div>
      </div>
    </Transition>
  </div>
</template>

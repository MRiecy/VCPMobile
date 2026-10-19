<script setup lang="ts">
import { ref, useId } from 'vue';

const props = defineProps<{
  title: string;
  description?: string;
  icon?: any;
  defaultOpen?: boolean;
  accentColor?: string;
  disabled?: boolean;
}>();

const isOpen = ref(props.defaultOpen || false);
const contentId = useId();

const toggle = () => {
  if (!props.disabled) isOpen.value = !isOpen.value;
};
</script>

<template>
  <div
    class="settings-disclosure border border-black/5 dark:border-white/10 rounded-2xl overflow-hidden transition-all duration-300"
    :class="[
      isOpen ? 'bg-black/[0.02] dark:bg-white/[0.02] shadow-sm' : 'bg-transparent'
    ]">
    <button type="button" :disabled="disabled" :aria-expanded="isOpen" :aria-controls="contentId" @click="toggle"
      class="settings-disclosure-toggle w-full flex items-center justify-between p-4 hover:bg-black/5 dark:hover:bg-white/5 transition-colors">
      <div class="flex items-center gap-3 min-w-0">
        <div v-if="icon" class="shrink-0 opacity-60" :class="accentColor">
          <component :is="icon" :size="18" />
        </div>
        <div class="flex flex-col items-start min-w-0">
          <span class="settings-disclosure-title text-[14px] font-bold truncate text-primary-text">
            {{ title }}
          </span>
          <span v-if="description" class="settings-disclosure-description text-[10px] opacity-40 leading-tight mt-0.5 text-left">
            {{ description }}
          </span>
        </div>
      </div>

      <div class="shrink-0 opacity-30 transition-transform duration-300" :class="{ 'rotate-180': isOpen }">
        <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"
          stroke-linecap="round" stroke-linejoin="round">
          <path d="m6 9 6 6 6-6" />
        </svg>
      </div>
    </button>

    <div v-if="isOpen" :id="contentId" class="settings-disclosure-content px-4 pb-4">
      <div class="settings-disclosure-body pt-2 border-t border-black/5 dark:border-white/5">
        <slot></slot>
      </div>
    </div>
  </div>
</template>

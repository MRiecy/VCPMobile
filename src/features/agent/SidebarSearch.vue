<script setup lang="ts">
import { computed } from 'vue';

const props = defineProps<{
  modelValue: string;
  activeTab: 'agents' | 'topics';
}>();

const emit = defineEmits<{
  (e: 'update:modelValue', value: string): void;
}>();

const placeholderText = computed(() => {
  return props.activeTab === 'agents' ? '搜索助手...' : '搜索话题...';
});
</script>

<template>
  <div class="relative group">
    <svg class="absolute left-3 top-1/2 -translate-y-1/2 opacity-50 w-4 h-4 text-primary-text transition-opacity group-focus-within:opacity-100" viewBox="0 0 24 24"
      fill="none" stroke="currentColor" stroke-width="2">
      <circle cx="11" cy="11" r="8"></circle>
      <line x1="21" y1="21" x2="16.65" y2="16.65"></line>
    </svg>
    <input :value="modelValue" @input="emit('update:modelValue', ($event.target as HTMLInputElement).value)" type="text"
      :placeholder="placeholderText"
      class="w-full bg-black/5 dark:bg-white/5 text-primary-text placeholder:text-secondary-text/50 text-sm rounded-xl py-2.5 pl-10 pr-9 outline-none border border-black/5 dark:border-white/10 focus:border-blue-500/50 dark:focus:border-blue-400/50 focus:bg-white/10 dark:focus:bg-white/10 transition-all shadow-inner" />
    <button v-if="modelValue" @click="emit('update:modelValue', '')" @mousedown.prevent class="absolute right-3 top-1/2 -translate-y-1/2 text-secondary-text hover:text-primary-text opacity-60 hover:opacity-100 transition-all p-1 flex items-center justify-center rounded-full active:scale-95" aria-label="Clear search">
      <svg class="w-3.5 h-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
        <line x1="18" y1="6" x2="6" y2="18"></line>
        <line x1="6" y1="6" x2="18" y2="18"></line>
      </svg>
    </button>
  </div>
</template>

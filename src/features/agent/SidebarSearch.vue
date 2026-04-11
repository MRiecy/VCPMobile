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
  <div class="relative">
    <svg class="absolute left-3 top-1/2 -translate-y-1/2 opacity-40 w-4 h-4 text-primary-text" viewBox="0 0 24 24"
      fill="none" stroke="currentColor" stroke-width="2">
      <circle cx="11" cy="11" r="8"></circle>
      <line x1="21" y1="21" x2="16.65" y2="16.65"></line>
    </svg>
    <input :value="modelValue" @input="emit('update:modelValue', ($event.target as HTMLInputElement).value)" type="text"
      :placeholder="placeholderText"
      class="w-full bg-black/5 dark:bg-black/20 text-primary-text text-sm rounded-xl py-2 pl-9 pr-4 outline-none border border-black/5 dark:border-white/5 focus:border-black/20 dark:focus:border-white/20 transition-colors" />
  </div>
</template>

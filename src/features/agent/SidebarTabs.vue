<script setup lang="ts">
defineProps<{
  activeTab: 'agents' | 'topics';
}>();

const emit = defineEmits<{
  (e: 'update:activeTab', value: 'agents' | 'topics'): void;
}>();
</script>

<template>
  <div class="flex p-1 bg-black/5 dark:bg-white/5 rounded-xl mb-4 border border-black/5 dark:border-white/10">
    <button @click="emit('update:activeTab', 'agents')"
      class="flex-1 py-1.5 text-sm font-bold rounded-lg transition-all active:scale-[0.97]"
      :class="activeTab === 'agents' 
        ? 'bg-white text-gray-800 shadow-sm dark:bg-white/20 dark:text-white' 
        : 'text-secondary-text hover:text-primary-text hover:bg-black/5 dark:hover:bg-white/5'">
      助手
    </button>
    <button @click="emit('update:activeTab', 'topics')"
      class="flex-1 py-1.5 text-sm font-bold rounded-lg transition-all active:scale-[0.97]"
      :class="activeTab === 'topics' 
        ? 'bg-white text-gray-800 shadow-sm dark:bg-white/20 dark:text-white' 
        : 'text-secondary-text hover:text-primary-text hover:bg-black/5 dark:hover:bg-white/5'">
      话题
    </button>
  </div>
</template>

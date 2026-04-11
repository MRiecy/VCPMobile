<script setup lang="ts">
defineProps<{
  activeTab: 'agents' | 'topics';
}>();

const emit = defineEmits<{
  (e: 'update:activeTab', value: 'agents' | 'topics'): void;
}>();
</script>

<template>
  <div class="flex p-1 bg-black/5 dark:bg-black/20 rounded-xl mb-4 border border-black/5 dark:border-white/5">
    <button @click="emit('update:activeTab', 'agents')"
      class="flex-1 py-1.5 text-sm font-bold rounded-lg transition-all"
      :class="activeTab === 'agents' ? 'bg-white shadow-sm text-gray-800 dark:bg-white/10 dark:text-white dark:shadow-sm' : 'text-gray-500 hover:text-gray-700 dark:text-white/40 dark:hover:text-white/60'">
      助手
    </button>
    <button @click="emit('update:activeTab', 'topics')"
      class="flex-1 py-1.5 text-sm font-bold rounded-lg transition-all"
      :class="activeTab === 'topics' ? 'bg-white shadow-sm text-gray-800 dark:bg-white/10 dark:text-white dark:shadow-sm' : 'text-gray-500 hover:text-gray-700 dark:text-white/40 dark:hover:text-white/60'">
      话题
    </button>
  </div>
</template>

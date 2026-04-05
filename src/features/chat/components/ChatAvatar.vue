<script setup lang="ts">
defineProps<{
  imageUrl?: string | null;
  fallbackText: string;
  isUser: boolean;
  borderColor?: string;
}>();
</script>

<template>
  <div class="flex-shrink-0 w-9 h-9">
    <div
      class="w-full h-full rounded-full overflow-hidden border transition-all duration-500 shadow-sm"
      :class="
        isUser ? 'bg-primary border-primary' : 'bg-white dark:bg-gray-800'
      "
      :style="!isUser ? { borderColor: borderColor || 'transparent' } : {}"
    >
      <img v-if="imageUrl" :src="imageUrl" class="w-full h-full object-cover" />
      <div
        v-else
        class="w-full h-full flex items-center justify-center text-xs font-bold text-white"
        :class="isUser ? 'bg-primary' : 'bg-gray-700'"
      >
        {{ fallbackText.charAt(0).toUpperCase() }}
      </div>
    </div>
  </div>
</template>

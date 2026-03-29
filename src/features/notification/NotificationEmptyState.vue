<template>
  <div class="h-full flex flex-col items-center justify-center opacity-20 text-center p-8">
    <Bell :size="48" stroke-width="1" class="mb-4 text-primary-text" />
    <div class="text-[10px] uppercase tracking-[0.2em] font-light text-primary-text">No notifications yet</div>
  </div>
</template>

<script setup lang="ts">
import { Bell } from 'lucide-vue-next';
</script>

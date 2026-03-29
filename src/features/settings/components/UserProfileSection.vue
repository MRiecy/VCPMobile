<script setup lang="ts">
import type { AppSettings } from '../../../core/stores/settings';

defineProps<{
  settings: AppSettings;
}>();
</script>

<template>
  <section class="card-modern">
    <div class="flex items-center gap-5">
      <div class="w-20 h-20 rounded-full bg-black/5 dark:bg-white/5 flex-center relative overflow-hidden border-2 border-black/5 dark:border-white/10 shadow-inner">
        <img v-if="settings.userAvatarUrl" :src="settings.userAvatarUrl" class="w-full h-full object-cover" />
        <svg v-else width="40" height="40" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round" class="opacity-20"><path d="M20 21v-2a4 4 0 0 0-4-4H8a4 4 0 0 0-4 4v2"></path><circle cx="12" cy="7" r="4"></circle></svg>
      </div>
      <div class="flex-1">
        <label class="text-[11px] uppercase font-black tracking-widest opacity-40 dark:opacity-30 mb-1.5 block">用户名</label>
        <input v-model="settings.userName" class="bg-transparent border-b border-black/10 dark:border-white/10 w-full focus:border-blue-500 outline-none py-1.5 text-lg font-medium transition-colors text-primary-text" />
      </div>
    </div>
  </section>
</template>

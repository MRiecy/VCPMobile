<script setup lang="ts">
import type { AppSettings } from '../../../core/stores/settings';

defineProps<{
  settings: AppSettings;
}>();

const emit = defineEmits<{
  (e: 'open-model-selector'): void;
}>();
</script>

<template>
  <section class="space-y-4">
    <div class="flex items-center gap-2 px-1">
      <div class="w-1 h-4 bg-yellow-500 rounded-full"></div>
      <h3 class="text-xs font-black uppercase tracking-[0.2em] opacity-50 dark:opacity-40">话题总结 (Topic Summary)</h3>
    </div>
    <div class="card-modern">
      <label class="text-[11px] uppercase font-bold opacity-50 dark:opacity-40 mb-2 block">总结专用模型</label>
      <div class="flex gap-2">
        <input v-model="settings.topicSummaryModel" placeholder="默认: gemini-2.5-flash" class="flex-1 bg-black/5 dark:bg-white/5 p-3.5 rounded-2xl outline-none border border-black/5 dark:border-white/5 focus:border-yellow-500/50 focus:bg-black/10 dark:focus:bg-white/10 transition-all font-mono text-sm" />
        <button @click="emit('open-model-selector')" class="w-12 h-12 bg-yellow-500/10 text-yellow-500 rounded-2xl flex-center active:scale-90 transition-all">
          <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M8.25 15L12 18.75 15.75 15m-7.5-6L12 5.25 15.75 9"></path></svg>
        </button>
      </div>
    </div>
  </section>
</template>

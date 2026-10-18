<script setup lang="ts">
import { ref } from 'vue';
import HapticsDebugPanel from './HapticsDebugPanel.vue';
import { useDeveloperSettingsStore } from './store';

const props = defineProps<{ active: boolean }>();
const emit = defineEmits<{ back: [] }>();
const developerSettings = useDeveloperSettingsStore();
const error = ref('');

function disableDeveloperMode() {
  if (!props.active) return;
  const result = developerSettings.setEnabled(false);
  error.value = result.ok ? '' : `关闭失败：${result.error}`;
  if (result.ok) emit('back');
}
</script>

<template>
  <div :inert="!active" class="space-y-6" data-testid="developer-settings-page">
    <HapticsDebugPanel :active="active && developerSettings.enabled" />
    <div class="border-t border-[var(--vcp-border-subtle)] pt-4">
      <button type="button" :disabled="!active" class="min-h-11 w-full border border-[var(--vcp-border-subtle)] rounded-lg px-3 text-sm text-[var(--primary-text)] active:bg-[var(--vcp-highlight-bg-10)] transition-colors"
        @click="disableDeveloperMode">关闭开发者模式</button>
      <p v-if="error" class="mt-2 text-xs text-red-500" role="alert">{{ error }}</p>
    </div>
  </div>
</template>

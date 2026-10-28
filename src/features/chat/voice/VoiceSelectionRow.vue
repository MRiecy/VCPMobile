<script setup lang="ts">
import { ChevronRight } from '@lucide/vue';
import SettingsRow from '../../../components/settings/SettingsRow.vue';

defineProps<{ title: string; value: string; disabled?: boolean; compact?: boolean; expanded?: boolean }>();
const emit = defineEmits<{ click: [event: MouseEvent] }>();
</script>

<template>
  <button v-if="compact" type="button" class="voice-selection-row" :disabled="disabled" :aria-label="title"
    aria-haspopup="dialog" :aria-expanded="expanded" @click="emit('click', $event)">
    <span>{{ title }}</span><span class="voice-selection-value">{{ value }}</span><ChevronRight :size="14" aria-hidden="true" />
  </button>
  <SettingsRow v-else :title="title" :value="value" clickable :disabled="disabled" :aria-label="title"
    aria-haspopup="dialog" :aria-expanded="expanded" @click="emit('click', $event)" />
</template>

<style scoped>
.voice-selection-row { width: 100%; display: flex; align-items: center; gap: 8px; min-height: 48px; padding: 10px 16px; border: 0; border-bottom: 1px solid var(--voice-row-divider, var(--border-color)); background: transparent; color: var(--primary-text); text-align: left; font-size: 13px; line-height: 20px; transition: background-color 150ms; touch-action: manipulation; }
.voice-selection-value { min-width: 0; max-width: 65%; margin-left: auto; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--highlight-text, #3b82f6); }
.voice-selection-row svg { flex-shrink: 0; color: var(--secondary-text); }
.voice-selection-row:disabled { opacity: 0.4; }
.voice-selection-row:enabled:active { background: color-mix(in srgb, var(--primary-text) 4%, transparent); }
.voice-selection-row:focus-visible { outline: 2px solid var(--voice-control-link, var(--highlight-text)); outline-offset: -3px; }
</style>

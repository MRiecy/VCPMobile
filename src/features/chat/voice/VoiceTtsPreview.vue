<script setup lang="ts">
import { computed, ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { Play, Square } from '@lucide/vue';
import SettingsDisclosure from '../../../components/settings/SettingsDisclosure.vue';
import VoiceTextArea from './VoiceTextArea.vue';
import { useVoicePreviewTask } from './useVoicePreviewTask';
import type { VoiceConfig } from '../../../core/config/generated/config';
import { cloneConfig } from '../../../core/config/fields';

const props = defineProps<{ settings: VoiceConfig; active: boolean; disabled?: boolean }>();
const text = ref('(慵懒)再让我睡五分钟……就五分钟，真的，最后一次。');
const { busy, feedback, unavailable, canStart: previewAvailable, start, stop } = useVoicePreviewTask(
  () => props.active, () => !!props.disabled,
);
const canStart = computed(() => previewAvailable.value && !!text.value.trim());
const toggle = async () => {
  if (busy.value) { await stop().catch(() => undefined); return; }
  if (!canStart.value) return;
  // Freeze the visible draft before any asynchronous admission.
  const settings = cloneConfig({ connection: props.settings.connection, tts: props.settings.tts });
  const previewText = text.value.trim();
  await start(ttsId => invoke('preview_tts', { ttsId, text: previewText, settings }));
};
defineExpose({ stop, busy });
</script>

<template>
  <SettingsDisclosure title="试听声音" :description="unavailable" :disabled="disabled">
    <div class="flex flex-col gap-3">
      <VoiceTextArea v-model="text" label="试听文本" hide-label :rows="3" :max-length="500" :disabled="disabled" />
      <button type="button" class="voice-preview-button" :disabled="!active || disabled || (!busy && !canStart)" @click="toggle">
        <Square v-if="busy" :size="16" aria-hidden="true" /><Play v-else :size="16" aria-hidden="true" />
        {{ busy ? '停止试听' : '试听当前设置' }}
      </button>
      <p v-if="feedback" role="alert" class="m-0 text-xs leading-relaxed text-red-500">{{ feedback }}</p>
    </div>
  </SettingsDisclosure>
</template>

<style scoped>
.voice-preview-button { display: inline-flex; align-self: flex-start; align-items: center; justify-content: center; gap: 8px; min-height: 44px; padding: 8px 12px; border: 1px solid var(--border-color); border-radius: 8px; background: transparent; color: inherit; font-size: 13px; touch-action: manipulation; }
.voice-preview-button:disabled { opacity: 0.4; }
</style>

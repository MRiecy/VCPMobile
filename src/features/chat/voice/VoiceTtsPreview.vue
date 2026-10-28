<script setup lang="ts">
import { computed, ref, useId } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { Play, Square } from '@lucide/vue';
import SettingsDisclosure from '../../../components/settings/SettingsDisclosure.vue';
import VoiceTextArea from './VoiceTextArea.vue';
import { useVoicePreviewTask } from './useVoicePreviewTask';
import type { VoiceConfig } from '../../../core/config/generated/config';
import { cloneConfig } from '../../../core/config/fields';

const props = defineProps<{ settings: VoiceConfig; active: boolean; disabled?: boolean; locked?: boolean; compact?: boolean }>();
const inputId = useId();
const text = ref(props.compact ? '你好，我是当前助手的音色。' : '你好，很高兴认识你。');
const { busy, feedback, unavailable, canStart: previewAvailable, start, stop } = useVoicePreviewTask(
  () => props.active, () => !!props.disabled || !!props.locked,
);
const canStart = computed(() => previewAvailable.value && !!text.value.trim());
const buttonDisabled = computed(() => !!props.disabled || (!busy.value && (!!unavailable.value || !text.value.trim())));
const toggle = async () => {
  if (!props.active || props.locked) return;
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
  <div v-if="compact" class="voice-preview-compact" :inert="!active || locked">
    <label :for="inputId" class="voice-preview-label">音色试听</label>
    <div class="voice-preview-inline">
      <label :for="inputId" class="voice-preview-input-target">
        <input :id="inputId" v-model="text" type="text" maxlength="500" :disabled="disabled" aria-label="试听文本" class="voice-preview-input" />
      </label>
      <button type="button" class="voice-preview-button voice-preview-button--compact"
        :disabled="buttonDisabled" @click="toggle">{{ busy ? '停止' : '试听' }}</button>
    </div>
    <p v-if="unavailable" class="m-0 mt-2 text-xs text-[var(--secondary-text)]">{{ unavailable }}</p>
    <p v-if="feedback" role="alert" class="m-0 mt-2 text-xs leading-relaxed text-red-500">{{ feedback }}</p>
  </div>
  <SettingsDisclosure v-else title="试听声音" :description="unavailable" :disabled="disabled" :inert="!active || locked">
    <div class="flex flex-col gap-3">
      <VoiceTextArea v-model="text" label="试听文本" hide-label :rows="3" :max-length="500" :disabled="disabled" />
      <button type="button" class="voice-preview-button" :disabled="buttonDisabled" @click="toggle">
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
.voice-preview-compact { min-width: 0; }
.voice-preview-label { display: block; margin-bottom: 4px; color: var(--secondary-text); font-size: 12px; line-height: 20px; }
.voice-preview-inline { display: flex; align-items: center; gap: 8px; }
.voice-preview-input-target { display: flex; flex: 1; align-items: center; min-width: 0; min-height: 44px; }
.voice-preview-input { width: 100%; min-width: 0; min-height: 32px; padding: 5px 10px; border: 1px solid var(--voice-control-border, var(--border-color)); border-radius: 6px; background: var(--voice-control-bg, transparent); color: var(--primary-text); font: inherit; font-size: 12px; line-height: 20px; }
.voice-preview-button--compact { position: relative; align-self: center; flex-shrink: 0; min-width: 52px; min-height: 32px; padding: 6px 12px; border: 0; border-radius: 6px; background: var(--voice-control-accent, #2563eb); color: white; font-size: 12px; line-height: 20px; }
.voice-preview-button--compact::after { content: ''; position: absolute; inset: -6px 0; }
.voice-preview-button--compact:enabled:active { opacity: 0.75; }
.voice-preview-input:focus-visible, .voice-preview-button:focus-visible { outline: 2px solid var(--voice-control-link, var(--highlight-text)); outline-offset: 2px; }
html[data-vcp-platform="ios"] .voice-preview-input { font-size: 16px; }
</style>

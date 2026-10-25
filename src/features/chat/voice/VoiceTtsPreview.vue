<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { Play, Square } from '@lucide/vue';
import SettingsDisclosure from '../../../components/settings/SettingsDisclosure.vue';
import VoiceTextArea from './VoiceTextArea.vue';
import { useVoiceCallStore } from './voiceCallStore';
import type { VoiceConfig } from '../../../core/config/generated/config';
import { cloneConfig } from '../../../core/config/fields';

const props = defineProps<{ settings: VoiceConfig; active: boolean; disabled?: boolean }>();
const voiceCall = useVoiceCallStore();
const text = ref('(慵懒)再让我睡五分钟……就五分钟，真的，最后一次。');
const busy = ref(false);
const feedback = ref('');
interface PreviewTask {
  id: string;
  submitted: boolean;
  cancelled: boolean;
  ready: Promise<void>;
  stopping?: Promise<void>;
  unlisten?: UnlistenFn;
}
let task: PreviewTask | null = null;
let mounted = true;
const unavailable = computed(() => voiceCall.active ? '通话结束后可试听。'
  : voiceCall.playbackActive ? '请等待当前朗读结束后再试听。' : undefined);
const canStart = computed(() => props.active && !props.disabled && !voiceCall.active && !voiceCall.playbackActive && !!text.value.trim());

const stop = (): Promise<void> => {
  const current = task;
  if (!current) return Promise.resolve();
  if (current.stopping) return current.stopping;
  current.cancelled = true;
  current.unlisten?.();
  const operation = current.ready.catch(() => undefined).then(async () => {
    // The start acknowledgement may arrive after Back. Do not leave until that
    // particular task has also released the native playback slot.
    if (current.submitted) await invoke('stop_tts', { ttsId: current.id });
    if (task === current) {
      task = null;
      if (mounted) busy.value = false;
    }
  }).catch(error => {
    current.stopping = undefined;
    if (mounted && props.active) feedback.value = '停止试听失败，请重试。';
    throw error;
  });
  current.stopping = operation;
  return operation;
};
const toggle = async () => {
  if (busy.value) { await stop().catch(() => undefined); return; }
  if (!canStart.value) return;
  if (task) {
    try { await stop(); } catch { return; }
    if (!canStart.value) return;
  }
  const current: PreviewTask = {
    id: `voice-preview-${crypto.randomUUID()}`, submitted: false, cancelled: false,
    ready: Promise.resolve(),
  };
  task = current;
  busy.value = true;
  feedback.value = '';
  // Freeze the visible draft before any asynchronous admission.
  const settings = cloneConfig({ connection: props.settings.connection, tts: props.settings.tts });
  const previewText = text.value.trim();
  current.ready = (async () => {
    const unlisten = await listen<{ ttsId: string; kind: string; stage: string; message?: string }>('vcp-tts', event => {
      if (!mounted || current.cancelled || task !== current || event.payload.ttsId !== current.id) return;
      if (event.payload.kind === 'error' || event.payload.kind === 'stopped') {
        feedback.value = event.payload.kind === 'error' ? event.payload.message || '试听失败，请检查语音设置后重试。' : '';
        busy.value = false;
        current.unlisten?.();
      }
    });
    current.unlisten = unlisten;
    if (!mounted || current.cancelled || task !== current || !canStart.value) { unlisten(); return; }
    current.submitted = true;
    await invoke('preview_tts', { ttsId: current.id, text: previewText, settings });
  })();
  try {
    await current.ready;
  } catch {
    if (mounted && !current.cancelled && task === current) {
      feedback.value = '试听未能开始，请检查连接、音色或参考音频后重试。';
      await stop().catch(() => undefined);
    }
  }
};
watch(() => props.active, active => { if (!active) void stop().catch(() => undefined); }, { flush: 'sync' });
onBeforeUnmount(() => { mounted = false; void stop().catch(() => undefined); });
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

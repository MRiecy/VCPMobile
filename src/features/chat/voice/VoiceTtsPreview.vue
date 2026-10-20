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
const text = ref('你好，很高兴和你聊天。我们慢慢说，一起把事情理清楚。');
const busy = ref(false);
const feedback = ref('');
let task: { id: string; submitted: boolean; unlisten?: UnlistenFn } | null = null;
const unavailable = computed(() => voiceCall.active ? '通话结束后可试听。'
  : voiceCall.playbackActive ? '请等待当前朗读结束后再试听。' : '使用当前草稿试听，不会保存设置。');
const canStart = computed(() => props.active && !props.disabled && !voiceCall.active && !voiceCall.playbackActive && !!text.value.trim());

const stop = async () => {
  const current = task;
  task = null;
  busy.value = false;
  current?.unlisten?.();
  if (current?.submitted) {
    await invoke('stop_tts', { ttsId: current.id }).catch(() => {
      if (props.active) feedback.value = '停止试听失败，请稍后重试。';
    });
  }
};
const toggle = async () => {
  if (busy.value) { await stop(); return; }
  if (!canStart.value) return;
  const current = { id: `voice-preview-${crypto.randomUUID()}`, submitted: false } as NonNullable<typeof task>;
  task = current;
  busy.value = true;
  feedback.value = '';
  // Freeze the visible draft before any asynchronous admission.
  const settings = cloneConfig({ connection: props.settings.connection, tts: props.settings.tts });
  const previewText = text.value.trim();
  try {
    const unlisten = await listen<{ ttsId: string; kind: string; stage: string; message?: string }>('vcp-tts', event => {
      if (task !== current || event.payload.ttsId !== current.id) return;
      if (event.payload.kind === 'error' || event.payload.kind === 'stopped') {
        feedback.value = event.payload.kind === 'error' ? event.payload.message || '试听失败，请检查语音设置后重试。' : '';
        task = null;
        busy.value = false;
        current.unlisten?.();
      }
    });
    current.unlisten = unlisten;
    if (task !== current) { unlisten(); return; }
    if (!canStart.value) { await stop(); return; }
    current.submitted = true;
    await invoke('preview_tts', { ttsId: current.id, text: previewText, settings });
    // A return/mode exit can precede the start acknowledgement. Only this ID
    // may be stopped; never cancel another page's narration.
    if (task !== current) await invoke('stop_tts', { ttsId: current.id });
  } catch {
    if (task === current) {
      feedback.value = '试听未能开始，请检查连接、音色或参考音频后重试。';
      await stop();
    }
  }
};
watch(() => props.active, active => { if (!active) void stop(); }, { flush: 'sync' });
watch(() => voiceCall.active, active => { if (active) void stop(); }, { flush: 'sync' });
onBeforeUnmount(() => { void stop(); });
defineExpose({ stop, busy });
</script>

<template>
  <SettingsDisclosure title="试听声音" :description="unavailable" :disabled="disabled">
    <div class="flex flex-col gap-3">
      <VoiceTextArea v-model="text" label="试听文本" :rows="3" :max-length="500" :disabled="disabled" />
      <p class="m-0 text-xs leading-relaxed text-[var(--secondary-text)]">
        试听使用默认演绎指导，不调用聊天模型生成指导。
      </p>
      <button type="button" class="voice-preview-button" :disabled="!active || (!busy && !canStart)" @click="toggle">
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

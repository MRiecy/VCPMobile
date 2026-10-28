<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue';
import type { AgentVoiceConfigDto } from '../../core/types/assistant';
import type { VoiceConfig } from '../../core/config/generated/config';
import { useSettingsStore } from '../../core/stores/settings';
import SettingsSwitch from '../../components/settings/SettingsSwitch.vue';
import VoiceTtsPreview from '../chat/voice/VoiceTtsPreview.vue';
import VoiceLibraryControl from '../chat/voice/VoiceLibraryControl.vue';
import VoicePresetPicker from '../chat/voice/VoicePresetPicker.vue';
import { MIMO_TTS_MODES } from '../chat/voice/settings';

const props = defineProps<{
  modelValue: AgentVoiceConfigDto;
  active: boolean;
  disabled?: boolean;
}>();
const emit = defineEmits<{ 'update:modelValue': [value: AgentVoiceConfigDto] }>();
const settingsStore = useSettingsStore();
const preview = ref<InstanceType<typeof VoiceTtsPreview> | null>(null);
const library = ref<InstanceType<typeof VoiceLibraryControl> | null>(null);
const preset = ref<InstanceType<typeof VoicePresetPicker> | null>(null);
const connectionLoading = ref(false);
const connectionError = ref('');
let generation = 0;

const disabled = computed(() => !props.active || props.disabled);
const voiceDisabled = computed(() => disabled.value || !props.modelValue.enabled);
const currentModel = computed(() => MIMO_TTS_MODES.find(mode => mode.value === props.modelValue.mode)?.model);
const previewSettings = computed<VoiceConfig | null>(() => {
  const global = settingsStore.settings?.voice;
  if (!global) return null;
  const { mode, voice, cloneSampleId, stream } = props.modelValue;
  return { ...global, tts: { mode, voice, cloneSampleId, stream } };
});

const update = <K extends keyof AgentVoiceConfigDto>(key: K, value: AgentVoiceConfigDto[K]) => {
  if (disabled.value || (key !== 'enabled' && !props.modelValue.enabled)) return;
  emit('update:modelValue', { ...props.modelValue, [key]: value });
};
const loadConnection = async () => {
  const visit = ++generation;
  if (!props.active) return;
  connectionLoading.value = true;
  connectionError.value = '';
  try {
    await settingsStore.fetchSettings();
  } catch (error) {
    if (visit === generation && props.active) connectionError.value = `无法读取语音连接设置：${String(error)}`;
  } finally {
    if (visit === generation) connectionLoading.value = false;
  }
};
const stop = async () => {
  preset.value?.cancelPending();
  library.value?.cancelPending();
  await Promise.all([preview.value?.stop(), library.value?.stop(), preset.value?.stop()]);
};
watch(() => props.active, active => {
  if (active) void loadConnection();
  else {
    generation += 1;
    void stop().catch(console.warn);
  }
}, { immediate: true, flush: 'sync' });
watch(() => props.modelValue.enabled, enabled => {
  if (!enabled) void stop().catch(console.warn);
}, { flush: 'sync' });
watch(() => props.modelValue.mode, () => {
  void stop().catch(console.warn);
}, { flush: 'sync' });
watch(disabled, value => { if (value) preset.value?.cancelPending(); }, { flush: 'sync' });
onBeforeUnmount(() => {
  generation += 1;
  void stop().catch(console.warn);
});
defineExpose({ stop });
</script>

<template>
  <section class="space-y-1.5" aria-labelledby="agent-voice-settings-title">
    <div class="flex items-center gap-1.5 px-0.5">
      <span class="w-1.5 h-1.5 rounded-full bg-blue-500" aria-hidden="true"></span>
      <h3 id="agent-voice-settings-title" class="text-xs font-bold text-primary-text">语音设置</h3>
    </div>
    <div class="voice-plate">
      <div class="voice-row voice-enable-row">
        <div class="min-w-0">
          <span class="text-xs font-medium">启用独立音色</span>
          <p class="voice-enable-description">关闭则使用全局默认音色</p>
        </div>
        <SettingsSwitch :model-value="modelValue.enabled" :disabled="disabled" aria-label="启用独立音色"
          @update:model-value="update('enabled', $event)" />
      </div>
      <div class="voice-options" :class="{ 'voice-options--inactive': !modelValue.enabled }"
        :inert="!modelValue.enabled" :aria-disabled="!modelValue.enabled">
        <div class="voice-row voice-mode-row">
          <span class="text-xs">音色模式</span>
          <div class="voice-mode-switch" role="group" aria-label="音色模式">
            <button v-for="mode in MIMO_TTS_MODES" :key="mode.value" type="button" :disabled="voiceDisabled"
              class="voice-mode" :aria-pressed="modelValue.mode === mode.value"
              @click="update('mode', mode.value)">{{ mode.label }}</button>
          </div>
        </div>
        <div class="voice-row">
          <span class="text-xs">模型</span>
          <code class="voice-model-badge">{{ currentModel }}</code>
        </div>
        <VoicePresetPicker v-if="modelValue.mode === 'builtin'" ref="preset" compact
          :model-value="modelValue.voice" :active="active" :disabled="voiceDisabled || preview?.busy"
          @update:model-value="update('voice', $event)" />
        <VoiceLibraryControl v-else-if="modelValue.mode === 'clone'" ref="library" compact :model-value="modelValue.cloneSampleId"
          :active="active && modelValue.mode === 'clone'" :disabled="voiceDisabled || preview?.busy"
          @update:model-value="update('cloneSampleId', $event)" />
        <div v-if="modelValue.mode === 'builtin'" class="voice-row">
          <span class="text-xs">流式朗读</span>
          <SettingsSwitch :model-value="modelValue.stream" :disabled="voiceDisabled" aria-label="流式朗读"
            @update:model-value="update('stream', $event)" />
        </div>
        <div class="voice-preview-area">
          <p v-if="connectionLoading && !previewSettings" class="m-0 text-xs opacity-60" role="status">正在读取语音连接设置…</p>
          <p v-if="connectionError" class="m-0 text-xs text-red-500" role="alert">
            {{ connectionError }}
            <button type="button" :disabled="voiceDisabled || connectionLoading" class="min-h-11 px-2 underline" @click="loadConnection">重试</button>
          </p>
          <VoiceTtsPreview v-if="previewSettings" ref="preview" compact :settings="previewSettings"
            :active="active && modelValue.enabled" :locked="connectionLoading || library?.locked"
            :disabled="voiceDisabled || !!connectionError || library?.busy" />
        </div>
      </div>
    </div>
  </section>
</template>

<style scoped>
.voice-plate { border: 1px solid rgb(0 0 0 / 0.1); border-radius: 8px; overflow: hidden; background: rgb(0 0 0 / 0.03); }
:global(.dark) .voice-plate { border-color: rgb(255 255 255 / 0.1); background: rgb(255 255 255 / 0.035); }
.voice-row { display: flex; align-items: center; justify-content: space-between; gap: 12px; min-height: 44px; padding: 0 12px; border-bottom: 1px solid rgb(0 0 0 / 0.05); }
:global(.dark) .voice-row { border-bottom-color: rgb(255 255 255 / 0.05); }
.voice-row:last-child { border-bottom: 0; }
.voice-enable-row { min-height: 52px; }
.voice-enable-description { margin: 1px 0 0; color: var(--secondary-text); font-size: 11px; line-height: 16px; }
.voice-options--inactive { opacity: 0.3; }
.voice-options--inactive :deep(button:disabled), .voice-options--inactive :deep(input:disabled) { opacity: 1; }
.voice-mode-row { min-height: 56px; background: rgb(0 0 0 / 0.025); }
.voice-mode-switch { display: flex; gap: 2px; padding: 2px; border: 1px solid var(--border-color); border-radius: 5px; background: rgb(0 0 0 / 0.035); }
.voice-mode { position: relative; min-height: 30px; padding: 4px 10px; border: 0; border-radius: 3px; background: transparent; color: var(--secondary-text); font-size: 12px; touch-action: manipulation; }
.voice-mode::after { content: ''; position: absolute; inset: -7px 0; }
.voice-mode[aria-pressed="true"] { background: #2563eb; color: #fff; }
.voice-mode:active { opacity: 0.75; }
.voice-model-badge { max-width: 75%; padding: 4px 7px; border: 1px solid var(--border-color); border-radius: 4px; background: rgb(0 0 0 / 0.035); color: var(--secondary-text); font-size: 10px; font-family: var(--font-mono, monospace); overflow-wrap: anywhere; text-align: right; }
:global(.dark) .voice-mode-row { background: rgb(0 0 0 / 0.08); }
:global(.dark) .voice-mode-switch, :global(.dark) .voice-model-badge { background: rgb(255 255 255 / 0.025); }
.voice-preview-area { padding: 8px 12px; background: rgb(0 0 0 / 0.02); }
button:disabled { opacity: 0.4; }
</style>

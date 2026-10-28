<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue';
import type { AgentVoiceConfigDto } from '../../core/types/assistant';
import type { VoiceConfig } from '../../core/config/generated/config';
import type { ContextMenuConfig, OverlayActionItem } from '../../core/types/overlay';
import { useOverlayStore } from '../../core/stores/overlay';
import { useSettingsStore } from '../../core/stores/settings';
import SettingsSwitch from '../../components/settings/SettingsSwitch.vue';
import VoiceTtsPreview from '../chat/voice/VoiceTtsPreview.vue';
import VoiceLibraryControl from '../chat/voice/VoiceLibraryControl.vue';
import { MIMO_TTS_MODES, MIMO_VOICES } from '../chat/voice/settings';

const props = defineProps<{
  modelValue: AgentVoiceConfigDto;
  active: boolean;
  disabled?: boolean;
}>();
const emit = defineEmits<{ 'update:modelValue': [value: AgentVoiceConfigDto] }>();
const settingsStore = useSettingsStore();
const overlayStore = useOverlayStore();
const preview = ref<InstanceType<typeof VoiceTtsPreview> | null>(null);
const library = ref<InstanceType<typeof VoiceLibraryControl> | null>(null);
const connectionLoading = ref(false);
const connectionError = ref('');
let generation = 0;
let selectionMenu: ContextMenuConfig | null = null;

const disabled = computed(() => !props.active || props.disabled);
const currentVoice = computed(() => MIMO_VOICES.find(voice => voice.value === props.modelValue.voice)?.label ?? props.modelValue.voice);
const currentModel = computed(() => MIMO_TTS_MODES.find(mode => mode.value === props.modelValue.mode)?.model);
const previewSettings = computed<VoiceConfig | null>(() => {
  const global = settingsStore.settings?.voice;
  if (!global) return null;
  const { mode, voice, cloneSampleId, stream } = props.modelValue;
  return { ...global, tts: { mode, voice, cloneSampleId, stream } };
});

const update = <K extends keyof AgentVoiceConfigDto>(key: K, value: AgentVoiceConfigDto[K]) => {
  if (disabled.value) return;
  emit('update:modelValue', { ...props.modelValue, [key]: value });
};
const closeSelection = () => {
  if (selectionMenu && overlayStore.contextMenuConfig === selectionMenu) overlayStore.closeContextMenu();
  selectionMenu = null;
};
const selectPreset = (event: MouseEvent) => {
  if (disabled.value) return;
  (event.currentTarget as HTMLButtonElement).focus({ preventScroll: true });
  const actions: OverlayActionItem[] = MIMO_VOICES.map(voice => ({
    label: voice.label,
    selected: voice.value === props.modelValue.voice,
    handler: () => {
      if (disabled.value || overlayStore.contextMenuConfig?.actions !== actions) return;
      update('voice', voice.value);
    },
  }));
  overlayStore.openContextMenu(actions, '选择预置音色');
  selectionMenu = overlayStore.contextMenuConfig;
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
  closeSelection();
  library.value?.cancelPending();
  await Promise.all([preview.value?.stop(), library.value?.stop()]);
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
watch(() => props.modelValue.mode, mode => {
  closeSelection();
  if (mode === 'none') void stop().catch(console.warn);
}, { flush: 'sync' });
watch(disabled, value => { if (value) closeSelection(); }, { flush: 'sync' });
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
      <div class="voice-row">
        <div class="min-w-0">
          <span class="text-xs font-medium">启用独立音色</span>
          <p class="m-0 mt-0.5 text-[11px] opacity-60">关闭则使用全局默认音色</p>
        </div>
        <SettingsSwitch :model-value="modelValue.enabled" :disabled="disabled" aria-label="启用独立音色"
          @update:model-value="update('enabled', $event)" />
      </div>
      <template v-if="modelValue.enabled">
        <div class="voice-row">
          <span class="text-xs opacity-60">音色模式</span>
          <div class="flex items-center gap-1" role="group" aria-label="音色模式">
            <button v-for="mode in MIMO_TTS_MODES" :key="mode.value" type="button" :disabled="disabled"
              class="voice-mode" :aria-pressed="modelValue.mode === mode.value"
              @click="update('mode', mode.value)">{{ mode.label }}</button>
          </div>
        </div>
        <div v-if="currentModel" class="voice-row">
          <span class="text-xs opacity-60">模型</span>
          <code class="text-[11px] font-mono break-all text-right">{{ currentModel }}</code>
        </div>
        <button v-if="modelValue.mode === 'builtin'" type="button" class="voice-row voice-selection"
          :disabled="disabled" aria-haspopup="dialog" @click="selectPreset">
          <span>当前音色</span>
          <span class="min-w-0 text-right text-blue-500">{{ currentVoice }} <span aria-hidden="true" class="opacity-60 ml-1">›</span></span>
        </button>
        <VoiceLibraryControl v-else-if="modelValue.mode === 'clone'" ref="library" :model-value="modelValue.cloneSampleId"
          :active="active && modelValue.mode === 'clone'" :disabled="disabled || preview?.busy"
          @update:model-value="update('cloneSampleId', $event)" />
        <div v-if="modelValue.mode === 'builtin'" class="voice-row">
          <span class="text-xs">流式朗读</span>
          <SettingsSwitch :model-value="modelValue.stream" :disabled="disabled" aria-label="流式朗读"
            @update:model-value="update('stream', $event)" />
        </div>
        <p v-if="modelValue.mode === 'none'" class="m-0 p-3 text-xs opacity-60">当前助手不使用语音朗读。</p>
        <div v-else class="p-3">
          <p v-if="connectionLoading" class="m-0 text-xs opacity-60" role="status">正在读取语音连接设置…</p>
          <p v-if="connectionError" class="m-0 text-xs text-red-500" role="alert">
            {{ connectionError }}
            <button type="button" :disabled="disabled || connectionLoading" class="min-h-11 px-2 underline" @click="loadConnection">重试</button>
          </p>
          <VoiceTtsPreview v-if="previewSettings" ref="preview" :settings="previewSettings"
            :active="active && modelValue.enabled" :disabled="disabled || connectionLoading || !!connectionError || library?.busy" />
        </div>
      </template>
    </div>
  </section>
</template>

<style scoped>
.voice-plate { border: 1px solid rgb(0 0 0 / 0.1); border-radius: 8px; overflow: hidden; background: rgb(0 0 0 / 0.03); }
:global(.dark) .voice-plate { border-color: rgb(255 255 255 / 0.1); background: rgb(255 255 255 / 0.035); }
.voice-row { display: flex; align-items: center; justify-content: space-between; gap: 12px; min-height: 44px; padding: 4px 12px; border-bottom: 1px solid rgb(0 0 0 / 0.05); }
:global(.dark) .voice-row { border-bottom-color: rgb(255 255 255 / 0.05); }
.voice-row:last-child { border-bottom: 0; }
.voice-mode { min-height: 44px; padding: 4px 10px; border: 0; border-bottom: 2px solid transparent; background: transparent; color: inherit; font-size: 12px; opacity: 0.6; }
.voice-mode[aria-pressed="true"] { border-bottom-color: var(--highlight-text); color: var(--highlight-text); opacity: 1; }
.voice-selection { width: 100%; background: transparent; color: inherit; border-top: 0; border-left: 0; border-right: 0; font-size: 12px; text-align: left; }
.voice-selection:active, .voice-mode:active { background: rgb(0 0 0 / 0.04); }
button:disabled { opacity: 0.4; }
</style>

<script setup lang="ts">
import { computed, onBeforeUnmount, ref, useId, watch } from 'vue';
import { ListChecks } from '@lucide/vue';
import { useOverlayStore } from '../../../core/stores/overlay';
import { useSettingsStore } from '../../../core/stores/settings';
import type { ContextMenuConfig, OverlayActionItem, PromptConfig } from '../../../core/types/overlay';
import SettingsSection from '../../../components/settings/SettingsSection.vue';
import SettingsCanvas from '../../../components/settings/SettingsCanvas.vue';
import SettingsDisclosure from '../../../components/settings/SettingsDisclosure.vue';
import SettingsTextField from '../../../components/settings/SettingsTextField.vue';
import SettingsRow from '../../../components/settings/SettingsRow.vue';
import SettingsSwitch from '../../../components/settings/SettingsSwitch.vue';
import type { SettingsPageProps } from '../../settings/contracts';
import { VOICE_FIELDS } from '../../settings/editing/fieldSets';
import { useSettingsPageSession } from '../../settings/editing/useSettingsPageSession';
import { SETTINGS_DEFAULTS, MIMO_TTS_MODES, MIMO_VOICES, settingDescriptor, settingOptions } from './settings';
import type { VoiceConfig } from '../../../core/config/generated/config';
import { applySettingsFields, type SettingsFieldValues } from '../../../core/config/fields';
import VoiceTextArea from './VoiceTextArea.vue';
import VoiceCloneSample from './VoiceCloneSample.vue';
import VoiceTtsPreview from './VoiceTtsPreview.vue';

const props = defineProps<SettingsPageProps>();
const providerTitleId = useId();
const overlayStore = useOverlayStore();
const settingsStore = useSettingsStore();
const { session, loading, error, reload } = useSettingsPageSession(VOICE_FIELDS, () => props.active);
const { draft, saving, leaving } = session;
const openingStyleGuidance = ref(false);
const preview = ref<InstanceType<typeof VoiceTtsPreview> | null>(null);
const cloneSample = ref<InstanceType<typeof VoiceCloneSample> | null>(null);
const previewSettings = computed<VoiceConfig>(() =>
  applySettingsFields(settingsStore.settings ?? SETTINGS_DEFAULTS, draft.value ?? {}).voice,
);
const ttsModel = computed(() => MIMO_TTS_MODES.find(mode => mode.value === draft.value?.['voice.tts.mode'])?.model ?? MIMO_TTS_MODES[0].model);
const disabled = computed(() => !props.active || leaving.value || openingStyleGuidance.value);
type DurationField = 'voice.vad.endSilenceMs' | 'voice.vad.playbackOnsetMs';
type SelectionField = 'voice.asr.language' | 'voice.asr.inputEncoding' | 'voice.tts.voice' | 'voice.tts.mode' | 'voice.vad.sensitivity' | DurationField;
const durationSettings = {
  'voice.vad.endSilenceMs': { min: settingDescriptor('voice.vad.endSilenceMs').min!, max: settingDescriptor('voice.vad.endSilenceMs').max!, presets: [320, 512, 800, 1200, 1600] },
  'voice.vad.playbackOnsetMs': { min: settingDescriptor('voice.vad.playbackOnsetMs').min!, max: settingDescriptor('voice.vad.playbackOnsetMs').max!, presets: [128, 192, 320] },
} as const;
const durationOptions = (field: DurationField) => durationSettings[field].presets.map(value => ({ value, label: `${value} ms` }));
const selections = {
  'voice.asr.language': {
    title: '识别语言',
    options: settingOptions('voice.asr.language', { auto: '自动识别', zh: '中文', en: '英文' }),
  },
  'voice.asr.inputEncoding': {
    title: '语音输入编码',
    options: settingOptions('voice.asr.inputEncoding', { wav: 'WAV 无损', mp3_64: 'MP3 64 kbps' }),
  },
  'voice.tts.voice': { title: '朗读音色', options: MIMO_VOICES },
  'voice.tts.mode': { title: '音色模式', options: MIMO_TTS_MODES },
  'voice.vad.sensitivity': {
    title: '人声检测灵敏度',
    options: settingOptions('voice.vad.sensitivity', { low: '较低', standard: '标准', high: '较高' }),
  },
  'voice.vad.endSilenceMs': { title: '说完等待时间', options: durationOptions('voice.vad.endSilenceMs') },
  'voice.vad.playbackOnsetMs': { title: '朗读时插话确认', options: durationOptions('voice.vad.playbackOnsetMs') },
} as const satisfies {
  [K in SelectionField]: { title: string; options: ReadonlyArray<{ value: SettingsFieldValues[K]; label: string }> };
};
const recognitionFields = ['voice.asr.language', 'voice.asr.inputEncoding'] as const;
const advancedFields = [
  { field: 'voice.vad.sensitivity', description: '较高更容易接纳轻声，也更容易误触。' },
  { field: 'voice.vad.endSilenceMs', description: '停顿达到此时长后结束当前录音。' },
  { field: 'voice.vad.playbackOnsetMs', description: '朗读期间持续检测到人声多久后确认插话。' },
] as const;
const isDurationField = (field: SelectionField): field is DurationField => field in durationSettings;
let selectionMenu: ContextMenuConfig | null = null;
let durationPrompt: PromptConfig | null = null;

const selectedLabel = (field: SelectionField) =>
  isDurationField(field) ? `${draft.value?.[field]} ms`
    : selections[field].options.find(option => option.value === draft.value?.[field])?.label ?? '';
const selectionOpen = (field: SelectionField) =>
  !!selectionMenu && overlayStore.contextMenuConfig === selectionMenu && selectionMenu.title === selections[field].title;
const closeSelection = () => {
  // A later menu belongs to its own caller, even if this settings visit ends.
  if (selectionMenu && overlayStore.contextMenuConfig === selectionMenu) overlayStore.closeContextMenu();
  selectionMenu = null;
};
const closeDurationPrompt = () => {
  if (durationPrompt && overlayStore.promptConfig === durationPrompt) overlayStore.closePrompt();
  durationPrompt = null;
};
const openDurationPrompt = (field: DurationField) => {
  if (disabled.value || !draft.value) return;
  const { min, max } = durationSettings[field];
  const validate = (raw: string) => {
    const value = raw.trim();
    return /^\d+$/.test(value) && Number.isSafeInteger(Number(value)) && Number(value) >= min && Number(value) <= max
      ? null : `请输入 ${min}–${max} ms 之间的整数`;
  };
  const config: PromptConfig = {
    title: `自定义${selections[field].title}（ms）`,
    initialValue: String(draft.value[field]),
    placeholder: `${min}–${max} ms`,
    inputMode: 'numeric',
    validate,
    onConfirm: value => {
      if (disabled.value || overlayStore.promptConfig !== durationPrompt
        || durationPrompt?.onConfirm !== config.onConfirm || validate(value)) return;
      session.edit(field, Number(value.trim()));
    },
  };
  overlayStore.openPrompt(config);
  durationPrompt = overlayStore.promptConfig;
};
const openSelection = (field: SelectionField, event: Event) => {
  if (disabled.value || !draft.value) return;
  (event.currentTarget as HTMLButtonElement).focus({ preventScroll: true });
  const actions: OverlayActionItem[] = selections[field].options.map(option => ({
    label: option.label,
    selected: draft.value?.[field] === option.value,
    handler: () => {
      if (disabled.value || overlayStore.contextMenuConfig?.actions !== actions) return;
      session.edit(field, option.value);
    },
  }));
  if (isDurationField(field)) {
    actions.push({
      label: '自定义…',
      selected: !selections[field].options.some(option => option.value === draft.value?.[field]),
      handler: () => {
        if (disabled.value || overlayStore.contextMenuConfig?.actions !== actions) return;
        openDurationPrompt(field);
      },
    });
  }
  overlayStore.openContextMenu(actions, selections[field].title);
  selectionMenu = overlayStore.contextMenuConfig;
};
watch(disabled, value => {
  if (value) {
    closeSelection();
    closeDurationPrompt();
  }
}, { flush: 'sync' });
onBeforeUnmount(() => {
  closeSelection();
  closeDurationPrompt();
});

const openStyleGuidance = async () => {
  if (disabled.value) return;
  openingStyleGuidance.value = true;
  try {
    cloneSample.value?.cancelPending();
    await preview.value?.stop();
    // A covering page ends this visit in the voice overlay. Commit the visible
    // draft before navigating, just like leaving the global settings page.
    if (draft.value && !(await session.save()).ok) return;
    if (props.active) overlayStore.openVoiceStyleGuidance();
  } finally {
    openingStyleGuidance.value = false;
  }
};

const beforeLeave = async () => {
  if (openingStyleGuidance.value) return false;
  cloneSample.value?.cancelPending();
  await preview.value?.stop();
  return session.prepareLeave();
};

defineExpose({ beforeLeave });
</script>

<template>
  <SettingsCanvas viewport :inert="!active">
        <p v-if="error" role="alert" class="m-0 text-sm text-red-500">
          {{ error }}
          <button v-if="!draft" type="button" class="ml-2 underline" :disabled="loading || !active" @click="reload">重试</button>
        </p>
        <p v-else-if="!draft && loading" class="m-0 text-sm opacity-60" role="status">正在加载设置…</p>
        <p v-if="saving || leaving" class="m-0 text-xs text-[var(--secondary-text)]" role="status">正在保存语音设置…</p>

        <section v-if="draft" class="voice-provider-section" :aria-labelledby="providerTitleId">
          <h2 :id="providerTitleId" class="settings-canvas-title">MiMo 语音服务</h2>
          <SettingsSection title="服务器连接">
            <div class="voice-connection-fields">
              <SettingsTextField :model-value="draft['voice.connection.apiBaseUrl']" :disabled="disabled"
                label="API 地址" :placeholder="settingDescriptor('voice.connection.apiBaseUrl').preset" mono
                @update:model-value="session.edit('voice.connection.apiBaseUrl', $event)" />
              <SettingsTextField :model-value="draft['voice.connection.apiKey']" :disabled="disabled"
                label="语音 API Key" placeholder="输入 MiMo API Key" is-secure
                @update:model-value="session.edit('voice.connection.apiKey', $event)" />
            </div>
            <template #footer>
              <p class="m-0">地址留空使用 MiMo 官方服务。</p>
            </template>
          </SettingsSection>

          <SettingsSection title="语音识别 ASR">
            <template #description>模型：<code class="font-mono">mimo-v2.5-asr</code></template>
            <div>
              <SettingsRow v-for="field in recognitionFields" :key="field" :title="selections[field].title"
                :description="field === 'voice.asr.inputEncoding' ? 'MP3 约节省 75% 上传流量' : undefined"
                :value="selectedLabel(field)" clickable :disabled="disabled"
                :aria-label="selections[field].title" aria-haspopup="dialog" :aria-expanded="selectionOpen(field)"
                @click="openSelection(field, $event)" />
              <SettingsRow title="流式识别结果" description="说完后整段上传，并流式返回转写文本。">
                <template #action>
                  <SettingsSwitch :model-value="draft['voice.asr.stream']" :disabled="disabled" aria-label="流式识别结果"
                    @update:model-value="session.edit('voice.asr.stream', $event)" />
                </template>
              </SettingsRow>
            </div>
          </SettingsSection>

          <SettingsSection title="语音朗读 TTS">
            <template #description>模型：<code class="font-mono">{{ ttsModel }}</code></template>
            <div>
              <SettingsRow title="音色模式" :value="selectedLabel('voice.tts.mode')" clickable
                :disabled="disabled" aria-label="音色模式" aria-haspopup="dialog"
                :aria-expanded="selectionOpen('voice.tts.mode')" @click="openSelection('voice.tts.mode', $event)" />
              <SettingsRow v-if="draft['voice.tts.mode'] === 'builtin'" title="朗读音色" :value="selectedLabel('voice.tts.voice')" clickable
                :disabled="disabled" aria-label="朗读音色" aria-haspopup="dialog"
                :aria-expanded="selectionOpen('voice.tts.voice')" @click="openSelection('voice.tts.voice', $event)" />
              <SettingsRow v-if="draft['voice.tts.mode'] === 'builtin'" title="流式朗读" description="收到音频片段后开始播放；关闭则等待完整合成。">
                <template #action>
                  <SettingsSwitch :model-value="draft['voice.tts.stream']" :disabled="disabled" aria-label="流式朗读"
                    @update:model-value="session.edit('voice.tts.stream', $event)" />
                </template>
              </SettingsRow>
            </div>
            <VoiceCloneSample v-show="draft['voice.tts.mode'] === 'clone'" ref="cloneSample"
              :model-value="draft['voice.tts.cloneSampleId']" :active="active" :disabled="disabled || preview?.busy"
              :settle-before-discard="session.settlePending"
              @update:model-value="session.edit('voice.tts.cloneSampleId', $event)" />
            <SettingsDisclosure title="默认演绎指导" :disabled="disabled"
              :description="draft['voice.tts.stylePrompt'].trim() ? '已填写 · 自动指导关闭或缺失时使用' : '可选 · 自动指导关闭或缺失时使用'">
              <VoiceTextArea label="默认演绎指导" :disabled="disabled" :max-length="settingDescriptor('voice.tts.stylePrompt').maxChars"
                :model-value="draft['voice.tts.stylePrompt']" placeholder="例如：语气轻松自然，语速稍慢。"
                description="保留所选音色，控制情绪、语气与节奏；留空则自然朗读。"
                @update:model-value="session.edit('voice.tts.stylePrompt', $event)" />
            </SettingsDisclosure>
            <p v-if="draft['voice.tts.mode'] !== 'builtin'" class="m-0 text-xs leading-relaxed text-[var(--secondary-text)]">
              每个文本分段合成完成后返回音频；模型回复仍会逐段提交朗读。
            </p>
            <VoiceTtsPreview ref="preview" :settings="previewSettings" :active="active" :disabled="disabled || cloneSample?.importing" />
            <template #footer>返回时自动保存，下一次朗读生效。</template>
          </SettingsSection>
        </section>

        <SettingsRow title="风格指导" description="LLM 音频标签与自然语言指导规则" :icon="ListChecks" clickable
          :disabled="disabled" class="voice-style-guidance-entry" @click="openStyleGuidance" />

        <SettingsDisclosure v-if="draft" title="高级参数" description="本地人声检测与插话" :disabled="disabled">
          <div>
            <SettingsRow v-for="item in advancedFields" :key="item.field" :title="selections[item.field].title"
              :description="item.description" :value="selectedLabel(item.field)" clickable :disabled="disabled"
              :aria-label="selections[item.field].title" aria-haspopup="dialog" :aria-expanded="selectionOpen(item.field)"
              @click="openSelection(item.field, $event)" />
          </div>
          <p class="m-0 text-xs leading-relaxed text-[var(--secondary-text)]">返回时自动保存，下次通话生效。</p>
        </SettingsDisclosure>

  </SettingsCanvas>
</template>

<style scoped>
.voice-provider-section { display: flex; flex-direction: column; gap: 20px; }

.voice-connection-fields {
  display: grid;
  grid-template-columns: minmax(0, 1fr);
  gap: 14px;
}

@media (min-width: 768px) {
  .voice-connection-fields { grid-template-columns: repeat(2, minmax(0, 1fr)); }
}
</style>

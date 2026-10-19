<script setup lang="ts">
import { computed, onBeforeUnmount, ref, useId, watch } from 'vue';
import { ListChecks, Play, Square } from 'lucide-vue-next';
import { openUrl } from '@tauri-apps/plugin-opener';
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
import { useVoiceCallStore } from './voiceCallStore';
import { DEFAULT_VOICE_SETTINGS, MIMO_TTS_MODES, MIMO_VOICES, type VoiceServiceSettings } from './settings';
import VoiceTextArea from './VoiceTextArea.vue';
import VoiceCloneSample from './VoiceCloneSample.vue';
import VoiceTtsPreview from './VoiceTtsPreview.vue';

const props = defineProps<SettingsPageProps>();
const providerTitleId = useId();
const overlayStore = useOverlayStore();
const voiceCall = useVoiceCallStore();
const settingsStore = useSettingsStore();
const { session, loading, error, reload } = useSettingsPageSession(VOICE_FIELDS, () => props.active);
const { draft, saving, leaving } = session;
const openingPresets = ref(false);
const preview = ref<InstanceType<typeof VoiceTtsPreview> | null>(null);
const cloneSample = ref<InstanceType<typeof VoiceCloneSample> | null>(null);
const validationError = ref('');
const previewSettings = computed<VoiceServiceSettings>(() => {
  const source = { ...DEFAULT_VOICE_SETTINGS, ...settingsStore.settings, ...draft.value };
  return Object.fromEntries(Object.keys(DEFAULT_VOICE_SETTINGS).map(key => [key, source[key as keyof VoiceServiceSettings]])) as unknown as VoiceServiceSettings;
});
const ttsModel = computed(() => MIMO_TTS_MODES.find(mode => mode.value === draft.value?.voiceTtsMode)?.model ?? MIMO_TTS_MODES[0].model);
const disabled = computed(() => !props.active || leaving.value || openingPresets.value);
type DurationField = 'voiceVadEndSilenceMs' | 'voiceVadPlaybackOnsetMs';
type SelectionField = 'voiceLanguage' | 'voiceInputEncoding' | 'voiceTtsVoice' | 'voiceTtsMode' | 'voiceVadSensitivity' | DurationField;
const durationSettings = {
  voiceVadEndSilenceMs: { min: 32, max: 5000, presets: [320, 512, 800, 1200, 1600] },
  voiceVadPlaybackOnsetMs: { min: 32, max: 2000, presets: [128, 192, 320] },
} as const;
const durationOptions = (field: DurationField) => durationSettings[field].presets.map(value => ({ value, label: `${value} ms` }));
const selections = {
  voiceLanguage: {
    title: '识别语言',
    options: [{ value: 'auto', label: '自动识别' }, { value: 'zh', label: '中文' }, { value: 'en', label: '英文' }],
  },
  voiceInputEncoding: {
    title: '语音输入编码',
    options: [{ value: 'wav', label: 'WAV 无损' }, { value: 'mp3_64', label: 'MP3 64 kbps' }],
  },
  voiceTtsVoice: { title: '朗读音色', options: MIMO_VOICES },
  voiceTtsMode: { title: '音色模式', options: MIMO_TTS_MODES },
  voiceVadSensitivity: {
    title: '人声检测灵敏度',
    options: [{ value: 'low', label: '较低' }, { value: 'standard', label: '标准' }, { value: 'high', label: '较高' }],
  },
  voiceVadEndSilenceMs: { title: '说完等待时间', options: durationOptions('voiceVadEndSilenceMs') },
  voiceVadPlaybackOnsetMs: { title: '朗读时插话确认', options: durationOptions('voiceVadPlaybackOnsetMs') },
} as const satisfies {
  [K in SelectionField]: { title: string; options: ReadonlyArray<{ value: VoiceServiceSettings[K]; label: string }> };
};
const recognitionFields = ['voiceLanguage', 'voiceInputEncoding'] as const;
const advancedFields = [
  { field: 'voiceVadSensitivity', description: '较高更容易接纳轻声，也更容易误触。' },
  { field: 'voiceVadEndSilenceMs', description: '停顿达到此时长后结束当前录音。' },
  { field: 'voiceVadPlaybackOnsetMs', description: '朗读期间持续检测到人声多久后确认插话。' },
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
const canProbe = computed(() => props.active && voiceCall.active && voiceCall.microphoneState === 'on');
const probeBusy = computed(() => voiceCall.probeActive || voiceCall.probePending);
const aecLabel = computed(() => {
  switch (voiceCall.aecState) {
    case 'enabled': return '系统回声消除已启用（效果待验证）';
    case 'unavailable': return '当前设备或音频路线未提供系统回声消除';
    case 'failed': return '系统回声消除启用失败';
    default: return '开启麦克风后显示回声消除状态';
  }
});

watch(() => props.active, (active, wasActive) => {
  if (!active && wasActive) void voiceCall.setProbe(false);
}, { flush: 'sync' });
onBeforeUnmount(() => {
  closeSelection();
  closeDurationPrompt();
  if (props.active) void voiceCall.setProbe(false);
});

const openPresets = async () => {
  if (disabled.value) return;
  if (!validateDraft()) return;
  openingPresets.value = true;
  try {
    cloneSample.value?.cancelPending();
    await preview.value?.stop();
    // A covering page ends this visit in the voice overlay. Commit the visible
    // draft before navigating, just like leaving the global settings page.
    if (draft.value && !(await session.save()).ok) return;
    if (props.active) overlayStore.openVoicePresets();
  } finally {
    openingPresets.value = false;
  }
};

const validateDraft = () => {
  validationError.value = '';
  if (draft.value?.voiceTtsMode === 'design' && !draft.value.voiceTtsDesignPrompt.trim()) {
    validationError.value = '请填写音色描述，用于自动指导关闭或缺失时的朗读。';
    return false;
  }
  return true;
};

const beforeLeave = async () => {
  if (openingPresets.value || !validateDraft()) return false;
  cloneSample.value?.cancelPending();
  await preview.value?.stop();
  return session.prepareLeave();
};

const openSource = (url: string) => {
  if (props.active) void openUrl(url).catch(error => console.warn('[VoiceAudio] Cannot open attribution:', error));
};

defineExpose({ beforeLeave });
</script>

<template>
  <SettingsCanvas viewport :inert="!active">
        <p v-if="error" role="alert" class="m-0 text-sm text-red-500">
          {{ draft ? '语音设置保存失败，修改已保留，请再次返回重试。' : '语音设置加载失败，请重试。' }}
          <button v-if="!draft" type="button" class="ml-2 underline" :disabled="loading || !active" @click="reload">重试</button>
        </p>
        <p v-else-if="!draft && loading" class="m-0 text-sm opacity-60" role="status">正在加载设置…</p>
        <p v-if="saving || leaving" class="m-0 text-xs text-[var(--secondary-text)]" role="status">正在保存语音设置…</p>
        <p v-if="validationError" class="m-0 text-sm text-red-500" role="alert">{{ validationError }}</p>

        <section v-if="draft" class="voice-provider-section" :aria-labelledby="providerTitleId">
          <h2 :id="providerTitleId" class="settings-canvas-title">MiMo 语音服务</h2>
          <SettingsSection title="服务器连接">
            <div class="voice-connection-fields">
              <SettingsTextField :model-value="draft.voiceApiBaseUrl" :disabled="disabled"
                label="API 地址" :placeholder="DEFAULT_VOICE_SETTINGS.voiceApiBaseUrl" mono
                @update:model-value="session.edit('voiceApiBaseUrl', $event)" />
              <SettingsTextField :model-value="draft.voiceApiKey" :disabled="disabled"
                label="语音 API Key" placeholder="输入 MiMo API Key" is-secure
                @update:model-value="session.edit('voiceApiKey', $event)" />
            </div>
            <template #footer>
              <p class="m-0">
                {{ draft.voiceApiKey.trim() ? '返回时自动保存，重新开启麦克风或开始新的朗读时生效。' : '填写 API Key 后返回会自动保存；配置在下次开麦或新朗读时生效。' }}
              </p>
            </template>
          </SettingsSection>

          <SettingsSection title="语音识别 ASR">
            <template #description>模型：<code class="font-mono">mimo-v2.5-asr</code></template>
            <div>
              <SettingsRow v-for="field in recognitionFields" :key="field" :title="selections[field].title"
                :description="field === 'voiceInputEncoding' ? 'MP3 约节省 75% 上传流量' : undefined"
                :value="selectedLabel(field)" clickable :disabled="disabled"
                :aria-label="selections[field].title" aria-haspopup="dialog" :aria-expanded="selectionOpen(field)"
                @click="openSelection(field, $event)" />
              <SettingsRow title="流式识别结果" description="说完后整段上传，再逐步返回转写文本。">
                <template #action>
                  <SettingsSwitch :model-value="draft.voiceAsrStream" :disabled="disabled" aria-label="流式识别结果"
                    @update:model-value="session.edit('voiceAsrStream', $event)" />
                </template>
              </SettingsRow>
            </div>
          </SettingsSection>

          <SettingsSection title="语音朗读 TTS">
            <template #description>模型：<code class="font-mono">{{ ttsModel }}</code></template>
            <div>
              <SettingsRow title="音色模式" :value="selectedLabel('voiceTtsMode')" clickable
                :disabled="disabled" aria-label="音色模式" aria-haspopup="dialog"
                :aria-expanded="selectionOpen('voiceTtsMode')" @click="openSelection('voiceTtsMode', $event)" />
              <SettingsRow v-if="draft.voiceTtsMode === 'builtin'" title="朗读音色" :value="selectedLabel('voiceTtsVoice')" clickable
                :disabled="disabled" aria-label="朗读音色" aria-haspopup="dialog"
                :aria-expanded="selectionOpen('voiceTtsVoice')" @click="openSelection('voiceTtsVoice', $event)" />
              <SettingsRow v-if="draft.voiceTtsMode === 'builtin'" title="流式朗读" description="收到音频片段后开始播放；关闭则等待完整合成。">
                <template #action>
                  <SettingsSwitch :model-value="draft.voiceTtsStream" :disabled="disabled" aria-label="流式朗读"
                    @update:model-value="session.edit('voiceTtsStream', $event)" />
                </template>
              </SettingsRow>
            </div>
            <VoiceCloneSample v-show="draft.voiceTtsMode === 'clone'" ref="cloneSample"
              :model-value="draft.voiceTtsCloneSampleId" :active="active" :disabled="disabled || preview?.busy"
              :settle-before-discard="session.settlePending"
              @update:model-value="session.edit('voiceTtsCloneSampleId', $event)" />
            <VoiceTextArea v-if="draft.voiceTtsMode === 'design'" label="音色描述" :disabled="disabled"
              :model-value="draft.voiceTtsDesignPrompt" :max-length="2000"
              description="自动指导关闭或缺失时使用。描述声线、语气与节奏。"
              @update:model-value="session.edit('voiceTtsDesignPrompt', $event)" />
            <SettingsDisclosure v-else title="演绎指导" :disabled="disabled"
              :description="draft.voiceTtsStylePrompt.trim() ? '已填写，自然语言控制语气与节奏' : '可选 · 语气、节奏与发音要求'">
              <VoiceTextArea label="自然语言演绎指导" :disabled="disabled" :max-length="2000"
                :model-value="draft.voiceTtsStylePrompt" placeholder="例如：语气轻松自然，语速稍慢。"
                @update:model-value="session.edit('voiceTtsStylePrompt', $event)" />
            </SettingsDisclosure>
            <p v-if="draft.voiceTtsMode !== 'builtin'" class="m-0 text-xs leading-relaxed text-[var(--secondary-text)]">
              每个文本分段合成完成后返回音频；模型回复仍会逐段提交朗读。
            </p>
            <VoiceTtsPreview ref="preview" :settings="previewSettings" :active="active" :disabled="disabled || cloneSample?.importing" />
            <template #footer>返回时自动保存，下一次朗读生效。</template>
          </SettingsSection>
        </section>

        <SettingsRow title="语音预设" description="文字设计模式 · LLM 标签与指导规则" :icon="ListChecks" clickable
          :disabled="disabled" class="voice-preset-entry" @click="openPresets" />

        <SettingsDisclosure v-if="draft" title="高级参数" description="本地人声检测与插话" :disabled="disabled">
          <div>
            <SettingsRow v-for="item in advancedFields" :key="item.field" :title="selections[item.field].title"
              :description="item.description" :value="selectedLabel(item.field)" clickable :disabled="disabled"
              :aria-label="selections[item.field].title" aria-haspopup="dialog" :aria-expanded="selectionOpen(item.field)"
              @click="openSelection(item.field, $event)" />
          </div>
          <p class="m-0 text-xs leading-relaxed text-[var(--secondary-text)]">返回时自动保存，下次通话生效。</p>
        </SettingsDisclosure>

        <SettingsDisclosure title="本地诊断" :disabled="disabled"
          :description="voiceCall.probeError || (probeBusy ? '本地样本播放中' : '回声消除与打断检查')">
          <div class="flex flex-col gap-3">
            <p class="m-0 text-xs leading-relaxed text-[var(--secondary-text)]">{{ aecLabel }}</p>
            <p class="m-0 text-sm leading-relaxed opacity-70">
              播放本地数字语音；播放时开口可检查误触与打断。样本长约 6.5 秒，播放一次后停止，不经过网络。
            </p>
            <div v-if="canProbe" class="flex flex-col gap-1 text-xs" role="status" aria-live="polite">
              <span>{{ voiceCall.speechActive ? '检测到说话' : '正在聆听' }}</span>
              <span v-if="voiceCall.route" class="opacity-60">音频路线：{{ voiceCall.route }}</span>
            </div>
            <p v-else class="m-0 text-xs leading-relaxed opacity-60">请先进入语音模式并开启麦克风。</p>
            <button type="button" class="voice-probe-button" :disabled="!canProbe"
              @click="voiceCall.setProbe(!probeBusy)">
              <Square v-if="probeBusy" :size="16" aria-hidden="true" />
              <Play v-else :size="16" aria-hidden="true" />
              {{ probeBusy ? '停止样本' : '播放本地样本' }}
            </button>
            <p v-if="voiceCall.probeError" class="m-0 text-xs leading-relaxed text-red-500" role="alert">{{ voiceCall.probeError }}</p>
            <p class="m-0 text-xs leading-relaxed opacity-60">
              样本来源：<a href="https://github.com/Jakobovski/free-spoken-digit-dataset" class="underline"
                @click.prevent="openSource('https://github.com/Jakobovski/free-spoken-digit-dataset')">FSDD</a>
              · <a href="https://creativecommons.org/licenses/by-sa/4.0/" class="underline"
                @click.prevent="openSource('https://creativecommons.org/licenses/by-sa/4.0/')">CC BY-SA 4.0</a>
            </p>
          </div>
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

.voice-probe-button {
  display: inline-flex;
  align-items: center;
  align-self: flex-start;
  justify-content: center;
  gap: 8px;
  min-height: 44px;
  padding: 8px 12px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: transparent;
  color: inherit;
  font-size: 13px;
  cursor: pointer;
  touch-action: manipulation;
}

.voice-probe-button:disabled { opacity: 0.4; cursor: default; }

@media (min-width: 768px) {
  .voice-connection-fields { grid-template-columns: repeat(2, minmax(0, 1fr)); }
}
</style>

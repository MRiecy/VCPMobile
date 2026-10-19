<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue';
import { ListChecks, Play, Square } from 'lucide-vue-next';
import { openUrl } from '@tauri-apps/plugin-opener';
import { useOverlayStore } from '../../../core/stores/overlay';
import type { ContextMenuConfig, OverlayActionItem } from '../../../core/types/overlay';
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
import { DEFAULT_VOICE_SETTINGS, MIMO_VOICES, type VoiceServiceSettings } from './settings';

const props = defineProps<SettingsPageProps>();
const overlayStore = useOverlayStore();
const voiceCall = useVoiceCallStore();
const { session, loading, error, reload } = useSettingsPageSession(VOICE_FIELDS, () => props.active);
const { draft, dirty, saving, leaving } = session;
const openingPresets = ref(false);
const disabled = computed(() => !props.active || leaving.value || openingPresets.value);
const save = () => { if (!disabled.value) void session.save(); };
type SelectionField = 'voiceLanguage' | 'voiceInputEncoding' | 'voiceTtsVoice';
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
} as const satisfies {
  [K in SelectionField]: { title: string; options: ReadonlyArray<{ value: VoiceServiceSettings[K]; label: string }> };
};
const recognitionFields = ['voiceLanguage', 'voiceInputEncoding'] as const;
let selectionMenu: ContextMenuConfig | null = null;

const selectedLabel = (field: SelectionField) =>
  selections[field].options.find(option => option.value === draft.value?.[field])?.label ?? '';
const selectionOpen = (field: SelectionField) =>
  !!selectionMenu && overlayStore.contextMenuConfig === selectionMenu && selectionMenu.title === selections[field].title;
const closeSelection = () => {
  // A later menu belongs to its own caller, even if this settings visit ends.
  if (selectionMenu && overlayStore.contextMenuConfig === selectionMenu) overlayStore.closeContextMenu();
  selectionMenu = null;
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
  overlayStore.openContextMenu(actions, selections[field].title);
  selectionMenu = overlayStore.contextMenuConfig;
};
watch(disabled, value => { if (value) closeSelection(); }, { flush: 'sync' });
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
  if (props.active) void voiceCall.setProbe(false);
});

const openPresets = async () => {
  if (disabled.value) return;
  openingPresets.value = true;
  try {
    // A covering page ends this visit in the voice overlay. Commit the visible
    // draft before navigating, just like leaving the global settings page.
    if (draft.value && !(await session.save()).ok) return;
    if (props.active) overlayStore.openVoicePresets();
  } finally {
    openingPresets.value = false;
  }
};

const openSource = (url: string) => {
  if (props.active) void openUrl(url).catch(error => console.warn('[VoiceAudio] Cannot open attribution:', error));
};

defineExpose({ beforeLeave: () => openingPresets.value ? Promise.resolve(false) : session.prepareLeave() });
</script>

<template>
  <SettingsCanvas viewport :inert="!active">
        <p v-if="error" role="alert" class="m-0 text-sm text-red-500">
          {{ draft ? '语音设置保存失败，请重试。' : '语音设置加载失败，请重试。' }}
          <button v-if="!draft" type="button" class="ml-2 underline" :disabled="loading || !active" @click="reload">重试</button>
        </p>
        <p v-else-if="!draft && loading" class="m-0 text-sm opacity-60" role="status">正在加载设置…</p>

        <template v-if="draft">
          <SettingsSection title="服务连接" description="MiMo · 识别与朗读共用">
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
                {{ draft.voiceApiKey.trim() ? '保存后，重新开启麦克风或开始新的朗读时生效。' : '填写并保存 API Key 后，可进行语音识别与朗读；配置在下次开麦或新朗读时生效。' }}
              </p>
            </template>
          </SettingsSection>

          <SettingsSection title="语音识别 ASR">
            <template #action><code class="settings-section-meta font-mono">mimo-v2.5-asr</code></template>
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
            <template #action><code class="settings-section-meta font-mono">mimo-v2.5-tts</code></template>
            <div>
              <SettingsRow title="朗读音色" :value="selectedLabel('voiceTtsVoice')" clickable
                :disabled="disabled" aria-label="朗读音色" aria-haspopup="dialog"
                :aria-expanded="selectionOpen('voiceTtsVoice')" @click="openSelection('voiceTtsVoice', $event)" />
              <SettingsRow title="流式朗读" description="收到音频片段后开始播放；关闭则等待完整合成。">
                <template #action>
                  <SettingsSwitch :model-value="draft.voiceTtsStream" :disabled="disabled" aria-label="流式朗读"
                    @update:model-value="session.edit('voiceTtsStream', $event)" />
                </template>
              </SettingsRow>
            </div>
          </SettingsSection>
        </template>

        <SettingsRow title="语音预设" description="回复风格与提示词" :icon="ListChecks" clickable
          :disabled="disabled" class="voice-preset-entry" @click="openPresets" />

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

    <template v-if="draft" #actions>
        <div class="min-w-0">
          <p class="m-0 text-xs font-medium" :class="error ? 'text-red-500' : ''" role="status" aria-live="polite">
            {{ saving || leaving ? '正在保存…' : error ? '保存失败，请重试' : dirty ? '有未保存修改' : '暂无修改' }}
          </p>
        </div>
        <button type="button" class="voice-settings-save shrink-0" :disabled="disabled || saving || !dirty" @click="save">
          {{ saving || leaving ? '正在保存…' : '保存语音设置' }}
        </button>
    </template>
  </SettingsCanvas>
</template>

<style scoped>
.voice-connection-fields {
  display: grid;
  grid-template-columns: minmax(0, 1fr);
  gap: 14px;
}

.voice-probe-button,
.voice-settings-save {
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

.voice-settings-save { align-self: center; font-weight: 600; }
.voice-settings-save:enabled { @apply bg-green-700 border-green-700 text-white; }
.voice-probe-button:disabled,
.voice-settings-save:disabled { opacity: 0.4; cursor: default; }

@media (min-width: 768px) {
  .voice-connection-fields { grid-template-columns: repeat(2, minmax(0, 1fr)); }
}
</style>

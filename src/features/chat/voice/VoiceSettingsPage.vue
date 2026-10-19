<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue';
import { ChevronRight, ListChecks, Play, Square } from 'lucide-vue-next';
import { openUrl } from '@tauri-apps/plugin-opener';
import { useOverlayStore } from '../../../core/stores/overlay';
import SettingsSection from '../../../components/settings/SettingsSection.vue';
import SettingsTextField from '../../../components/settings/SettingsTextField.vue';
import SettingsRow from '../../../components/settings/SettingsRow.vue';
import SettingsSwitch from '../../../components/settings/SettingsSwitch.vue';
import type { SettingsPageProps } from '../../settings/contracts';
import { VOICE_FIELDS } from '../../settings/editing/fieldSets';
import { useSettingsPageSession } from '../../settings/editing/useSettingsPageSession';
import { useVoiceCallStore } from './voiceCallStore';
import { DEFAULT_VOICE_SETTINGS, MIMO_VOICES, type VoiceInputEncoding, type VoiceLanguage, type VoiceTtsVoice } from './settings';

const props = defineProps<SettingsPageProps>();
const overlayStore = useOverlayStore();
const voiceCall = useVoiceCallStore();
const { session, loading, error, reload } = useSettingsPageSession(VOICE_FIELDS, () => props.active);
const { draft, dirty, saving, leaving } = session;
const openingPresets = ref(false);
const disabled = computed(() => !props.active || leaving.value || openingPresets.value);
const save = () => { if (!disabled.value) void session.save(); };
const editLanguage = (event: Event) => {
  if (!disabled.value) session.edit('voiceLanguage', (event.target as HTMLSelectElement).value as VoiceLanguage);
};
const editInputEncoding = (event: Event) => {
  if (!disabled.value) session.edit('voiceInputEncoding', (event.target as HTMLSelectElement).value as VoiceInputEncoding);
};
const editVoice = (event: Event) => {
  if (!disabled.value) session.edit('voiceTtsVoice', (event.target as HTMLSelectElement).value as VoiceTtsVoice);
};
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
  <div class="flex flex-col gap-6 text-primary-text" :inert="!active">
    <p v-if="error" role="alert" class="m-0 text-sm text-red-500">
      {{ draft ? '语音设置保存失败，请重试。' : '语音设置加载失败，请重试。' }}
      <button v-if="!draft" type="button" class="ml-2 underline" :disabled="loading || !active" @click="reload">重试</button>
    </p>
    <p v-else-if="!draft && loading" class="m-0 text-sm opacity-60" role="status">正在加载设置…</p>

    <template v-if="draft">
      <SettingsSection title="MiMo 语音服务" :card="false">
        <div class="flex flex-col gap-4">
          <SettingsTextField :model-value="draft.voiceApiBaseUrl" :disabled="disabled"
            label="API 地址" :placeholder="DEFAULT_VOICE_SETTINGS.voiceApiBaseUrl" mono
            @update:model-value="session.edit('voiceApiBaseUrl', $event)" />
          <SettingsTextField :model-value="draft.voiceApiKey" :disabled="disabled"
            label="语音 API Key" placeholder="输入 MiMo API Key" is-secure
            @update:model-value="session.edit('voiceApiKey', $event)" />
          <p class="m-0 px-1 text-xs leading-relaxed opacity-60">
            {{ draft.voiceApiKey.trim() ? '保存后，重新开启麦克风或开始新的朗读时生效。' : '填写并保存 API Key 后，可进行语音识别与朗读；配置在下次开麦或新朗读时生效。' }}
          </p>
        </div>
      </SettingsSection>

      <SettingsSection title="语音识别" :card="false">
        <div class="flex flex-col gap-2">
          <p class="m-0 px-1 text-xs opacity-60">模型：<code class="font-mono">mimo-v2.5-asr</code></p>
          <label class="voice-select-row">
            <span class="text-sm font-semibold">识别语言</span>
            <select :value="draft.voiceLanguage" :disabled="disabled" aria-label="识别语言" @change="editLanguage">
              <option value="auto">自动识别</option>
              <option value="zh">中文</option>
              <option value="en">英文</option>
            </select>
          </label>
          <label class="voice-select-row">
            <span class="text-sm font-semibold">语音输入编码</span>
            <select :value="draft.voiceInputEncoding" :disabled="disabled" aria-label="语音输入编码" @change="editInputEncoding">
              <option value="wav">WAV 无损</option>
              <option value="mp3_64">MP3 64 kbps</option>
            </select>
          </label>
          <p class="m-0 px-1 text-xs leading-relaxed opacity-60">
            MP3 约节省 75% 上传流量，仅用于语音识别输入；保存后重新开启麦克风生效。
          </p>
          <SettingsRow title="流式识别结果" description="说完后整段上传，再逐步返回转写文本。">
            <template #action>
              <SettingsSwitch :model-value="draft.voiceAsrStream" :disabled="disabled" aria-label="流式识别结果"
                @update:model-value="session.edit('voiceAsrStream', $event)" />
            </template>
          </SettingsRow>
        </div>
      </SettingsSection>

      <SettingsSection title="语音朗读" :card="false">
        <div class="flex flex-col gap-2">
          <p class="m-0 px-1 text-xs opacity-60">模型：<code class="font-mono">mimo-v2.5-tts</code></p>
          <label class="voice-select-row">
            <span class="text-sm font-semibold">朗读音色</span>
            <select :value="draft.voiceTtsVoice" :disabled="disabled" aria-label="朗读音色" @change="editVoice">
              <option v-for="voice in MIMO_VOICES" :key="voice.value" :value="voice.value">{{ voice.label }}</option>
            </select>
          </label>
          <SettingsRow title="流式朗读" description="收到音频片段后开始播放；关闭则等待完整合成。">
            <template #action>
              <SettingsSwitch :model-value="draft.voiceTtsStream" :disabled="disabled" aria-label="流式朗读"
                @update:model-value="session.edit('voiceTtsStream', $event)" />
            </template>
          </SettingsRow>
          <button type="button" class="voice-settings-save text-blue-500" :disabled="disabled || saving || !dirty" @click="save">
            {{ saving || leaving ? '正在保存…' : '保存语音设置' }}
          </button>
        </div>
      </SettingsSection>
    </template>

    <SettingsSection title="回复方式" :card="false">
      <button type="button" :disabled="disabled" class="voice-preset-entry" @click="openPresets">
        <ListChecks :size="20" :stroke-width="1.8" class="shrink-0 opacity-60" aria-hidden="true" />
        <span class="flex flex-col gap-1 min-w-0 flex-1 text-left">
          <span class="text-sm font-semibold">语音预设</span>
          <span class="text-xs leading-relaxed opacity-60">回复风格与提示词</span>
        </span>
        <ChevronRight :size="18" class="shrink-0 opacity-40" aria-hidden="true" />
      </button>
    </SettingsSection>

    <SettingsSection title="本地回声诊断" :card="false">
      <div class="px-1 flex flex-col gap-3">
        <p class="m-0 text-sm leading-relaxed opacity-70">
          播放本地数字语音；播放时开口可检查误触与打断。样本长约 6.5 秒，播放一次后停止，不经过网络。
        </p>
        <p class="m-0 text-xs leading-relaxed opacity-60">{{ aecLabel }}</p>
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
    </SettingsSection>

    <p v-if="leaving" class="m-0 text-xs opacity-60" role="status">正在保存设置…</p>
  </div>
</template>

<style scoped>
.voice-select-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
  min-height: 48px;
  padding: 0 4px;
}

.voice-select-row select {
  min-height: 44px;
  max-width: 65%;
  padding: 8px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  color: inherit;
  background: var(--primary-bg);
  font-size: 13px;
}

.voice-probe-button,
.voice-settings-save {
  display: inline-flex;
  align-items: center;
  align-self: flex-start;
  gap: 8px;
  min-height: 40px;
  padding: 8px 12px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: transparent;
  color: inherit;
  font-size: 13px;
  cursor: pointer;
  touch-action: manipulation;
}

.voice-probe-button:disabled,
.voice-settings-save:disabled {
  opacity: 0.4;
  cursor: default;
}

.voice-preset-entry {
  display: flex;
  align-items: center;
  gap: 12px;
  width: 100%;
  min-height: 56px;
  padding: 12px 4px;
  color: inherit;
  background: transparent;
  border: 0;
  border-bottom: 1px solid var(--border-color);
  cursor: pointer;
  touch-action: manipulation;
  transition: opacity 120ms ease;
}

.voice-preset-entry:active {
  opacity: 0.65;
}

.voice-preset-entry:focus-visible {
  outline: 2px solid var(--highlight-text, var(--primary-text));
  outline-offset: 2px;
}

@media (prefers-reduced-motion: reduce) {
  .voice-preset-entry {
    transition: none;
  }
}
</style>

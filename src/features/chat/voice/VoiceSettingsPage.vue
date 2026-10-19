<script setup lang="ts">
import { computed, onBeforeUnmount, watch } from 'vue';
import { ChevronRight, ListChecks, Play, Square } from 'lucide-vue-next';
import { openUrl } from '@tauri-apps/plugin-opener';
import { useOverlayStore } from '../../../core/stores/overlay';
import SettingsSection from '../../../components/settings/SettingsSection.vue';
import type { SettingsPageProps } from '../../settings/contracts';
import { useVoiceCallStore } from './voiceCallStore';

const props = defineProps<SettingsPageProps>();
const overlayStore = useOverlayStore();
const voiceCall = useVoiceCallStore();
const canProbe = computed(() => props.active && voiceCall.active && voiceCall.microphoneState === 'on');
const probeBusy = computed(() => voiceCall.playbackActive || voiceCall.probePending);
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

const openPresets = () => {
  if (props.active) overlayStore.openVoicePresets();
};

const openSource = (url: string) => {
  if (props.active) void openUrl(url).catch(error => console.warn('[VoiceAudio] Cannot open attribution:', error));
};
</script>

<template>
  <div class="flex flex-col gap-6 text-primary-text" :inert="!active">
    <SettingsSection title="回复方式" :card="false">
      <button type="button" :disabled="!active" class="voice-preset-entry" @click="openPresets">
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

    <SettingsSection title="语音服务" :card="false">
      <p class="m-0 px-1 text-sm leading-relaxed opacity-70">
        语音识别与朗读尚未接入。后续可在这里配置 ASR、TTS 与通话选项。
      </p>
    </SettingsSection>
  </div>
</template>

<style scoped>
.voice-probe-button {
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

.voice-probe-button:disabled {
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

<script setup lang="ts">
import { computed } from 'vue';
import { Mic, MicOff, PhoneOff } from '@lucide/vue';
import type { VoiceServiceView } from './voiceConversation';

const props = defineProps<{
  microphoneState: 'off' | 'requesting' | 'on' | 'error';
  level: number;
  bands: readonly number[];
  serviceState?: VoiceServiceView['state'];
  serviceMessage?: string;
  partialText?: string;
  errorMessage?: string;
  canRetrySubmission?: boolean;
  retryPending?: boolean;
}>();

const emit = defineEmits<{
  toggleMicrophone: [];
  hangup: [];
  retrySubmission: [];
}>();

const microphoneOn = computed(() => props.microphoneState === 'on');
const requestingMicrophone = computed(() => props.microphoneState === 'requesting');
const inputLevel = computed(() => microphoneOn.value && Number.isFinite(props.level)
  ? Math.min(1, Math.max(0, props.level))
  : 0);
const status = computed(() => {
  // One presentation owner: errors stay visible, then transcript/service progress.
  // Capture transitions and VAD frames only affect the controls and waveform.
  if (props.errorMessage?.trim() || props.microphoneState === 'error') {
    return { text: props.errorMessage?.trim() || '麦克风暂不可用，请重试', error: true };
  }
  if (props.serviceState === 'error') {
    return { text: props.serviceMessage?.trim() || '语音服务暂不可用，请重试', error: true };
  }
  return {
    text: props.partialText || props.serviceMessage?.trim() || (microphoneOn.value ? '正在聆听' : ''),
    error: false,
  };
});
const microphoneLabel = computed(() => requestingMicrophone.value
  ? '取消麦克风开启'
  : microphoneOn.value ? '关闭麦克风' : '开启麦克风');

// Each bar follows its own measured frequency band, without random animation.
const bars = computed(() => Array.from({ length: 32 }, (_, index) => {
  const value = props.bands?.[index];
  return {
    height: microphoneOn.value && Number.isFinite(value) ? Math.min(1, Math.max(0, value)) : 0,
    hue: 160 + index * 4,
  };
}));
</script>

<template>
  <section class="voice-call-controls no-swipe" aria-label="语音控制">
    <div class="voice-waveform" aria-hidden="true" :style="{ opacity: microphoneOn ? 0.65 + inputLevel * 0.35 : 0.3 }">
      <span
        v-for="(bar, index) in bars"
        :key="index"
        class="voice-waveform-bar"
        :style="{ transform: `scaleY(${(2 + bar.height * 34) / 36})`, '--wave-hue': bar.hue }"
      ></span>
    </div>

    <div class="voice-control-status" role="status" aria-live="polite" aria-atomic="true">
      <p class="voice-service-status" :class="{ 'is-error': status.error }" :title="status.text">
        {{ status.text }}
      </p>
      <button v-if="canRetrySubmission" type="button" class="voice-retry-button"
        :disabled="retryPending" @click="emit('retrySubmission')">重新提交</button>
    </div>

    <div class="voice-control-actions">
      <div class="voice-control-action">
        <button
          type="button"
          class="voice-control-button voice-microphone-button"
          :class="{ 'is-on': microphoneOn }"
          :aria-label="microphoneLabel"
          :aria-pressed="microphoneOn"
          @click="emit('toggleMicrophone')"
        >
          <Mic v-if="microphoneOn || requestingMicrophone" :size="24" :stroke-width="1.8" aria-hidden="true" />
          <MicOff v-else :size="24" :stroke-width="1.8" aria-hidden="true" />
        </button>
        <span class="voice-control-label">麦克风</span>
      </div>

      <div class="voice-control-action">
        <button type="button" class="voice-control-button voice-hangup-button" aria-label="挂断并退出语音模式"
          @click="emit('hangup')">
          <PhoneOff :size="24" :stroke-width="1.8" aria-hidden="true" />
        </button>
        <span class="voice-control-label">挂断</span>
      </div>
    </div>
  </section>
</template>

<style scoped>
.voice-call-controls {
  display: flex;
  flex-direction: column;
  align-items: center;
  width: 100%;
  min-width: 0;
  padding: 8px 0 10px;
  color: var(--primary-text);
}

.voice-waveform {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 3px;
  width: 100%;
  height: 36px;
  pointer-events: none;
}

.voice-waveform-bar {
  width: 3px;
  height: 36px;
  flex: 0 0 3px;
  border-radius: 2px;
  background-color: hsl(var(--wave-hue), 75%, 43%);
  transform-origin: center;
  transition: transform 40ms ease-out;
}

.dark .voice-waveform-bar {
  background-color: hsl(var(--wave-hue), 80%, 65%);
}

.voice-control-status {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 8px;
  width: 100%;
  max-width: 420px;
  padding: 4px 8px 0;
  text-align: center;
}

.voice-service-status {
  min-width: 0;
  margin: 0;
  height: 16px;
  font-size: 11px;
  line-height: 16px;
  opacity: 0.6;
  white-space: nowrap;
  text-overflow: ellipsis;
  overflow: hidden;
}

.voice-service-status.is-error {
  color: #ef4444;
  opacity: 1;
}

.voice-retry-button {
  flex-shrink: 0;
  min-height: 32px;
  padding: 4px;
  border: 0;
  background: transparent;
  color: var(--primary-text);
  font-size: 12px;
  text-decoration: underline;
  cursor: pointer;
  touch-action: manipulation;
}

.voice-retry-button:disabled {
  opacity: 0.45;
  cursor: default;
}

.voice-control-actions {
  display: flex;
  justify-content: space-between;
  width: 184px;
  max-width: 100%;
  padding-top: 12px;
}

.voice-control-action {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 5px;
}

.voice-control-button {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 56px;
  height: 56px;
  border: 0;
  border-radius: 16px;
  cursor: pointer;
  touch-action: manipulation;
  transition: opacity 120ms ease, background-color 120ms ease;
}

.voice-control-button:active {
  opacity: 0.7;
}

.voice-control-button:focus-visible {
  outline: 2px solid var(--highlight-text, var(--primary-text));
  outline-offset: 4px;
}

.voice-control-button:disabled {
  cursor: default;
  opacity: 0.45;
}

.voice-microphone-button {
  color: var(--primary-text);
  background-color: var(--secondary-bg);
}

.voice-microphone-button.is-on {
  color: var(--primary-bg);
  background-color: var(--primary-text);
}

.voice-hangup-button {
  color: #fff;
  background-color: #dc2626;
}

.voice-control-label {
  font-size: 12px;
  line-height: 18px;
  opacity: 0.8;
}

@media (max-height: 520px) {
  .voice-call-controls {
    padding-top: 4px;
    padding-bottom: 4px;
  }

  .voice-waveform {
    height: 20px;
  }

  .voice-waveform-bar {
    height: 20px;
  }

  .voice-control-actions {
    padding-top: 6px;
  }
}

@media (prefers-reduced-motion: reduce) {
  .voice-waveform-bar,
  .voice-control-button {
    transition: none;
  }
}
</style>

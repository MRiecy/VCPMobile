<script setup lang="ts">
import { ChevronRight, ListChecks } from 'lucide-vue-next';
import { useOverlayStore } from '../../../core/stores/overlay';
import SettingsSection from '../../../components/settings/SettingsSection.vue';
import type { SettingsPageProps } from '../../settings/contracts';

const props = defineProps<SettingsPageProps>();
const overlayStore = useOverlayStore();

const openPresets = () => {
  if (props.active) overlayStore.openVoicePresets();
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

    <SettingsSection title="语音服务" :card="false">
      <p class="m-0 px-1 text-sm leading-relaxed opacity-70">
        语音识别与朗读尚未接入。后续可在这里配置 ASR、TTS 与通话选项。
      </p>
    </SettingsSection>
  </div>
</template>

<style scoped>
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

<script setup lang="ts">
import { computed } from 'vue';
import { ArrowLeft } from 'lucide-vue-next';
import SlidePage from '../../../components/ui/SlidePage.vue';
import VoicePresetsPage from './VoicePresetsPage.vue';
import VoiceSettingsPage from './VoiceSettingsPage.vue';

const props = defineProps<{
  mode: 'settings' | 'presets';
  isOpen: boolean;
  active: boolean;
  zIndex: number;
}>();

const emit = defineEmits<{ close: [] }>();
const title = computed(() => props.mode === 'settings' ? '语音设置' : '语音预设');
const pageActive = computed(() => props.isOpen && props.active);
const close = () => { if (pageActive.value) emit('close'); };
</script>

<template>
  <SlidePage :is-open="isOpen" :z-index="zIndex" :inert="!pageActive"
    :aria-hidden="!pageActive" :aria-label="title" role="dialog" :aria-modal="pageActive ? 'true' : undefined">
    <div class="voice-options-view flex flex-col h-full w-full min-w-0 text-primary-text">
      <header class="voice-options-header flex items-center gap-3 shrink-0 px-3 border-b border-[var(--border-color)]">
        <button type="button" class="voice-options-back min-w-11 min-h-11 flex items-center justify-center"
          :aria-label="`返回，关闭${title}`" @click="close">
          <ArrowLeft :size="22" :stroke-width="1.8" aria-hidden="true" />
        </button>
        <h2 class="m-0 text-base font-semibold">{{ title }}</h2>
      </header>
      <div class="flex-1 min-h-0 overflow-y-auto no-rubber-band px-4 pt-6 pb-[calc(var(--vcp-safe-bottom,0px)+24px)]">
        <VoiceSettingsPage v-if="mode === 'settings'" :active="pageActive" />
        <VoicePresetsPage v-else :active="pageActive" />
      </div>
    </div>
  </SlidePage>
</template>

<style scoped>
.voice-options-header {
  padding-top: var(--vcp-safe-top, 24px);
  padding-bottom: 8px;
}

.voice-options-back {
  color: inherit;
  background: transparent;
  border: 0;
  border-radius: 12px;
  touch-action: manipulation;
  transition: opacity 120ms ease;
}

.voice-options-back:active {
  opacity: 0.6;
}

.voice-options-back:focus-visible {
  outline: 2px solid var(--highlight-text, var(--primary-text));
  outline-offset: 2px;
}

@media (prefers-reduced-motion: reduce) {
  .voice-options-back {
    transition: none;
  }
}
</style>

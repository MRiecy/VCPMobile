<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, watch } from 'vue';
import { ArrowLeft } from '@lucide/vue';
import SlidePage from '../../../components/ui/SlidePage.vue';
import VoiceStyleGuidancePage from './VoiceStyleGuidancePage.vue';
import VoiceSettingsPage from './VoiceSettingsPage.vue';
import { useModalHistory } from '../../../core/composables/useModalHistory';
import type { SettingsPageHandle } from '../../settings/contracts';

const props = defineProps<{
  mode: 'settings' | 'styleGuidance';
  isOpen: boolean;
  active: boolean;
  zIndex: number;
}>();

const emit = defineEmits<{ close: [] }>();
const title = computed(() => props.mode === 'settings' ? '语音设置' : '风格指导');
const pageActive = computed(() => props.isOpen && props.active);
const pageHandle = ref<SettingsPageHandle | null>(null);
const visit = ref(0);
const navigationError = ref('');
const { registerModal } = useModalHistory();
let mounted = true;
let closing: Promise<void> | null = null;

// ModalHistory callbacks are synchronous. Retain its existing page entry while
// the settings session commits; emit close only after a confirmed save.
const close = (): boolean => {
  if (!mounted) return true;
  if (!pageActive.value || closing) return false;
  const serial = visit.value;
  navigationError.value = '';
  const operation = nextTick().then(() => mounted && pageActive.value && visit.value === serial
    ? pageHandle.value?.beforeLeave?.() ?? true : false)
    .then(allowed => { if (allowed && mounted && pageActive.value && visit.value === serial) emit('close'); })
    .catch(() => { if (mounted && pageActive.value && visit.value === serial) navigationError.value = '设置保存失败，请重试。'; })
    .finally(() => { if (closing === operation) closing = null; });
  closing = operation;
  return false;
};

watch(pageActive, active => {
  closing = null;
  if (active) {
    visit.value += 1;
    navigationError.value = '';
    // OverlayStore already owns this ID. Update its dismissal guard without
    // adding another history level, for either editable voice page.
    registerModal(props.mode === 'settings' ? 'Page:voiceSettings:' : 'Page:voiceStyleGuidance:', close, undefined, close);
  }
}, { immediate: true, flush: 'sync' });
onBeforeUnmount(() => { mounted = false; });
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
      <div class="flex-1 min-h-0 overflow-hidden flex flex-col">
        <p v-if="navigationError" class="shrink-0 px-4 py-2 text-sm text-red-500" role="alert">{{ navigationError }}</p>
        <VoiceSettingsPage v-if="mode === 'settings'" :key="visit" ref="pageHandle" :active="pageActive" />
        <VoiceStyleGuidancePage v-else :key="visit" ref="pageHandle" :active="pageActive" />
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

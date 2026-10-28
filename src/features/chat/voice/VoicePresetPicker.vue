<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, useId, watch } from 'vue';
import { Check, Circle, Search, X } from '@lucide/vue';
import { useModalHistory } from '../../../core/composables/useModalHistory';
import { MIMO_VOICES } from './settings';
import VoiceSelectionRow from './VoiceSelectionRow.vue';

const props = withDefaults(defineProps<{
  modelValue: string;
  active: boolean;
  disabled?: boolean;
  compact?: boolean;
  title?: string;
}>(), { title: '当前音色' });
const emit = defineEmits<{ 'update:modelValue': [value: string] }>();
const { registerModal, unregisterModal } = useModalHistory();
const modalId = `VoicePresetPicker:${useId()}`;
const open = ref(false);
const query = ref('');
const surface = ref<HTMLElement | null>(null);
const closeButton = ref<HTMLButtonElement | null>(null);
const current = computed(() => MIMO_VOICES.find(voice => voice.value === props.modelValue));
const value = computed(() => current.value?.label ?? '音色配置无效');
const choices = computed(() => {
  const term = query.value.trim().toLocaleLowerCase();
  return MIMO_VOICES.filter(voice => !term || `${voice.label} ${voice.description}`.toLocaleLowerCase().includes(term));
});
const close = () => { open.value = false; };
const show = (event: MouseEvent) => {
  if (!props.active || props.disabled) return;
  (event.currentTarget as HTMLElement).focus({ preventScroll: true });
  query.value = '';
  open.value = true;
};
const select = (voice: string) => {
  if (!open.value || !props.active || props.disabled) return;
  emit('update:modelValue', voice);
  close();
};
watch(open, async visible => {
  if (visible) {
    registerModal(modalId, close, () => surface.value);
    await nextTick();
    if (open.value) closeButton.value?.focus({ preventScroll: true });
  } else unregisterModal(modalId);
}, { flush: 'sync' });
watch(() => props.active, active => { if (!active) close(); }, { flush: 'sync' });
onBeforeUnmount(() => unregisterModal(modalId));
const stop = async () => { close(); };
defineExpose({ stop, cancelPending: close });
</script>

<template>
  <VoiceSelectionRow :title="title" :value="value" :compact="compact" :disabled="disabled || !active" :expanded="open" @click="show" />
  <Teleport to="#vcp-feature-overlays">
    <div v-if="open && active" class="vcp-viewport-surface fixed inset-0 z-sheet bg-black/50 pointer-events-auto" @click.self="close">
      <section ref="surface" class="voice-picker-panel vcp-safe-inline" role="dialog" aria-modal="true" aria-label="选择预制音色">
        <div class="voice-picker-handle" aria-hidden="true"></div>
        <header class="voice-picker-header">
          <h3>选择预制音色</h3>
          <button ref="closeButton" type="button" class="voice-picker-close" aria-label="关闭预制音色选择" @click="close"><X :size="18" /></button>
        </header>
        <label class="voice-picker-search">
          <Search :size="16" aria-hidden="true" />
          <input v-model="query" type="search" aria-label="搜索预制音色" placeholder="搜索音色" />
        </label>
        <div class="voice-picker-options vcp-scrollable" role="radiogroup" aria-label="预制音色">
          <button v-for="voice in choices" :key="voice.value" type="button" class="voice-picker-option" role="radio"
            :aria-checked="voice.value === modelValue" :class="{ selected: voice.value === modelValue }" :disabled="disabled" @click="select(voice.value)">
            <span class="voice-picker-copy"><span class="voice-picker-name">{{ voice.label }}</span><span v-if="voice.description" class="voice-picker-detail">{{ voice.description }}</span></span>
            <Check v-if="voice.value === modelValue" :size="20" aria-hidden="true" /><Circle v-else :size="14" aria-hidden="true" />
          </button>
          <p v-if="choices.length === 0" class="voice-picker-empty">没有匹配的音色</p>
        </div>
      </section>
    </div>
  </Teleport>
</template>

<style scoped>
.voice-picker-panel { position: absolute; inset: auto 0 0; display: flex; flex-direction: column; height: min(88%, 42rem); padding-bottom: var(--vcp-safe-bottom, 24px); border: 1px solid var(--border-color); border-bottom: 0; border-radius: 12px 12px 0 0; background: var(--primary-bg); color: var(--primary-text); }
.voice-picker-handle { flex-shrink: 0; width: 28px; height: 3px; margin: 9px auto 0; border-radius: 2px; background: var(--border-color); }
.voice-picker-header { flex-shrink: 0; display: flex; align-items: center; justify-content: space-between; min-height: 58px; padding: 0 16px; border-bottom: 1px solid var(--border-color); }
.voice-picker-header h3 { margin: 0; font-size: 15px; font-weight: 600; }
.voice-picker-close { width: 44px; height: 44px; display: grid; place-items: center; margin-right: -10px; border: 0; background: transparent; color: inherit; }
.voice-picker-search { flex-shrink: 0; display: flex; align-items: center; gap: 8px; min-height: 40px; margin: 14px 16px 10px; padding: 0 10px; border: 1px solid var(--border-color); border-radius: 6px; color: var(--secondary-text); background: color-mix(in srgb, var(--primary-text) 3%, transparent); }
.voice-picker-search input { min-width: 0; width: 100%; padding: 9px 0; border: 0; outline: none; background: transparent; color: var(--primary-text); font-size: 13px; }
:global(html[data-vcp-platform="ios"]) .voice-picker-search input { font-size: 16px; }
.voice-picker-search:has(input:focus-visible), button:focus-visible { outline: 2px solid var(--highlight-text, #3b82f6); outline-offset: 2px; }
.voice-picker-options { min-height: 0; overflow-y: auto; padding: 0 16px; }
.voice-picker-option { position: relative; display: flex; align-items: center; gap: 12px; width: 100%; min-height: 68px; padding: 12px 0; border: 0; border-top: 1px solid var(--border-color); background: transparent; color: inherit; text-align: left; touch-action: manipulation; }
.voice-picker-copy { min-width: 0; display: flex; flex: 1; flex-direction: column; gap: 5px; }
.voice-picker-name { font-size: 14px; font-weight: 500; }
.voice-picker-detail { font-size: 12px; color: var(--secondary-text); }
.voice-picker-option > svg { flex-shrink: 0; color: var(--secondary-text); }
.voice-picker-option.selected { color: var(--highlight-text, #3b82f6); }
.voice-picker-option.selected > svg { color: inherit; }
.voice-picker-option.selected::before { position: absolute; content: ''; top: 15px; bottom: 15px; left: -8px; width: 2px; background: currentColor; }
.voice-picker-option:disabled { opacity: 0.4; }
.voice-picker-empty { padding: 20px 0; font-size: 13px; color: var(--secondary-text); }
</style>

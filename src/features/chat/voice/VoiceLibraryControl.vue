<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, shallowRef, useId, watch } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { Check, ChevronRight, Circle, Play, Plus, Search, Square, X } from '@lucide/vue';
import { useOverlayStore } from '../../../core/stores/overlay';
import { useModalHistory } from '../../../core/composables/useModalHistory';
import { LAYER_SHEET } from '../../../core/constants/layers';
import SlidePage from '../../../components/ui/SlidePage.vue';
import type { ConfirmConfig } from '../../../core/types/overlay';
import { useVoiceLibraryStore, voiceSampleDetail, type VoiceCloneModelDto } from './voiceLibraryStore';
import { useVoicePreviewTask } from './useVoicePreviewTask';
import VoiceCloneUpload from './VoiceCloneUpload.vue';
import VoiceSelectionRow from './VoiceSelectionRow.vue';

const props = defineProps<{ modelValue: string; active: boolean; disabled?: boolean; compact?: boolean }>();
const emit = defineEmits<{ 'update:modelValue': [id: string] }>();
const library = useVoiceLibraryStore();
const overlay = useOverlayStore();
const { registerModal, unregisterModal } = useModalHistory();
const managerId = `VoiceLibrary:${useId()}`;
const pickerId = `VoiceClonePicker:${useId()}`;
const pickerOpen = ref(false);
const managerOpen = ref(false);
const uploadOpen = ref(false);
const managerSurface = ref<HTMLElement | null>(null);
const pickerSurface = ref<HTMLElement | null>(null);
const pickerCloseButton = ref<HTMLButtonElement | null>(null);
const closeButton = ref<HTMLButtonElement | null>(null);
const pickerQuery = ref('');
const managerQuery = ref('');
const detailId = ref('');
const upload = ref<InstanceType<typeof VoiceCloneUpload> | null>(null);
const deleting = ref(false);
const stopping = ref(false);
const operationError = ref('');
const playingId = ref('');
const preview = useVoicePreviewTask(() => props.active && (managerOpen.value || pickerOpen.value), () => !!props.disabled || deleting.value || !!upload.value?.busy);
const { busy: previewBusy, feedback: previewError, unavailable } = preview;
const busy = computed(() => deleting.value || previewBusy.value || !!upload.value?.busy);
const locked = computed(() => busy.value || stopping.value);
const disabled = computed(() => !!props.disabled || deleting.value || !!upload.value?.busy);
const blocked = computed(() => !props.active || disabled.value || stopping.value);
const current = computed(() => library.samples.find(sample => sample.id === props.modelValue));
const currentLabel = computed(() => current.value?.name ?? '无');
const matches = (sample: VoiceCloneModelDto, query: string) => `${sample.name} ${sample.fileName} ${sample.format}`.toLocaleLowerCase().includes(query.trim().toLocaleLowerCase());
const pickerSamples = computed(() => library.samples.filter(sample => matches(sample, pickerQuery.value)));
const managerSamples = computed(() => library.samples.filter(sample => matches(sample, managerQuery.value)));
const showNone = computed(() => !pickerQuery.value.trim() || '无'.includes(pickerQuery.value.trim()));
const confirmation = shallowRef<ConfirmConfig | null>(null);
let generation = 0;
let mounted = true;

const refresh = () => library.refresh().catch(() => undefined);
const cancelPending = () => {
  generation += 1;
  if (confirmation.value && overlay.confirmConfig === confirmation.value) confirmation.value.onCancel();
  confirmation.value = null;
  upload.value?.cancelPending();
  uploadOpen.value = false;
};
const stop = async () => {
  cancelPending();
  stopping.value = true;
  try {
    await preview.stop();
    managerOpen.value = false;
    pickerOpen.value = false;
  } finally { stopping.value = false; }
};
const closeManager = async () => {
  cancelPending();
  stopping.value = true;
  try {
    await preview.stop();
    managerOpen.value = false;
  } catch {
    operationError.value = '停止原声试听失败，请重试。';
  } finally { stopping.value = false; }
};
const requestCloseManager = () => { void closeManager(); return false; };
const closePicker = async () => {
  generation += 1;
  stopping.value = true;
  try { await preview.stop(); pickerOpen.value = false; }
  catch { operationError.value = '停止原声试听失败，请重试。'; }
  finally { stopping.value = false; }
};
const requestClosePicker = () => { void closePicker(); return false; };
const select = async (id: string, source: 'picker' | 'manager') => {
  const sourceOpen = () => source === 'manager' ? managerOpen.value : pickerOpen.value && !managerOpen.value;
  if (blocked.value || !sourceOpen()) return;
  const currentGeneration = ++generation;
  try { await preview.stop(); } catch { return; }
  if (!props.active || !sourceOpen() || currentGeneration !== generation) return;
  emit('update:modelValue', id);
  if (source === 'manager') await closeManager();
  else await closePicker();
};

const openSelection = (event: MouseEvent) => {
  if (blocked.value) return;
  generation += 1;
  (event.currentTarget as HTMLElement).focus({ preventScroll: true });
  pickerQuery.value = '';
  operationError.value = '';
  pickerOpen.value = true;
  void refresh();
};

const openManager = (event: MouseEvent) => {
  if (blocked.value) return;
  generation += 1;
  (event.currentTarget as HTMLElement).focus({ preventScroll: true });
  operationError.value = '';
  managerQuery.value = '';
  detailId.value = '';
  managerOpen.value = true;
  void refresh();
};
watch(pickerOpen, async open => {
  if (open) {
    registerModal(pickerId, requestClosePicker, () => pickerSurface.value, undefined, () => false);
    await nextTick();
    if (pickerOpen.value) pickerCloseButton.value?.focus({ preventScroll: true });
  } else unregisterModal(pickerId);
}, { flush: 'sync' });
watch(managerOpen, async open => {
  if (open) {
    registerModal(managerId, requestCloseManager, () => managerSurface.value, undefined, () => false);
    await nextTick();
    if (managerOpen.value) closeButton.value?.focus({ preventScroll: true });
  } else unregisterModal(managerId);
}, { flush: 'sync' });

const toggleOriginal = async (sample: VoiceCloneModelDto) => {
  if (previewBusy.value && playingId.value === sample.id) {
    await preview.stop().catch(() => undefined);
    return;
  }
  if (blocked.value || !preview.canStart.value) return;
  const currentGeneration = generation;
  try { await preview.stop(); } catch { return; }
  if (!props.active || blocked.value || currentGeneration !== generation) return;
  playingId.value = sample.id;
  await preview.start((ttsId, documentId) => invoke('preview_voice_clone_sample', { ttsId, documentId, sampleId: sample.id }));
};
const remove = async (sample: VoiceCloneModelDto) => {
  if (blocked.value || !managerOpen.value) return;
  const currentGeneration = generation;
  try { await preview.stop(); } catch { return; }
  if (!props.active || blocked.value || !managerOpen.value || currentGeneration !== generation) return;
  const answer = overlay.showConfirm({
    title: '删除克隆音色',
    message: `确定删除「${sample.name}」及其本机音频吗？使用此音色的设置将显示为「无」。`,
    isDanger: true,
  });
  confirmation.value = overlay.confirmConfig;
  const confirmed = await answer;
  confirmation.value = null;
  if (!confirmed || !mounted || currentGeneration !== generation || blocked.value) return;
  deleting.value = true;
  operationError.value = '';
  try {
    await library.deleteSample(sample.id);
    if (detailId.value === sample.id) detailId.value = '';
  } catch (cause) {
    if (mounted && currentGeneration === generation && props.active) operationError.value = String(cause);
  } finally {
    deleting.value = false;
  }
};

const onImported = (id: string) => {
  if (props.active && managerOpen.value) emit('update:modelValue', id);
};
const openUpload = async () => {
  if (blocked.value || !managerOpen.value) return;
  const currentGeneration = generation;
  try { await preview.stop(); } catch { return; }
  if (props.active && managerOpen.value && currentGeneration === generation) uploadOpen.value = true;
};
watch(() => props.active, active => {
  if (active) void refresh();
  else {
    cancelPending();
    managerOpen.value = false;
    pickerOpen.value = false;
    void preview.stop().catch(() => undefined);
  }
}, { immediate: true, flush: 'sync' });
onBeforeUnmount(() => {
  mounted = false;
  cancelPending();
  unregisterModal(managerId);
  unregisterModal(pickerId);
});
defineExpose({ cancelPending, stop, busy, locked });
</script>

<template>
  <div class="voice-library-control" :class="{ 'voice-library-control--regular': !compact }" :inert="!active || stopping">
    <VoiceSelectionRow title="当前音色" :value="currentLabel" :compact="compact" :disabled="disabled" :expanded="pickerOpen" @click="openSelection" />
    <VoiceSelectionRow title="音色管理" :value="`${library.samples.length} 个音色`" :compact="compact" :disabled="disabled" :expanded="managerOpen" @click="openManager" />
    <p v-if="library.error" role="alert" class="voice-library-feedback">{{ library.error }} <button type="button" class="underline" :disabled="disabled || library.loading" @click="refresh">重试</button></p>
  </div>
  <Teleport to="#vcp-feature-overlays">
    <div v-if="pickerOpen && active" class="vcp-viewport-surface fixed inset-0 z-sheet bg-black/50 pointer-events-auto" :inert="managerOpen || stopping" @click.self="requestClosePicker">
      <section ref="pickerSurface" class="voice-clone-picker vcp-safe-inline" role="dialog" aria-modal="true" aria-label="选择复刻音色">
        <div class="voice-library-handle" aria-hidden="true"></div>
        <header class="voice-library-header">
          <h3>选择复刻音色</h3>
          <button ref="pickerCloseButton" type="button" class="voice-library-close" :disabled="stopping && previewBusy" aria-label="关闭复刻音色选择" @click="requestClosePicker"><X :size="18" /></button>
        </header>
        <label class="voice-library-search">
          <Search :size="16" aria-hidden="true" /><input v-model="pickerQuery" type="search" aria-label="搜索复刻音色" placeholder="搜索音色" />
        </label>
        <div class="voice-library-list vcp-scrollable" role="radiogroup" aria-label="复刻音色">
          <div v-if="showNone" class="voice-library-choice" :class="{ selected: !current }">
            <button type="button" class="voice-library-select" role="radio" :aria-checked="!current" :disabled="disabled" @click="select('', 'picker')">
              <span class="voice-library-copy"><span class="voice-library-name">无</span></span><Check v-if="!current" :size="20" aria-hidden="true" /><Circle v-else :size="14" aria-hidden="true" />
            </button>
            <span class="voice-library-play" aria-hidden="true"></span>
          </div>
          <div v-for="sample in pickerSamples" :key="sample.id" class="voice-library-choice" :class="{ selected: sample.id === modelValue }">
            <button type="button" class="voice-library-select" role="radio" :aria-checked="sample.id === modelValue" :disabled="disabled" @click="select(sample.id, 'picker')">
              <span class="voice-library-copy"><span class="voice-library-name">{{ sample.name }}</span><span class="voice-library-meta">{{ voiceSampleDetail(sample) }}</span></span>
              <Check v-if="sample.id === modelValue" :size="20" aria-hidden="true" /><Circle v-else :size="14" aria-hidden="true" />
            </button>
            <button type="button" class="voice-library-play" :aria-label="`${previewBusy && playingId === sample.id ? '停止原声' : '试听原声'}：${sample.name}`"
              :disabled="previewBusy && playingId === sample.id ? stopping : disabled || !preview.canStart.value" @click="toggleOriginal(sample)">
              <Square v-if="previewBusy && playingId === sample.id" :size="17" aria-hidden="true" /><Play v-else :size="18" aria-hidden="true" />
            </button>
          </div>
          <p v-if="!library.loading && !pickerSamples.length" class="voice-library-empty">{{ pickerQuery ? '没有匹配的音色' : '暂无复刻音色，可在音色管理中新增。' }}</p>
        </div>
        <p v-if="operationError || previewError || library.error" role="alert" class="voice-library-feedback">{{ operationError || previewError || library.error }}</p>
        <footer class="voice-library-picker-footer">
          <span>{{ library.samples.length }} 个音色</span><button type="button" :disabled="disabled" @click="openManager">音色管理</button>
        </footer>
      </section>
    </div>
    <SlidePage :is-open="managerOpen && active" :z-index="LAYER_SHEET" :aria-hidden="!managerOpen || !active"
      :inert="!managerOpen || !active || uploadOpen || !!confirmation || stopping">
      <section ref="managerSurface" class="voice-library-manager" role="dialog" aria-modal="true" aria-label="音色管理">
        <header class="voice-library-header">
          <h3>音色管理</h3>
          <button ref="closeButton" type="button" class="voice-library-close" :disabled="stopping && previewBusy" aria-label="关闭音色管理" @click="requestCloseManager"><X :size="18" /></button>
        </header>
        <div class="voice-library-toolbar">
          <label class="voice-library-search"><Search :size="16" aria-hidden="true" /><input v-model="managerQuery" type="search" aria-label="搜索声音库" placeholder="搜索音色" /></label>
          <button type="button" class="voice-library-add" :disabled="disabled" aria-label="上传新音色" @click="openUpload"><Plus :size="15" aria-hidden="true" />新增</button>
        </div>
        <div class="voice-library-list vcp-scrollable">
          <p v-if="!library.loading && !managerSamples.length && !library.error" class="voice-library-empty">{{ managerQuery ? '没有匹配的音色' : '暂无音色，上传一段参考音频即可创建。' }}</p>
          <article v-for="sample in managerSamples" :key="sample.id" class="voice-library-item" :class="{ selected: sample.id === modelValue }">
            <button type="button" class="voice-library-detail-toggle" :aria-label="`音色详情：${sample.name}`" :aria-expanded="detailId === sample.id" @click="detailId = detailId === sample.id ? '' : sample.id">
              <span class="voice-library-copy"><span class="voice-library-name">{{ sample.name }}</span><span class="voice-library-meta">{{ voiceSampleDetail(sample) }}</span></span>
              <ChevronRight :size="17" :class="{ 'rotate-90': detailId === sample.id }" aria-hidden="true" />
            </button>
            <div v-if="detailId === sample.id" class="voice-library-detail">
              <p class="voice-library-file">{{ sample.fileName }}</p>
              <div class="voice-library-detail-actions">
                <button type="button" class="voice-library-action inline-flex items-center gap-1.5"
                  :disabled="previewBusy && playingId === sample.id ? stopping : disabled || !preview.canStart.value" @click="toggleOriginal(sample)">
                  <Square v-if="previewBusy && playingId === sample.id" :size="15" aria-hidden="true" /><Play v-else :size="15" aria-hidden="true" />
                  {{ previewBusy && playingId === sample.id ? '停止原声' : '试听原声' }}
                </button>
                <button type="button" class="voice-library-action text-blue-500" :disabled="disabled" @click="select(sample.id, 'manager')">选用</button>
                <button type="button" class="voice-library-action text-red-500" :disabled="disabled" @click="remove(sample)">删除</button>
              </div>
            </div>
          </article>
        </div>
        <p v-if="unavailable" class="px-4 text-sm text-[var(--secondary-text)]">{{ unavailable }}</p>
        <p v-if="operationError || previewError || library.error" role="alert" class="voice-library-feedback px-4">{{ operationError || previewError || library.error }}</p>
      </section>
    </SlidePage>
  </Teleport>
  <VoiceCloneUpload ref="upload" :open="uploadOpen" :active="active && managerOpen" @close="uploadOpen = false" @imported="onImported" />
</template>

<style scoped>
.voice-library-control--regular :deep(.settings-row:first-child) { border-top: 1px solid var(--settings-row-divider, var(--border-color)); }
.voice-library-feedback { margin: 0; padding: 10px 16px; font-size: 13px; color: var(--danger-color, #ef4444); overflow-wrap: anywhere; }
.voice-clone-picker { position: absolute; inset: auto 0 0; display: flex; flex-direction: column; height: min(76%, 40rem); padding-bottom: var(--vcp-safe-bottom, 24px); border: 1px solid var(--border-color); border-bottom: 0; border-radius: 12px 12px 0 0; background: var(--primary-bg); color: var(--primary-text); }
.voice-library-manager { display: flex; flex-direction: column; height: 100%; padding-top: var(--vcp-safe-top, 24px); padding-bottom: var(--vcp-safe-bottom, 24px); background: var(--primary-bg); color: var(--primary-text); }
.voice-library-handle { flex-shrink: 0; width: 28px; height: 3px; margin: 9px auto 0; border-radius: 2px; background: var(--border-color); }
.voice-library-header { flex-shrink: 0; display: flex; align-items: center; justify-content: space-between; gap: 12px; min-height: 58px; padding: 0 16px; border-bottom: 1px solid var(--border-color); }
.voice-library-header h3 { margin: 0; font-size: 15px; font-weight: 600; }
.voice-library-close { width: 44px; height: 44px; display: grid; place-items: center; margin-right: -10px; border: 0; background: transparent; color: inherit; }
.voice-library-search { display: flex; flex-shrink: 0; align-items: center; gap: 8px; min-height: 40px; margin: 14px 16px 10px; padding: 0 10px; border: 1px solid var(--border-color); border-radius: 6px; background: color-mix(in srgb, var(--primary-text) 3%, transparent); color: var(--secondary-text); }
.voice-library-search input { min-width: 0; width: 100%; padding: 9px 0; border: 0; outline: none; background: transparent; color: var(--primary-text); font-size: 13px; }
:global(html[data-vcp-platform="ios"]) .voice-library-search input { font-size: 16px; }
.voice-library-search:has(input:focus-visible), button:focus-visible { outline: 2px solid var(--highlight-text, #3b82f6); outline-offset: 2px; }
.voice-library-list { min-height: 0; overflow-y: auto; padding: 0 16px; }
.voice-library-manager .voice-library-list { flex: 1; }
.voice-library-choice { position: relative; display: flex; align-items: stretch; gap: 8px; border-top: 1px solid var(--border-color); }
.voice-library-select { min-width: 0; display: flex; flex: 1; align-items: center; gap: 12px; min-height: 68px; padding: 12px 0; border: 0; background: transparent; color: inherit; text-align: left; }
.voice-library-copy { min-width: 0; display: flex; flex: 1; flex-direction: column; gap: 6px; }
.voice-library-name { font-size: 14px; font-weight: 500; overflow-wrap: anywhere; }
.voice-library-meta { font-size: 11px; line-height: 1.5; font-family: var(--font-mono, monospace); color: var(--secondary-text); }
.voice-library-select > svg, .voice-library-detail-toggle > svg { flex-shrink: 0; color: var(--secondary-text); }
.voice-library-choice.selected .voice-library-select { color: var(--highlight-text, #3b82f6); }
.voice-library-choice.selected .voice-library-select > svg { color: inherit; }
.voice-library-choice.selected::before, .voice-library-item.selected::before { position: absolute; content: ''; top: 15px; left: -8px; width: 2px; height: 38px; background: var(--highlight-text, #3b82f6); }
.voice-library-play { width: 44px; flex-shrink: 0; display: grid; place-items: center; padding: 0; border: 0; background: transparent; color: var(--secondary-text); }
.voice-library-picker-footer { flex-shrink: 0; display: flex; align-items: center; justify-content: space-between; min-height: 60px; margin-top: auto; padding: 0 16px; border-top: 1px solid var(--border-color); font-size: 12px; color: var(--secondary-text); }
.voice-library-picker-footer button { min-height: 44px; border: 0; background: transparent; color: var(--highlight-text, #3b82f6); font-size: 14px; }
.voice-library-toolbar { display: flex; flex-shrink: 0; align-items: center; gap: 8px; padding: 14px 16px 10px; }
.voice-library-toolbar .voice-library-search { min-width: 0; flex: 1; margin: 0; }
.voice-library-add { display: inline-flex; align-items: center; justify-content: center; gap: 4px; min-height: 40px; padding: 8px 12px; border: 1px solid var(--border-color); border-radius: 6px; background: color-mix(in srgb, var(--primary-text) 4%, transparent); color: inherit; font-size: 13px; }
.voice-library-item { position: relative; border-top: 1px solid var(--border-color); }
.voice-library-item.selected .voice-library-name { color: var(--highlight-text, #3b82f6); }
.voice-library-detail-toggle { display: flex; align-items: center; gap: 12px; width: 100%; min-height: 68px; padding: 12px 0; border: 0; background: transparent; color: inherit; text-align: left; }
.voice-library-detail { padding: 0 0 10px; }
.voice-library-file { margin: 0 0 4px; font-size: 12px; color: var(--secondary-text); overflow-wrap: anywhere; }
.voice-library-detail-actions { display: flex; align-items: center; gap: 24px; }
.voice-library-action { min-height: 44px; padding: 8px 0; border: 0; background: transparent; font-size: 13px; touch-action: manipulation; }
.voice-library-empty { padding: 14px 0; font-size: 13px; color: var(--secondary-text); }
button:disabled { opacity: 0.4; }
</style>

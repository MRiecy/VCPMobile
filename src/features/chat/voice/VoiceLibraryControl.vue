<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, shallowRef, useId, watch } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { ChevronRight, Play, Square } from '@lucide/vue';
import { useOverlayStore } from '../../../core/stores/overlay';
import { useModalHistory } from '../../../core/composables/useModalHistory';
import type { ConfirmConfig, ContextMenuConfig, OverlayActionItem } from '../../../core/types/overlay';
import { useVoiceLibraryStore, voiceSampleDetail, type VoiceCloneModelDto } from './voiceLibraryStore';
import { useVoicePreviewTask } from './useVoicePreviewTask';
import VoiceCloneUpload from './VoiceCloneUpload.vue';

const props = defineProps<{ modelValue: string; active: boolean; disabled?: boolean }>();
const emit = defineEmits<{ 'update:modelValue': [id: string] }>();
const library = useVoiceLibraryStore();
const overlay = useOverlayStore();
const { registerModal, unregisterModal } = useModalHistory();
const managerId = `VoiceLibrary:${useId()}`;
const managerOpen = ref(false);
const uploadOpen = ref(false);
const managerSurface = ref<HTMLElement | null>(null);
const closeButton = ref<HTMLButtonElement | null>(null);
const upload = ref<InstanceType<typeof VoiceCloneUpload> | null>(null);
const deleting = ref(false);
const stopping = ref(false);
const operationError = ref('');
const playingId = ref('');
const preview = useVoicePreviewTask(() => props.active && managerOpen.value, () => !!props.disabled || deleting.value || !!upload.value?.busy);
const { busy: previewBusy, feedback: previewError, unavailable } = preview;
const busy = computed(() => deleting.value || stopping.value || previewBusy.value || !!upload.value?.busy);
const blocked = computed(() => !props.active || !!props.disabled || busy.value);
const current = computed(() => library.samples.find(sample => sample.id === props.modelValue));
const currentLabel = computed(() => current.value ? `${current.value.name} · ${(current.value.durationMs / 1000).toFixed(1)} 秒` : '无');
let selection: ContextMenuConfig | null = null;
const confirmation = shallowRef<ConfirmConfig | null>(null);
let generation = 0;
let mounted = true;

const refresh = () => library.refresh().catch(() => undefined);
const closeSelection = () => {
  if (selection && overlay.contextMenuConfig === selection) overlay.closeContextMenu();
  selection = null;
};
const cancelPending = () => {
  generation += 1;
  closeSelection();
  if (confirmation.value && overlay.confirmConfig === confirmation.value) confirmation.value.onCancel();
  confirmation.value = null;
  upload.value?.cancelPending();
  uploadOpen.value = false;
};
const stop = async () => {
  cancelPending();
  stopping.value = true;
  try { await preview.stop(); } finally { stopping.value = false; }
};
const closeManager = async () => {
  try {
    await stop();
    managerOpen.value = false;
  } catch {
    operationError.value = '停止原声试听失败，请重试。';
  }
};
const requestCloseManager = () => { void closeManager(); return false; };
const select = async (id: string) => {
  if (blocked.value) return;
  emit('update:modelValue', id);
  await closeManager();
};

const openSelection = async (event: MouseEvent) => {
  if (blocked.value) return;
  (event.currentTarget as HTMLElement).focus({ preventScroll: true });
  const currentGeneration = ++generation;
  try { await library.refresh(); } catch { return; }
  if (!mounted || currentGeneration !== generation || blocked.value) return;
  const actions: OverlayActionItem[] = [
    {
      id: 'voice-library-none', label: '无', selected: !current.value,
      handler: () => {
        if (!blocked.value && overlay.contextMenuConfig === selection) emit('update:modelValue', '');
      },
    },
    ...library.samples.map(sample => ({
      id: sample.id,
      label: `${sample.name} · ${(sample.durationMs / 1000).toFixed(1)} 秒 ${sample.format.toUpperCase()}`,
      selected: sample.id === props.modelValue,
      handler: () => {
        if (!blocked.value && overlay.contextMenuConfig === selection) emit('update:modelValue', sample.id);
      },
    })),
  ];
  overlay.openContextMenu(actions, '选择复刻音色');
  selection = overlay.contextMenuConfig;
};

const openManager = (event: MouseEvent) => {
  if (blocked.value) return;
  (event.currentTarget as HTMLElement).focus({ preventScroll: true });
  operationError.value = '';
  managerOpen.value = true;
  void refresh();
};
watch(managerOpen, async open => {
  if (open) {
    registerModal(managerId, requestCloseManager, () => managerSurface.value);
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
  playingId.value = sample.id;
  await preview.start(ttsId => invoke('preview_voice_clone_sample', { ttsId, sampleId: sample.id }));
};
const remove = async (sample: VoiceCloneModelDto) => {
  if (blocked.value) return;
  const currentGeneration = generation;
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
  } catch (cause) {
    if (mounted && currentGeneration === generation && props.active) operationError.value = String(cause);
  } finally {
    deleting.value = false;
  }
};

const onImported = (id: string) => {
  if (props.active && managerOpen.value) emit('update:modelValue', id);
};
watch(() => props.active, active => {
  if (active) void refresh();
  else {
    cancelPending();
    managerOpen.value = false;
    void preview.stop().catch(() => undefined);
  }
}, { immediate: true, flush: 'sync' });
onBeforeUnmount(() => {
  mounted = false;
  cancelPending();
  unregisterModal(managerId);
});
defineExpose({ cancelPending, stop, busy });
</script>

<template>
  <div class="voice-library-control">
    <button type="button" class="voice-library-row" :disabled="blocked" aria-label="当前音色" aria-haspopup="dialog" @click="openSelection">
      <span>当前音色</span><span class="voice-library-value">{{ library.loading ? '正在读取…' : currentLabel }}</span><ChevronRight :size="15" aria-hidden="true" />
    </button>
    <button type="button" class="voice-library-row" :disabled="blocked" aria-label="音色管理" aria-haspopup="dialog" :aria-expanded="managerOpen" @click="openManager">
      <span>音色管理</span><span class="voice-library-value">{{ library.samples.length }} 个音色</span><ChevronRight :size="15" aria-hidden="true" />
    </button>
    <p v-if="library.error" role="alert" class="voice-library-feedback">{{ library.error }} <button type="button" class="underline" :disabled="blocked || library.loading" @click="refresh">重试</button></p>
  </div>
  <Teleport to="#vcp-feature-overlays">
    <div v-if="managerOpen && active" class="vcp-viewport-surface fixed inset-0 z-sheet bg-black/40 pointer-events-auto" @click.self="requestCloseManager">
      <section ref="managerSurface" class="voice-library-sheet vcp-safe-inline" role="dialog" aria-modal="true" aria-label="音色管理" :inert="uploadOpen || !!confirmation">
        <header class="voice-library-header">
          <h3 class="m-0 text-sm font-semibold">音色管理 <span class="ml-1 text-xs font-normal text-[var(--secondary-text)]">{{ library.samples.length }}</span></h3>
          <div class="flex items-center gap-3">
            <button type="button" class="voice-library-action text-blue-500" :disabled="blocked" @click="uploadOpen = true">上传新音色</button>
            <button ref="closeButton" type="button" class="voice-library-action" :disabled="stopping" aria-label="关闭音色管理" @click="requestCloseManager">关闭</button>
          </div>
        </header>
        <div class="min-h-0 overflow-y-auto vcp-scrollable">
          <p v-if="library.loading" role="status" class="p-4 text-xs text-[var(--secondary-text)]">正在读取声音库…</p>
          <p v-else-if="!library.samples.length && !library.error" class="p-4 text-xs text-[var(--secondary-text)]">暂无音色，上传一段参考音频即可创建。</p>
          <article v-for="sample in library.samples" :key="sample.id" class="voice-library-item">
            <div class="flex items-start justify-between gap-3">
              <div class="min-w-0">
                <p class="m-0 text-sm font-medium break-words">{{ sample.name }} <span v-if="sample.id === modelValue" class="ml-1 text-[11px] text-blue-500">当前选中</span></p>
                <p class="m-0 mt-1 text-[11px] font-mono text-[var(--secondary-text)]">{{ voiceSampleDetail(sample) }}</p>
                <p class="m-0 mt-1 text-[11px] text-[var(--secondary-text)] break-all">{{ sample.fileName }}</p>
              </div>
              <button type="button" class="voice-library-action shrink-0 text-blue-500" :disabled="blocked" @click="select(sample.id)">选用</button>
            </div>
            <div class="flex items-center gap-5">
              <button type="button" class="voice-library-action inline-flex items-center gap-1.5"
                :disabled="previewBusy && playingId === sample.id ? stopping : blocked || !preview.canStart.value" @click="toggleOriginal(sample)">
                <Square v-if="previewBusy && playingId === sample.id" :size="14" aria-hidden="true" /><Play v-else :size="14" aria-hidden="true" />
                {{ previewBusy && playingId === sample.id ? '停止原声' : '试听原声' }}
              </button>
              <button type="button" class="voice-library-action text-red-500" :disabled="blocked" @click="remove(sample)">删除</button>
            </div>
          </article>
        </div>
        <p v-if="unavailable" class="px-4 text-xs text-[var(--secondary-text)]">{{ unavailable }}</p>
        <p v-if="operationError || previewError || library.error" role="alert" class="voice-library-feedback px-4">{{ operationError || previewError || library.error }}</p>
      </section>
    </div>
  </Teleport>
  <VoiceCloneUpload ref="upload" :open="uploadOpen" :active="active && managerOpen" @close="uploadOpen = false" @imported="onImported" />
</template>

<style scoped>
.voice-library-row { display: flex; align-items: center; gap: 8px; width: 100%; min-height: 44px; padding: 10px 12px; text-align: left; border: 0; border-bottom: 1px solid var(--border-color); background: transparent; color: var(--primary-text); font-size: 12px; touch-action: manipulation; }
.voice-library-row:last-of-type { border-bottom: 0; }
.voice-library-value { margin-left: auto; min-width: 0; max-width: 65%; text-align: right; overflow-wrap: anywhere; color: var(--secondary-text); }
.voice-library-row:disabled, .voice-library-action:disabled { opacity: 0.4; }
.voice-library-feedback { margin: 0; padding: 8px 12px; font-size: 12px; color: var(--danger-color, #ef4444); overflow-wrap: anywhere; }
.voice-library-sheet { position: absolute; left: 0; right: 0; bottom: 0; display: flex; flex-direction: column; max-height: min(70%, calc(100% - var(--vcp-safe-top, 24px))); padding-bottom: var(--vcp-safe-bottom, 24px); border: 1px solid var(--border-color); border-radius: 8px 8px 0 0; background: var(--primary-bg); color: var(--primary-text); }
.voice-library-header { flex-shrink: 0; display: flex; align-items: center; justify-content: space-between; gap: 12px; padding: 0 16px; border-bottom: 1px solid var(--border-color); }
.voice-library-item { padding: 12px 16px 0; border-bottom: 1px solid var(--border-color); }
.voice-library-item:last-child { border-bottom: 0; }
.voice-library-action { min-height: 44px; padding: 8px 0; border: 0; background: transparent; font-size: 12px; touch-action: manipulation; }
</style>

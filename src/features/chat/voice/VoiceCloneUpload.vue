<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, shallowRef, useId, watch } from 'vue';
import { deleteTempFile, type PickedFile } from 'tauri-plugin-vcp-mobile';
import { cancelFileOperation, pickFile } from '../../../core/utils/nativeFilePicker';
import { useModalHistory } from '../../../core/composables/useModalHistory';
import { useVoiceLibraryStore } from './voiceLibraryStore';

const props = defineProps<{ open: boolean; active: boolean }>();
const emit = defineEmits<{ close: []; imported: [id: string] }>();
const library = useVoiceLibraryStore();
const { registerModal, unregisterModal } = useModalHistory();
const modalId = `VoiceCloneUpload:${useId()}`;
const surface = ref<HTMLElement | null>(null);
const nameInput = ref<HTMLInputElement | null>(null);
const name = ref('');
const picked = shallowRef<PickedFile | null>(null);
const picking = ref(false);
const importing = ref(false);
const error = ref('');
const busy = computed(() => picking.value || importing.value);
let generation = 0;
let pickerId: string | null = null;
let mounted = true;

const releasePicked = async (file: PickedFile | null) => {
  if (!file) return;
  await deleteTempFile(file.path).catch(console.warn);
  if (file.thumbnailPath) await deleteTempFile(file.thumbnailPath).catch(console.warn);
};
const cancelPending = () => {
  generation += 1;
  const operationId = pickerId;
  pickerId = null;
  if (operationId) void cancelFileOperation(operationId).catch(console.warn);
  const file = picked.value;
  picked.value = null;
  void releasePicked(file);
  picking.value = false;
  importing.value = false;
};
const close = () => {
  cancelPending();
  emit('close');
};
watch(() => props.open && props.active, async open => {
  if (open) {
    name.value = '';
    error.value = '';
    registerModal(modalId, close, () => surface.value);
    await nextTick();
    if (props.open && props.active) nameInput.value?.focus({ preventScroll: true });
  } else {
    cancelPending();
    unregisterModal(modalId);
  }
}, { immediate: true, flush: 'sync' });

const chooseFile = async () => {
  if (!props.open || !props.active || busy.value) return;
  const current = ++generation;
  const operationId = `voice-library-${crypto.randomUUID()}`;
  pickerId = operationId;
  picking.value = true;
  error.value = '';
  try {
    const result = await pickFile('file', { operationId, maxBytes: 7_000_000 });
    if (!mounted || current !== generation || !props.open || !props.active) {
      await releasePicked(result);
      return;
    }
    const previous = picked.value;
    picked.value = result;
    if (!name.value.trim()) name.value = result.name.replace(/\.[^/.]+$/, '').slice(0, 80);
    await releasePicked(previous);
  } catch (cause) {
    if (current === generation && props.open && props.active && !/cancelled|canceled|取消/i.test(String(cause))) {
      error.value = String(cause);
    }
  } finally {
    if (current === generation) { picking.value = false; pickerId = null; }
  }
};

const save = async () => {
  const file = picked.value;
  if (!props.open || !props.active || busy.value || !file || !name.value.trim()) return;
  const current = ++generation;
  // Once submitted, this operation owns the picker's copy until Rust finishes.
  picked.value = null;
  importing.value = true;
  error.value = '';
  try {
    const result = await library.importSample({
      path: file.path, fileName: file.name, name: name.value.trim(),
    });
    if (mounted && current === generation && props.open && props.active) {
      emit('imported', result.id);
      emit('close');
    }
    // A successful import is a permanent library asset, even after navigation.
  } catch (cause) {
    if (current === generation && props.open && props.active) error.value = String(cause);
  } finally {
    await releasePicked(file);
    if (current === generation) importing.value = false;
  }
};

onBeforeUnmount(() => {
  mounted = false;
  cancelPending();
  unregisterModal(modalId);
});
defineExpose({ cancelPending, busy });
</script>

<template>
  <Teleport to="#vcp-feature-overlays">
    <div v-if="open && active" class="vcp-viewport-surface fixed inset-0 z-dialog bg-black/50 pointer-events-auto flex items-center justify-center p-4" @click.self="close">
      <form ref="surface" class="voice-upload" role="dialog" aria-modal="true" aria-label="上传新音色" @submit.prevent="save">
        <header class="flex items-center justify-between gap-3 border-b border-black/10 dark:border-white/10">
          <h3 class="m-0 text-sm font-semibold">上传新音色</h3>
          <button type="button" class="voice-action" aria-label="关闭上传" @click="close">关闭</button>
        </header>
        <div class="flex flex-col gap-3 py-3 min-h-0 overflow-y-auto vcp-scrollable">
          <label class="flex flex-col gap-1.5 text-xs">
            音色名称
            <input ref="nameInput" v-model="name" maxlength="80" required :disabled="busy" class="voice-field" placeholder="为这个音色命名" />
          </label>
          <div class="flex flex-col gap-1.5">
            <span class="text-xs">参考音频</span>
            <button type="button" class="voice-file" :disabled="busy" @click="chooseFile">
              {{ picking ? '正在选择…' : importing ? '正在导入…' : picked?.name || '选择 MP3 / WAV 文件' }}
            </button>
            <p class="m-0 text-[11px] text-[var(--secondary-text)]">建议使用清晰的单人录音，文件不超过 7 MB。</p>
          </div>
          <p v-if="error" role="alert" class="m-0 text-xs text-red-500 break-words">{{ error }}</p>
        </div>
        <footer class="flex justify-end gap-3 border-t border-black/10 dark:border-white/10">
          <button type="button" class="voice-action" @click="close">取消</button>
          <button type="submit" class="voice-action text-blue-500" :disabled="busy || !picked || !name.trim()">{{ importing ? '正在入库…' : '保存入库' }}</button>
        </footer>
      </form>
    </div>
  </Teleport>
</template>

<style scoped>
.voice-upload { width: min(100%, 28rem); max-height: calc(100% - var(--vcp-safe-top, 24px) - var(--vcp-safe-bottom, 24px)); display: flex; flex-direction: column; padding: 0 16px; border: 1px solid var(--border-color); border-radius: 8px; background: var(--primary-bg); color: var(--primary-text); }
.voice-action { min-height: 44px; padding: 8px 4px; font-size: 12px; background: none; border: 0; touch-action: manipulation; }
.voice-action:disabled, .voice-field:disabled, .voice-file:disabled { opacity: 0.4; }
.voice-field { width: 100%; padding: 10px; border: 1px solid var(--border-color); border-radius: 8px; background: transparent; color: inherit; font: inherit; }
.voice-file { min-height: 60px; padding: 12px; border: 1px dashed var(--border-color); border-radius: 8px; background: transparent; color: inherit; font-size: 12px; overflow-wrap: anywhere; }
</style>

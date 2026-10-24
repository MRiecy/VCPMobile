<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { deleteTempFile, type PickedFile } from 'tauri-plugin-vcp-mobile';
import { cancelFileOperation, pickFile } from '../../../core/utils/nativeFilePicker';
import SettingsRow from '../../../components/settings/SettingsRow.vue';
import SettingsNote from '../../../components/settings/SettingsNote.vue';

interface CloneSample { id: string; fileName: string; format: 'mp3' | 'wav'; bytes: number }
const props = defineProps<{
  modelValue: string;
  active: boolean;
  disabled?: boolean;
  settleBeforeDiscard: () => Promise<unknown>;
}>();
const emit = defineEmits<{ 'update:modelValue': [id: string] }>();
const sample = ref<CloneSample | null>(null);
const loading = ref(false);
const importing = ref(false);
const error = ref('');
const imported = new Set<string>();
let generation = 0;
let lookup = 0;
let pickerId: string | null = null;
const detail = computed(() => sample.value
  ? `${sample.value.format.toUpperCase()} · ${sample.value.bytes < 1_000_000 ? `${(sample.value.bytes / 1000).toFixed(0)} KB` : `${(sample.value.bytes / 1_000_000).toFixed(2)} MB`}`
  : props.modelValue && !loading.value ? '样本文件缺失，请重新导入。' : '选择 MP3 / WAV 文件');
const discard = async (id: string) => {
  // Rust protects the persisted reference and live TTS readers.
  await invoke('discard_voice_clone_sample', { sampleId: id }).catch(() => {});
  imported.delete(id);
};
const cancelPending = () => {
  generation += 1;
  const operationId = pickerId;
  pickerId = null;
  importing.value = false;
  if (operationId) void cancelFileOperation(operationId).catch(() => {});
};
const cleanup = () => {
  cancelPending();
  lookup += 1;
  const ids = [...imported];
  // An already-submitted SQLite save continues after this view disappears.
  // Await its owner before Rust decides whether each imported ID is unreferenced.
  void props.settleBeforeDiscard().finally(() => {
    for (const id of ids) void discard(id);
  }).catch(() => {});
};
watch(() => [props.modelValue, props.active] as const, async ([id, active]) => {
  const revision = ++lookup;
  sample.value = null;
  error.value = '';
  loading.value = active && !!id;
  if (!id || !active) return;
  try {
    const result = await invoke<CloneSample | null>('get_voice_clone_sample', { sampleId: id });
    if (revision === lookup && props.active) sample.value = result;
  } catch {
    if (revision === lookup && props.active) error.value = '无法读取参考音频信息，请重新导入。';
  } finally {
    if (revision === lookup) loading.value = false;
  }
}, { immediate: true });
const importSample = async () => {
  if (!props.active || props.disabled || importing.value) return;
  const serial = ++generation;
  const operationId = `voice-sample-${crypto.randomUUID()}`;
  pickerId = operationId;
  importing.value = true;
  error.value = '';
  let picked: PickedFile | null = null;
  try {
    picked = await pickFile('file', { operationId, maxBytes: 7_000_000 });
    if (serial !== generation || !props.active) return;
    const result = await invoke<CloneSample>('import_voice_clone_sample', { path: picked.path, name: picked.name });
    imported.add(result.id);
    if (serial !== generation || !props.active) { await discard(result.id); return; }
    const previous = props.modelValue;
    emit('update:modelValue', result.id);
    if (previous !== result.id && imported.has(previous)) void discard(previous);
  } catch (cause) {
    if (serial === generation && props.active && !/cancelled|canceled|取消/i.test(String(cause))) {
      error.value = '导入失败，请选择有效的 MP3 / WAV，文件不能超过 7 MB。';
    }
  } finally {
    if (picked) {
      await deleteTempFile(picked.path).catch(() => {});
      if (picked.thumbnailPath) await deleteTempFile(picked.thumbnailPath).catch(() => {});
    }
    if (serial === generation) { importing.value = false; pickerId = null; }
  }
};
const remove = () => {
  if (!props.active || props.disabled || importing.value) return;
  const previous = props.modelValue;
  emit('update:modelValue', '');
  if (imported.has(previous)) void discard(previous);
};
watch(() => props.active, active => { if (!active) cleanup(); }, { flush: 'sync' });
onBeforeUnmount(cleanup);
defineExpose({ cancelPending, importing });
</script>

<template>
  <div class="flex flex-col gap-2">
    <SettingsRow title="参考音频" :description="detail" :value="importing ? '正在导入…' : loading ? '正在读取…' : sample?.fileName || '导入文件'"
      clickable :disabled="disabled || importing" aria-label="参考音频" @click="importSample" />
    <div v-if="modelValue" class="flex items-center gap-5">
      <button type="button" class="voice-sample-action" :disabled="disabled || importing" @click="importSample">替换音频</button>
      <button type="button" class="voice-sample-action" :disabled="disabled || importing" @click="remove">移除音频</button>
    </div>
    <p v-if="error" class="m-0 text-xs leading-relaxed text-red-500" role="alert">{{ error }}</p>
    <p v-else-if="modelValue && !loading && !sample" class="m-0 text-xs leading-relaxed text-red-500" role="alert">样本文件缺失，请重新导入。</p>
    <SettingsNote>
      建议使用简短、清晰的单人录音，文件大小 ≤ 7MB
    </SettingsNote>
  </div>
</template>

<style scoped>
.voice-sample-action { min-height: 44px; padding: 8px 0; border: 0; background: transparent; color: var(--secondary-text); font-size: 12px; text-decoration: underline; touch-action: manipulation; }
.voice-sample-action:disabled { opacity: 0.4; }
</style>

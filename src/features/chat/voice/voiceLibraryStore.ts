import { defineStore } from 'pinia';
import { ref, shallowRef } from 'vue';
import { invoke } from '@tauri-apps/api/core';

export interface VoiceCloneModelDto {
  id: string;
  name: string;
  fileName: string;
  format: 'mp3' | 'wav';
  fileBytes: number;
  durationMs: number;
  fileHash: string;
  createdAt: number;
  updatedAt: number;
}

export interface VoiceCloneImport {
  path: string;
  name: string;
  fileName: string;
}

export const voiceSampleDetail = (sample: VoiceCloneModelDto): string => {
  const size = sample.fileBytes < 1_000_000
    ? `${(sample.fileBytes / 1000).toFixed(0)} KB`
    : `${(sample.fileBytes / 1_000_000).toFixed(2)} MB`;
  return `${(sample.durationMs / 1000).toFixed(1)} 秒 · ${sample.format.toUpperCase()} · ${size}`;
};

/** A projection of the Rust registry; only acknowledged IPC results update it. */
export const useVoiceLibraryStore = defineStore('voiceLibrary', () => {
  const samples = shallowRef<VoiceCloneModelDto[]>([]);
  const loading = ref(false);
  const error = ref('');
  let revision = 0;

  const refresh = async () => {
    const current = ++revision;
    loading.value = true;
    error.value = '';
    try {
      const result = await invoke<VoiceCloneModelDto[]>('list_voice_clone_samples');
      if (current === revision) samples.value = result;
    } catch (cause) {
      if (current === revision) error.value = String(cause);
      throw cause;
    } finally {
      if (current === revision) loading.value = false;
    }
  };

  const importSample = async (input: VoiceCloneImport) => {
    const result = await invoke<VoiceCloneModelDto>('import_voice_clone_sample', { ...input });
    // A list requested before the import cannot replace this acknowledged asset.
    revision += 1;
    loading.value = false;
    samples.value = [result, ...samples.value.filter(sample => sample.id !== result.id)]
      .sort((left, right) => right.createdAt - left.createdAt);
    return result;
  };

  const deleteSample = async (sampleId: string) => {
    const deleted = await invoke<boolean>('delete_voice_clone_sample', { sampleId });
    revision += 1;
    loading.value = false;
    samples.value = samples.value.filter(sample => sample.id !== sampleId);
    return deleted;
  };

  return { samples, loading, error, refresh, importSample, deleteSample };
});

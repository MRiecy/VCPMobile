import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { createPinia, disposePinia, setActivePinia, type Pinia } from 'pinia';
import { useVoiceLibraryStore, type VoiceCloneModelDto } from '../../../features/chat/voice/voiceLibraryStore';
import { invokeMock, mockInvoke } from '../../mocks/tauri';

function sample(id: string): VoiceCloneModelDto {
  return {
    id, name: `Voice ${id}`, fileName: `${id}.wav`, format: 'wav', fileBytes: 48044,
    durationMs: 1000, fileHash: `hash-${id}`, createdAt: 100, updatedAt: 100,
  };
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(accept => { resolve = accept; });
  return { promise, resolve };
}
let pinia: Pinia;
beforeEach(() => { pinia = createPinia(); setActivePinia(pinia); });
afterEach(() => disposePinia(pinia));

describe('voice library acknowledgement projection', () => {
  it('does not lose a completed import when an earlier list response arrives late', async () => {
    const listing = deferred<VoiceCloneModelDto[]>();
    mockInvoke('list_voice_clone_samples', () => listing.promise);
    const saved = sample('new');
    mockInvoke('import_voice_clone_sample', () => saved);
    const store = useVoiceLibraryStore();
    const refreshing = store.refresh();
    const input = { path: '/native/staging/new.wav', name: 'My voice', fileName: 'new.wav' };
    await store.importSample(input);
    expect(invokeMock).toHaveBeenCalledWith('import_voice_clone_sample', input);
    expect(store.samples).toEqual([saved]);
    listing.resolve([]);
    await refreshing;
    expect(store.samples).toEqual([saved]);
    expect(store.loading).toBe(false);
  });

  it('retains the registered asset when Rust reports deletion failed', async () => {
    const asset = sample('bound');
    mockInvoke('list_voice_clone_samples', () => [asset]);
    mockInvoke('delete_voice_clone_sample', () => Promise.reject('声音库写入失败'));
    const store = useVoiceLibraryStore();
    await store.refresh();
    await expect(store.deleteSample(asset.id)).rejects.toBe('声音库写入失败');
    expect(store.samples).toEqual([asset]);
  });

  it('does not resurrect an acknowledged deletion through an earlier refresh', async () => {
    const asset = sample('deleted');
    mockInvoke('list_voice_clone_samples', () => [asset]);
    const store = useVoiceLibraryStore();
    await store.refresh();
    const listing = deferred<VoiceCloneModelDto[]>();
    mockInvoke('list_voice_clone_samples', () => listing.promise);
    const refreshing = store.refresh();
    mockInvoke('delete_voice_clone_sample', () => true);
    await store.deleteSample(asset.id);
    listing.resolve([asset]);
    await refreshing;
    expect(store.samples).toEqual([]);
  });

  it('does not fabricate an imported asset after the backend rejects the file', async () => {
    const asset = sample('existing');
    mockInvoke('list_voice_clone_samples', () => [asset]);
    mockInvoke('import_voice_clone_sample', () => Promise.reject('音频格式无效'));
    const store = useVoiceLibraryStore();
    await store.refresh();
    await expect(store.importSample({ path: '/native/staging/bad.wav', name: 'Bad', fileName: 'bad.wav' })).rejects.toBe('音频格式无效');
    expect(store.samples).toEqual([asset]);
  });
});

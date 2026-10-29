import { defineStore } from 'pinia';
import { onScopeDispose } from 'vue';
import { invoke } from '@tauri-apps/api/core';

/** One document owns independent playback; ordinary foreground recovery keeps it. */
export const useVoiceDocumentStore = defineStore('voiceDocument', () => {
  let allocated: Promise<string> | null = null;
  let connected: Promise<string> | null = null;
  let disposed = false;

  const connect = (): Promise<string> => {
    if (disposed) return Promise.reject(new Error('cancelled: voice document disposed'));
    if (connected) return connected;
    // Allocate before activation: a late allocation from an old, destroyed
    // document cannot supersede the current owner. Rust orders activations.
    if (!allocated) {
      allocated = invoke<string>('allocate_tts_document').catch(error => {
        allocated = null;
        throw error;
      });
    }
    const allocation = allocated;
    const attempt = (async () => {
      const documentId = await allocation;
      if (disposed) throw new Error('cancelled: voice document disposed');
      await invoke('connect_tts_document', { documentId });
      if (disposed) throw new Error('cancelled: voice document disposed');
      return documentId;
    })().catch(error => {
      if (connected === attempt) connected = null;
      throw error;
    });
    connected = attempt;
    return attempt;
  };

  onScopeDispose(() => { disposed = true; });
  return { connect };
});

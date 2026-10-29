import { computed, onBeforeUnmount, ref, watch } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { useVoiceCallStore } from './voiceCallStore';
import { useVoiceDocumentStore } from './voiceDocument';

interface PreviewTask {
  id: string;
  submitted: boolean;
  cancelled: boolean;
  ready: Promise<void>;
  stopping?: Promise<void>;
  unlisten?: UnlistenFn;
}

/** Frontend ownership of a Rust preview, shared by synthesis and source audio. */
export function useVoicePreviewTask(active: () => boolean, disabled: () => boolean) {
  const voiceCall = useVoiceCallStore();
  const voiceDocument = useVoiceDocumentStore();
  const busy = ref(false);
  const feedback = ref('');
  const unavailable = computed(() => voiceCall.active ? '通话结束后可试听。'
    : voiceCall.playbackActive ? '请等待当前朗读结束后再试听。' : undefined);
  const canStart = computed(() => active() && !disabled() && !voiceCall.active && !voiceCall.playbackActive);
  let task: PreviewTask | null = null;
  let mounted = true;

  const stop = (): Promise<void> => {
    const current = task;
    if (!current) return Promise.resolve();
    if (current.stopping) return current.stopping;
    current.cancelled = true;
    current.unlisten?.();
    const operation = current.ready.catch(() => undefined).then(async () => {
      // Even after navigation, await late admission before releasing its slot.
      if (current.submitted) await invoke('stop_tts', { ttsId: current.id });
      if (task === current) {
        task = null;
        if (mounted) busy.value = false;
      }
    }).catch(cause => {
      current.stopping = undefined;
      if (mounted && active()) feedback.value = '停止试听失败，请重试。';
      throw cause;
    });
    current.stopping = operation;
    return operation;
  };

  const start = async (submit: (ttsId: string, documentId: string) => Promise<unknown>) => {
    if (!canStart.value) return;
    if (task) {
      try { await stop(); } catch { return; }
      if (!canStart.value) return;
    }
    const current: PreviewTask = {
      id: `voice-preview-${crypto.randomUUID()}`, submitted: false, cancelled: false,
      ready: Promise.resolve(),
    };
    task = current;
    busy.value = true;
    feedback.value = '';
    current.ready = (async () => {
      const unlisten = await listen<{ ttsId: string; kind: string; message?: string }>('vcp-tts', event => {
        if (!mounted || current.cancelled || task !== current || event.payload.ttsId !== current.id) return;
        if (event.payload.kind === 'error' || event.payload.kind === 'stopped') {
          feedback.value = event.payload.kind === 'error' ? event.payload.message || '试听失败，请重试。' : '';
          busy.value = false;
          current.unlisten?.();
        }
      });
      current.unlisten = unlisten;
      const documentId = await voiceDocument.connect();
      if (!mounted || current.cancelled || task !== current || !canStart.value) {
        unlisten();
        if (task === current) { task = null; busy.value = false; }
        return;
      }
      current.submitted = true;
      await submit(current.id, documentId);
    })();
    try {
      await current.ready;
    } catch (cause) {
      if (mounted && !current.cancelled && task === current) {
        feedback.value = cause instanceof Error ? cause.message : String(cause);
        await stop().catch(() => undefined);
      }
    }
  };

  watch(active, value => { if (!value) void stop().catch(() => undefined); }, { flush: 'sync' });
  onBeforeUnmount(() => { mounted = false; void stop().catch(() => undefined); });
  return { busy, feedback, unavailable, canStart, start, stop };
}

import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { AcceptedWindow, ConversationKey } from '../../../core/stores/chatSessionStore';

export type VoiceCallTarget = Pick<ConversationKey, 'ownerId' | 'ownerType' | 'topicId'>;

interface VoiceInputDevice {
  id: string;
  label: string;
  kind: 'builtin' | 'bluetooth' | 'wired' | 'usb' | 'other';
}

export interface VoiceInputDevices {
  devices: VoiceInputDevice[];
  routeOptions?: VoiceInputDevice[];
  activeId: string | null;
}

export interface VoiceCallSnapshot {
  callId: string;
  target: VoiceCallTarget;
  revision: number;
  active: boolean;
  micState: 'closed' | 'starting' | 'listening' | 'stopping';
  micStopReason: string | null;
  inputState: 'idle' | 'recording' | 'transcribing' | 'submitting' | 'error';
  replyState: 'idle' | 'generating' | 'recovering' | 'stopping' | 'finished' | 'cancelled' | 'error';
  ttsState: 'idle' | 'ready' | 'synthesizing' | 'speaking' | 'finished' | 'cancelled' | 'error';
  speakingMessageId?: string;
  partial: string;
  error: string | null;
  notice: string | null;
  endReason?: 'target_deleted' | null;
  inputSessionId: string | null;
  inputDeviceId?: string | null;
  lastPersistedMessageId: string | null;
}

export interface VoiceServiceView {
  state: 'off' | 'local' | 'listening' | 'recognizing' | 'thinking' | 'speaking' | 'error';
  message: string;
  partialText: string;
}

interface CallScope {
  id: string;
  ready: Promise<void>;
  pending: boolean;
}

interface Options {
  conversation: () => ConversationKey | null;
  window: () => AcceptedWindow | null;
  connectDocument: () => Promise<string>;
  onView: (view: VoiceServiceView) => void;
  onSnapshot: (snapshot: VoiceCallSnapshot | null) => void;
  onTargetDeleted: (snapshot: VoiceCallSnapshot) => void;
}

const projectView = (snapshot: VoiceCallSnapshot): VoiceServiceView => {
  if (!snapshot.active) return { state: 'off', message: '', partialText: '' };
  const partialText = snapshot.partial.slice(0, 2000);
  if (snapshot.replyState === 'error' && snapshot.error) {
    return { state: 'error', message: snapshot.error.slice(0, 200), partialText };
  }
  if (snapshot.replyState === 'stopping') return { state: 'thinking', message: '正在停止回复', partialText };
  if (snapshot.ttsState === 'speaking') return { state: 'speaking', message: '正在朗读', partialText };
  if (snapshot.replyState === 'recovering') return { state: 'thinking', message: '模型连接暂不可用，正在等待接续。', partialText };
  if (snapshot.replyState === 'generating' || snapshot.ttsState === 'synthesizing') {
    return { state: 'thinking', message: snapshot.error?.slice(0, 200) || '正在回复', partialText };
  }
  if (snapshot.inputState === 'submitting') {
    return { state: 'recognizing', message: '正在发送', partialText };
  }
  if (snapshot.inputState === 'transcribing') {
    return { state: 'recognizing', message: '正在识别', partialText };
  }
  if (snapshot.error) return { state: 'error', message: snapshot.error.slice(0, 200), partialText };
  return { state: 'listening', message: snapshot.notice?.slice(0, 200) || '', partialText };
};

/** Vue sends user intents and projects Rust snapshots; it never owns a reply job. */
export function createVoiceConversation(options: Options) {
  let call: CallScope | null = null;
  let unlisten: UnlistenFn | null = null;
  let subscription: Promise<void> | null = null;
  let pendingCallEnd: { id: string; result: Promise<void> } | null = null;
  let revision = -1;
  let intentEpoch = 0;
  let lastEndedCallId: string | null = null;
  let projectedSnapshot: VoiceCallSnapshot | null = null;
  let microphoneIntent: { sessionId: string; submitted: boolean; observed: boolean } | null = null;
  let disposed = false;
  const diagnostic = (error: unknown) => console.warn('[VoiceConversation]', String(error).slice(0, 512));

  const applySnapshot = (snapshot: VoiceCallSnapshot, fromRecovery = false) => {
    if (disposed || !Number.isSafeInteger(snapshot.revision) || snapshot.revision <= revision) return;
    if (snapshot.active && snapshot.callId === lastEndedCallId) return;
    if (call && call.id !== snapshot.callId && (call.pending || !snapshot.active && !fromRecovery)) return;
    const announceDeletion = !fromRecovery && !snapshot.active && snapshot.endReason === 'target_deleted'
      && snapshot.callId !== lastEndedCallId;
    if (snapshot.active) {
      if (call?.id !== snapshot.callId) {
        microphoneIntent = null;
        call = { id: snapshot.callId, ready: Promise.resolve(), pending: false };
      }
    } else {
      lastEndedCallId = snapshot.callId;
      call = null;
    }
    if (microphoneIntent) {
      if (snapshot.inputSessionId === microphoneIntent.sessionId) microphoneIntent.observed = true;
      if (!snapshot.active || microphoneIntent.observed && snapshot.micState === 'closed') microphoneIntent = null;
    }
    revision = snapshot.revision;
    projectedSnapshot = snapshot;
    options.onSnapshot(snapshot);
    options.onView(projectView(snapshot));
    if (announceDeletion) options.onTargetDeleted(snapshot);
  };

  const subscribe = () => {
    if (disposed) return Promise.reject(new Error('voice_view_disposed'));
    if (subscription) return subscription;
    const installing = listen<VoiceCallSnapshot>('vcp-voice-call', ({ payload }) => applySnapshot(payload))
      .then(release => {
        if (disposed) release();
        else unlisten = release;
      }).catch(error => {
        if (subscription === installing) subscription = null;
        throw error;
      });
    subscription = installing;
    return installing;
  };

  const recover = async (): Promise<VoiceCallSnapshot | null> => {
    await subscribe();
    if (disposed) return null;
    const epoch = intentEpoch;
    const before = revision;
    // New documents revoke old capture through Rust. A cleanup failure must not
    // hide the surviving call or prevent its authoritative snapshot being read.
    void options.connectDocument().catch(diagnostic);
    const snapshot = await invoke<VoiceCallSnapshot | null>('get_voice_conversation_snapshot');
    if (disposed || epoch !== intentEpoch) return null;
    if (snapshot) applySnapshot(snapshot, true);
    else if (revision === before && !call?.pending) {
      if (call) lastEndedCallId = call.id;
      call = null;
      projectedSnapshot = null;
      options.onSnapshot(null);
      options.onView({ state: 'off', message: '', partialText: '' });
    }
    return projectedSnapshot;
  };

  const begin = () => {
    if (disposed) throw new Error('voice_view_disposed');
    if (call) return call.id;
    const key = options.conversation();
    const window = options.window();
    if (!key || !window) throw new Error('voice_conversation_unavailable');
    intentEpoch += 1;
    const run: CallScope = { id: crypto.randomUUID(), ready: Promise.resolve(), pending: true };
    call = run;
    const previousEnd = pendingCallEnd;
    run.ready = (async () => {
      await subscribe();
      if (previousEnd) await previousEnd.result.catch(() => {});
      if (disposed || call !== run) return;
      const { ownerId, ownerType, topicId } = key;
      const snapshot = await invoke<VoiceCallSnapshot>('begin_voice_conversation', {
        callId: run.id, target: { ownerId, ownerType, topicId },
        documentId: window.documentId, expectedFocusRevision: window.focusRevision,
      });
      if (snapshot.callId !== run.id) throw new Error('voice_call_not_active');
      if (!disposed && call === run) applySnapshot(snapshot);
    })().catch(error => {
      if (call === run) call = null;
      throw error;
    }).finally(() => { run.pending = false; });
    void run.ready.catch(diagnostic);
    return run.id;
  };

  const stop = (): Promise<void> => {
    const run = call;
    if (!run || pendingCallEnd?.id === run.id) return pendingCallEnd?.result ?? Promise.resolve();
    intentEpoch += 1;
    microphoneIntent = null;
    // Retirement is published before physical release. If that event was
    // missed, read the actual state on both success and failure.
    const confirm = async () => {
      if (!disposed && call === run) await recover();
    };
    const ending = invoke<void>('end_voice_conversation', { callId: run.id }).then(confirm, async error => {
      await confirm().catch(diagnostic);
      throw error;
    }).finally(() => { if (pendingCallEnd?.result === ending) pendingCallEnd = null; });
    pendingCallEnd = { id: run.id, result: ending };
    return ending;
  };

  const openMicrophone = async (expectedLifecycleEpoch?: number | null): Promise<{ configured: boolean } | null> => {
    begin();
    const run = call!;
    const epoch = intentEpoch;
    let admitted = false;
    // The UUID only correlates an early close with this intent. Rust owns
    // admission, physical release and any completion after this view is gone.
    const intent = { sessionId: crypto.randomUUID(), submitted: false, observed: false };
    microphoneIntent = intent;
    try {
      await run.ready;
      admitted = true;
      if (disposed || call !== run || microphoneIntent !== intent) return null;
      const documentId = await options.connectDocument();
      if (disposed || call !== run || microphoneIntent !== intent) return null;
      intent.submitted = true;
      const result = await invoke<{ configured: boolean }>('start_voice_microphone', {
        callId: run.id, sessionId: intent.sessionId, documentId,
        ...(expectedLifecycleEpoch === undefined ? {} : { expectedLifecycleEpoch }),
      });
      return !disposed && call === run && microphoneIntent === intent ? result : null;
    } catch (error) {
      if (disposed || epoch !== intentEpoch) return null;
      // Rust publishes inactive before returning a failed native begin. That
      // terminal clears the microphone intent but must not hide the start error.
      if (microphoneIntent !== intent &&
        (admitted || projectedSnapshot?.active !== false || projectedSnapshot.callId !== run.id)) return null;
      const detail = error instanceof Error ? error.message : String(error);
      if (detail.startsWith('cancelled') && projectedSnapshot?.micState === 'closed' && projectedSnapshot.micStopReason) return null;
      throw error;
    } finally {
      if (microphoneIntent === intent) microphoneIntent = null;
    }
  };

  const closeMicrophone = async (): Promise<void> => {
    const run = call;
    if (disposed || !run) return;
    const intent = microphoneIntent;
    microphoneIntent = null;
    // Cancel a local intent before it reaches IPC. Once submitted, close never
    // waits for permission/start, and its exact UUID survives early delivery.
    if (intent && !intent.submitted) return;
    const documentId = await options.connectDocument();
    if (disposed || call !== run) return;
    await invoke<void>('close_voice_microphone', {
      callId: run.id, documentId, ...(intent ? { sessionId: intent.sessionId } : {}),
    });
  };

  const inputDeviceScope = async () => {
    const run = call;
    if (!run) throw new Error('voice_call_not_active');
    await run.ready;
    if (disposed || call !== run) throw new Error('voice_call_not_active');
    const documentId = await options.connectDocument();
    if (disposed || call !== run) throw new Error('voice_call_not_active');
    return { callId: run.id, documentId };
  };

  const listInputs = async () =>
    invoke<VoiceInputDevices>('voice_call_list_inputs', await inputDeviceScope());

  const selectInput = async (inputDeviceId: string | null, expectedLifecycleEpoch: number | null) => {
    const expectedSessionId = projectedSnapshot?.inputSessionId ?? null;
    const scope = await inputDeviceScope();
    await invoke<void>('voice_call_select_input', { ...scope, inputDeviceId, expectedSessionId, expectedLifecycleEpoch });
  };

  const dispose = () => {
    disposed = true;
    intentEpoch += 1;
    microphoneIntent = null;
    unlisten?.();
    unlisten = null;
    // Listener disposal has no authority to cancel the Rust call or its work.
  };

  return { begin, stop, openMicrophone, closeMicrophone, listInputs, selectInput, recover, dispose };
}

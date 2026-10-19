import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { ConversationKey } from '../../../core/stores/chatSessionStore';

export type VoiceCallTarget = Pick<ConversationKey, 'ownerId' | 'ownerType' | 'topicId'>;

export interface VoiceCallSnapshot {
  callId: string;
  target: VoiceCallTarget;
  revision: number;
  active: boolean;
  micState: 'closed' | 'listening';
  inputState: 'idle' | 'recording' | 'transcribing' | 'submitting' | 'error';
  replyState: 'idle' | 'generating' | 'recovering' | 'stopping' | 'finished' | 'cancelled' | 'error';
  ttsState: 'idle' | 'ready' | 'synthesizing' | 'speaking' | 'finished' | 'cancelled' | 'error';
  partial: string;
  error: string | null;
  notice: string | null;
  inputSessionId: string | null;
  originUserMessageId: string | null;
  ttsId: string | null;
  lastPersistedMessageId: string | null;
}

export interface VoiceServiceView {
  state: 'off' | 'local' | 'listening' | 'recognizing' | 'thinking' | 'speaking' | 'error';
  message: string;
  partialText: string;
}

interface CallScope {
  id: string;
  key: ConversationKey;
  ready: Promise<void>;
  pending: boolean;
}

interface Options {
  conversation: () => ConversationKey | null;
  isConversationCurrent: (key: ConversationKey) => boolean;
  onView: (view: VoiceServiceView) => void;
  onSnapshot: (snapshot: VoiceCallSnapshot | null) => void;
}

const sameTarget = (key: VoiceCallTarget | null, target: VoiceCallTarget) =>
  !!key && key.ownerId === target.ownerId && key.ownerType === target.ownerType && key.topicId === target.topicId;

const projectView = (snapshot: VoiceCallSnapshot): VoiceServiceView => {
  if (!snapshot.active) return { state: 'off', message: '', partialText: '' };
  const partialText = snapshot.partial.slice(0, 2000);
  if (snapshot.replyState === 'stopping') return { state: 'thinking', message: '正在停止回复', partialText };
  if (snapshot.ttsState === 'speaking') return { state: 'speaking', message: '正在朗读', partialText };
  if (snapshot.replyState === 'recovering') return { state: 'thinking', message: '模型连接暂不可用，正在等待接续。', partialText };
  if (snapshot.replyState === 'generating' || snapshot.ttsState === 'synthesizing') {
    return { state: 'thinking', message: snapshot.error?.slice(0, 200) || '正在回复', partialText };
  }
  if (snapshot.inputState === 'transcribing' || snapshot.inputState === 'submitting') {
    return { state: 'recognizing', message: '正在识别', partialText };
  }
  if (snapshot.error) return { state: 'error', message: snapshot.error.slice(0, 200), partialText };
  return { state: 'listening', message: snapshot.notice?.slice(0, 200)
    || (snapshot.micState === 'listening' ? '正在聆听' : '麦克风已关闭'), partialText };
};

/** Vue sends user intents and projects Rust snapshots; it never owns a reply job. */
export function createVoiceConversation(options: Options) {
  let call: CallScope | null = null;
  let unlisten: UnlistenFn | null = null;
  let subscription: Promise<void> | null = null;
  let pendingCallEnd: Promise<void> | null = null;
  let revision = -1;
  let intentEpoch = 0;
  let endedCallId: string | null = null;
  let projectedSnapshot: VoiceCallSnapshot | null = null;
  let disposed = false;
  const diagnostic = (error: unknown) => console.warn('[VoiceConversation]', String(error).slice(0, 512));
  const command = (name: string, args: Record<string, unknown>) => invoke<void>(name, args).catch(diagnostic);

  const applySnapshot = (snapshot: VoiceCallSnapshot) => {
    if (disposed || !Number.isSafeInteger(snapshot.revision) || snapshot.revision <= revision) return;
    const key = options.conversation();
    if (!sameTarget(key, snapshot.target) || (call && call.id !== snapshot.callId)) return;
    if (snapshot.active && snapshot.callId === endedCallId) return;
    if (snapshot.active && key) {
      if (!call) call = { id: snapshot.callId, key: { ...key }, ready: Promise.resolve(), pending: false };
      if (!options.isConversationCurrent(call.key)) return;
    } else call = null;
    revision = snapshot.revision;
    projectedSnapshot = snapshot;
    options.onSnapshot(snapshot);
    options.onView(projectView(snapshot));
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
    const snapshot = await invoke<VoiceCallSnapshot | null>('get_voice_conversation_snapshot');
    if (disposed || epoch !== intentEpoch) return null;
    if (snapshot) applySnapshot(snapshot);
    else if (revision === before && !call?.pending) {
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
    if (!key) throw new Error('voice_conversation_unavailable');
    intentEpoch += 1;
    const run: CallScope = { id: crypto.randomUUID(), key: { ...key }, ready: Promise.resolve(), pending: true };
    call = run;
    const previousEnd = pendingCallEnd;
    run.ready = (async () => {
      await subscribe();
      if (previousEnd) await previousEnd;
      if (disposed || call !== run) return;
      const { ownerId, ownerType, topicId } = run.key;
      const snapshot = await invoke<VoiceCallSnapshot>('begin_voice_conversation', {
        callId: run.id, target: { ownerId, ownerType, topicId },
      });
      if (!disposed && call === run) applySnapshot(snapshot);
    })().catch(error => {
      if (call === run) call = null;
      throw error;
    }).finally(() => { run.pending = false; });
    void run.ready.catch(diagnostic);
    return run.id;
  };

  const stop = () => {
    intentEpoch += 1;
    const run = call;
    call = null;
    projectedSnapshot = null;
    if (run) {
      endedCallId = run.id;
      // Rust owns cancellation of unsent recognition, model work and TTS.
      // Do not wait for a permission dialog or for model completion in Vue.
      const ending = command('end_voice_conversation', { callId: run.id });
      pendingCallEnd = ending;
      void ending.finally(() => {
        if (pendingCallEnd === ending) pendingCallEnd = null;
      });
    }
    options.onSnapshot(null);
    options.onView({ state: 'off', message: '', partialText: '' });
  };

  const start = async (_sessionId: string): Promise<string> => {
    begin();
    const run = call!;
    await run.ready;
    if (disposed || call !== run || !options.isConversationCurrent(run.key)) throw new Error('cancelled');
    return run.id;
  };

  const dispose = () => {
    disposed = true;
    intentEpoch += 1;
    unlisten?.();
    unlisten = null;
    // Listener disposal has no authority to cancel the Rust call or its work.
  };

  return { begin, start, stop, recover, dispose };
}

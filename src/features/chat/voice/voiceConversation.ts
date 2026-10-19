import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { ConversationKey } from '../../../core/stores/chatSessionStore';
import type { VoiceMessageScope } from '../../../core/stores/chatHistoryStore';
import type { StreamEventDto } from '../../../core/types/chat';
import type { CaptureStopReceipt } from './microphoneMonitor';

export interface VoiceServiceEvent {
  sessionId: string;
  callId: string;
  kind: 'transcript' | 'asr-partial' | 'error' | 'status' | 'barge-in';
  turnId?: string;
  offerId?: string;
  batchId?: string;
  oldClaimedTurnId?: string;
  text?: string;
  stage?: string;
  message?: string;
}

export interface VoiceServiceView {
  state: 'off' | 'local' | 'listening' | 'recognizing' | 'thinking' | 'speaking' | 'error';
  message: string;
  partialText: string;
}

interface ReplyJob {
  id: string;
  origin: string;
  ttsId: string;
  cancelled: boolean;
  ttsDone: boolean;
  attempts: Map<string, 'request' | 'group'>;
  cancelledAttempts: Set<string>;
  unobserve?: () => void;
  completionTimer?: ReturnType<typeof setTimeout>;
  sendTask?: Promise<void>;
  sent: boolean;
  settled: boolean;
  activitySeen: boolean;
}

interface InputSession {
  id: string;
  configured: boolean;
  startSubmitted: boolean;
  closing: boolean;
}

interface CallScope {
  id: string;
  key: ConversationKey;
  ready: Promise<void>;
  beginSubmitted: boolean;
  hasConfiguredInput: boolean;
  unlisten: UnlistenFn[];
  inputs: Map<string, InputSession>;
  jobs: Set<ReplyJob>;
  seenTranscripts: Set<string>;
  latestJob?: ReplyJob;
}

interface TtsEvent {
  ttsId: string;
  kind: 'status' | 'error' | 'stopped';
  stage?: 'ready' | 'synthesizing' | 'speaking' | 'finished' | 'cancelled' | 'tts';
  message?: string;
}

interface Options {
  conversation: () => ConversationKey | null;
  isConversationCurrent: (key: ConversationKey) => boolean;
  observeChat: (listener: (event: StreamEventDto) => void) => () => void;
  sendMessage: (text: string, key: ConversationKey, scope: VoiceMessageScope) => Promise<void>;
  onView: (view: VoiceServiceView) => void;
}

const boundedRemember = (ids: Set<string>, id: string) => {
  ids.add(id);
  if (ids.size > 128) ids.delete(ids.values().next().value!);
};

/** Input capture, chat generation and TTS jobs have independent lifetimes. */
export function createVoiceConversation(options: Options) {
  let call: CallScope | null = null;
  let pendingSend: Promise<void> | null = null;
  let pendingInputClose: Promise<void> | null = null;
  let pendingCallEnd: Promise<void> | null = null;
  let pendingTtsStop: Promise<void> | null = null;
  let view: VoiceServiceView = { state: 'off', message: '', partialText: '' };
  const show = (patch: Partial<VoiceServiceView>) => {
    view = { ...view, ...patch };
    options.onView(view);
  };
  const ownsCall = (run: CallScope) => call === run && options.isConversationCurrent(run.key);
  const ownsJob = (run: CallScope, job: ReplyJob) => ownsCall(run) && !job.cancelled;
  const diagnostic = (error: unknown) => console.warn('[VoiceConversation]', String(error).slice(0, 512));
  const command = (name: string, args: Record<string, unknown>) => invoke<void>(name, args).catch(diagnostic);

  const forgetSettledJob = (run: CallScope, job: ReplyJob) => {
    if (!job.settled || !job.ttsDone) return;
    run.jobs.delete(job);
    if (run.latestJob === job) run.latestJob = undefined;
  };
  const releaseObserver = (job: ReplyJob) => {
    job.unobserve?.();
    job.unobserve = undefined;
    clearTimeout(job.completionTimer);
  };
  const cancelAttempt = (run: CallScope, job: ReplyJob, id: string, kind: 'request' | 'group') => {
    const token = kind + ':' + id;
    if (job.cancelledAttempts.has(token)) return;
    job.cancelledAttempts.add(token);
    void command('interrupt_chat_attempt', {
      ownerId: run.key.ownerId, ownerType: run.key.ownerType, topicId: run.key.topicId, attemptId: id, kind,
    });
  };
  const stopTts = (job: ReplyJob) => {
    const previous = pendingTtsStop;
    const stopping = command('stop_tts', { ttsId: job.ttsId });
    const settled = previous ? Promise.all([previous, stopping]).then(() => {}) : stopping;
    pendingTtsStop = settled;
    void settled.finally(() => { if (pendingTtsStop === settled) pendingTtsStop = null; }).catch(diagnostic);
    return settled;
  };
  const cancelJob = (run: CallScope, job: ReplyJob) => {
    job.cancelled = true;
    for (const [id, kind] of job.attempts) cancelAttempt(run, job, id, kind);
    return stopTts(job).then(() => { job.ttsDone = true; forgetSettledJob(run, job); });
  };

  const stop = () => {
    const run = call;
    if (!run) return;
    call = null;
    run.unlisten.forEach(unlisten => unlisten());
    run.unlisten = [];
    const stopping: Promise<unknown>[] = Array.from(run.inputs.values()).filter(input => input.startSubmitted)
      .map(input => command('stop_voice_service', { sessionId: input.id }));
    stopping.push(command('stop_voice_call_inputs', { callId: run.id }));
    run.inputs.clear();
    stopping.push(...Array.from(run.jobs, job => cancelJob(run, job)));
    if (pendingCallEnd) stopping.push(pendingCallEnd);
    // Do not await a pending begin/settings request or a model's terminal commit.
    const ending = Promise.allSettled(stopping).then(async () => {
      if (run.beginSubmitted) await command('plugin:vcp-mobile|end_voice_call', { callId: run.id });
    });
    pendingCallEnd = ending;
    void ending.finally(() => { if (pendingCallEnd === ending) pendingCallEnd = null; }).catch(diagnostic);
    show({ state: 'off', message: '', partialText: '' });
  };

  const acceptTranscript = (run: CallScope, event: VoiceServiceEvent) => {
    const text = event.text?.trim();
    const offerId = event.offerId;
    if (!text || !offerId || !ownsCall(run) || !run.hasConfiguredInput || run.seenTranscripts.has(offerId)) return;
    boundedRemember(run.seenTranscripts, offerId);
    // A replacement offer supersedes preparation that has not sent a chat
    // message; submitted jobs are stopped only by explicit barge-in/hangup.
    for (const previous of run.jobs) if (!previous.sent && !previous.cancelled) void cancelJob(run, previous);
    const job: ReplyJob = {
      id: offerId, origin: 'msg_voice_' + crypto.randomUUID(), ttsId: crypto.randomUUID(),
      cancelled: false, ttsDone: false, attempts: new Map(), cancelledAttempts: new Set(),
      sent: false, settled: false, activitySeen: false,
    };
    run.jobs.add(job);
    run.latestJob = job;
    show({ state: 'thinking', message: '正在回复', partialText: '' });
    job.unobserve = options.observeChat(event => {
      const context = event.context;
      if (context?.originUserMessageId !== job.origin || context.replayed === true
        || context.ownerId !== run.key.ownerId || context.ownerType !== run.key.ownerType || context.topicId !== run.key.topicId) return;
      if (event.type === 'request_activity' || event.type === 'group_turn') {
        const kind = event.type === 'request_activity' ? 'request' : 'group';
        const id = kind === 'request' ? context.requestId : context.turnId;
        if (typeof id === 'string') {
          job.activitySeen = true;
          if ((event.chunk as { active?: boolean } | null)?.active === false) job.attempts.delete(id);
          else {
            job.attempts.set(id, kind);
            if (!ownsJob(run, job)) cancelAttempt(run, job, id, kind);
          }
        }
        if (job.settled && job.attempts.size === 0) releaseObserver(job);
        return;
      }
      if (!ownsJob(run, job) || run.latestJob !== job) return;
      // Rust's real output owner feeds TTS, even without a WebView consumer.
      // Transport recovery/error is not a user request to stop either job.
      if (event.type === 'reconnecting' || event.type === 'error') {
        show({ state: 'thinking', message: '模型连接暂不可用，正在等待接续。' });
      } else if (event.type === 'stopping' || (event.type === 'end' && event.finishReason === 'cancelled_by_user')) {
        show({ state: 'listening', message: '模型回复已停止。' });
      }
    });

    const previousSend = pendingSend;
    const task = (async () => {
      // A cancelled group owner may still be committing its terminal message.
      // Only serialize our own voice sends, never unrelated typed requests.
      if (previousSend) await previousSend.catch(() => {});
      if (pendingTtsStop) await pendingTtsStop;
      if (!ownsJob(run, job)) return;
      try {
        const tts = await invoke<{ configured: boolean }>('start_tts_session', {
          ttsId: job.ttsId, originUserMessageId: job.origin,
        });
        if (!ownsJob(run, job)) { await stopTts(job); return; }
        job.ttsDone = !tts.configured;
        if (!tts.configured) show({ message: '朗读未配置，正在生成文字回复。' });
      } catch (error) {
        diagnostic(error);
        job.ttsDone = true;
        if (!ownsJob(run, job)) return;
        show({ message: '朗读暂不可用，正在生成文字回复。' });
      }
      const claimed = await invoke<{ accepted: boolean; text?: string; turnId?: string }>('claim_voice_transcript', {
        callId: run.id, offerId,
      });
      if (!claimed.accepted || !ownsJob(run, job) || !claimed.text?.trim()) {
        job.cancelled = true;
        job.ttsDone = true;
        await stopTts(job);
        if (ownsCall(run) && run.latestJob === job) show({ state: 'recognizing', message: '等待你说完后发送。' });
        return;
      }
      if (claimed.turnId) job.id = claimed.turnId;
      job.sent = true;
      await options.sendMessage(claimed.text.trim(), run.key, {
        messageId: job.origin, isCurrent: () => ownsJob(run, job),
      });
    })();
    job.sendTask = task;
    pendingSend = task;
    void task.catch(error => {
      diagnostic(error);
      if (ownsJob(run, job)) show({ state: 'error', message: '语音消息发送失败，请重试。' });
      // Sending failure does not discard already-submitted TTS.
    }).finally(async () => {
      job.settled = true;
      if (pendingSend === task) pendingSend = null;
      if (!job.sent || (job.activitySeen && job.attempts.size === 0)) releaseObserver(job);
      else job.completionTimer = setTimeout(() => releaseObserver(job), 5000);
      // Close the complete chat producer, including an empty invite-only first
      // round or failed persistence. Rust drains submitted speech naturally.
      if (!job.cancelled && !job.ttsDone) await command('finish_tts_chat', { ttsId: job.ttsId });
      forgetSettledJob(run, job);
    });
  };

  const handleService = (run: CallScope, event: VoiceServiceEvent) => {
    const input = run.inputs.get(event.sessionId);
    if (!ownsCall(run) || event.callId !== run.id) return;
    if (event.kind === 'transcript') acceptTranscript(run, event);
    else if (event.kind === 'barge-in') {
      // Confirmed new speech explicitly stops both generation and playback.
      const previousTurn = event.oldClaimedTurnId ?? event.turnId;
      for (const job of run.jobs) if (!previousTurn || job.id === previousTurn) void cancelJob(run, job);
      show({ state: 'listening', message: '正在聆听', partialText: '' });
    } else if (event.kind === 'asr-partial') {
      show({ state: 'recognizing', message: '正在识别', partialText: event.text?.slice(0, 2000) ?? '' });
    } else if (event.kind === 'error') {
      show({ state: 'error', message: event.message?.slice(0, 200) || '本次识别失败，请重新说话。', partialText: '' });
    } else if (event.kind === 'status' && event.stage === 'closed') {
      if (input) run.inputs.delete(input.id);
    } else if (event.kind === 'status' && (event.stage === 'asr' || event.stage === 'recognizing')) {
      show({ state: 'recognizing', message: '正在识别' });
    } else if (event.kind === 'status' && event.stage === 'listening'
      && (view.state === 'recognizing' || (view.state === 'listening' && !run.latestJob))) {
      show({ state: 'listening', message: '正在聆听', partialText: '' });
    }
  };

  const handleTts = (run: CallScope, event: TtsEvent) => {
    if (!ownsCall(run)) return;
    const job = Array.from(run.jobs).find(job => job.ttsId === event.ttsId);
    if (!job) return;
    if (event.kind === 'error' || event.kind === 'stopped') {
      job.ttsDone = true;
      if (run.latestJob === job) {
        if (event.kind === 'error') show({ state: 'error', message: event.message?.slice(0, 200) || '朗读失败，文字回复仍可继续。' });
        else show({ state: 'listening', message: event.stage === 'cancelled' ? '朗读已停止。' : '正在聆听' });
      }
      forgetSettledJob(run, job);
    } else if (run.latestJob === job && event.stage === 'speaking') {
      show({ state: 'speaking', message: '正在朗读' });
    }
  };

  const begin = () => {
    if (call) return call.id;
    const key = options.conversation();
    if (!key) throw new Error('voice_conversation_unavailable');
    const run: CallScope = {
      id: crypto.randomUUID(), key: { ...key }, ready: Promise.resolve(), beginSubmitted: false, hasConfiguredInput: false,
      unlisten: [], inputs: new Map(), jobs: new Set(), seenTranscripts: new Set(),
    };
    call = run;
    const previousEnd = pendingCallEnd;
    const subscribe = async <T>(event: string, handler: (event: T) => void) => {
      const unlisten = await listen<T>(event, ({ payload }) => handler(payload));
      if (call === run) run.unlisten.push(unlisten);
      else unlisten();
    };
    const platform = (async () => {
      if (previousEnd) await previousEnd;
      if (call !== run) return;
      run.beginSubmitted = true;
      await invoke<void>('plugin:vcp-mobile|begin_voice_call', { callId: run.id });
      if (call !== run) await command('plugin:vcp-mobile|end_voice_call', { callId: run.id });
    })();
    run.ready = Promise.all([
      platform,
      subscribe<VoiceServiceEvent>('vcp-voice-service', event => handleService(run, event)),
      subscribe<TtsEvent>('vcp-tts', event => handleTts(run, event)),
    ]).then(() => {}).catch(error => {
      // A failed reservation/listener installation cannot be reused as a
      // permanently rejected call. Retry creates a new precise call identity.
      if (call === run) stop();
      throw error;
    });
    void run.ready.catch(diagnostic);
    return run.id;
  };

  const closeInput = (id: string, receipt?: CaptureStopReceipt) => {
    const run = call;
    const input = run?.inputs.get(id);
    if (!run || !input || input.closing) return;
    input.closing = true;
    if (!input.startSubmitted) { run.inputs.delete(id); return; }
    const closing = invoke<void>('close_voice_input', { sessionId: id, ...receipt });
    pendingInputClose = closing;
    void closing.finally(() => {
      if (pendingInputClose === closing) pendingInputClose = null;
    }).catch(diagnostic);
  };

  const start = async (id: string): Promise<string> => {
    begin();
    const run = call!;
    const previousClose = pendingInputClose;
    const input: InputSession = { id, configured: false, startSubmitted: false, closing: false };
    run.inputs.set(id, input);
    try {
      await run.ready;
      if (previousClose) await previousClose;
      if (!ownsCall(run) || input.closing) return run.id;
      input.startSubmitted = true;
      const result = await invoke<{ configured: boolean }>('start_voice_service', { sessionId: id, callId: run.id });
      if (!ownsCall(run)) { await command('stop_voice_service', { sessionId: id }); return run.id; }
      input.configured = result.configured;
      run.hasConfiguredInput ||= result.configured;
      if (input.closing) await command('close_voice_input', { sessionId: id });
      else if (!['speaking', 'thinking', 'recognizing'].includes(view.state)) show({ state: result.configured ? 'listening' : 'local', partialText: '',
        message: result.configured ? '正在聆听' : '配置语音服务并重新开麦后，即可识别与朗读。' });
      return run.id;
    } catch (error) {
      run.inputs.delete(id);
      if (input.startSubmitted) await command('stop_voice_service', { sessionId: id });
      throw error;
    }
  };

  return { begin, start, closeInput, stop };
}

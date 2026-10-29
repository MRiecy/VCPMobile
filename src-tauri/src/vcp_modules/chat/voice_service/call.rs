//! The call owns admission and explicit interruption. Input, model and narration
//! keep independent cancellation lifetimes; UI events are only projections.
use super::{
    batch::{Began, CallInput, Offer},
    input, network, tts,
};
use crate::vcp_modules::{
    chat::voice_submission::prepare_voice_submission, chat_manager::ChatMessage,
    message_service::MessageWriteResultDto, topic_types::TopicKey,
    settings_manager::{read_config, SettingsState},
    vcp_client::ActiveGroupTurns,
};
use serde::Serialize;
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
};
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tauri_plugin_vcp_mobile::stream::{ForegroundLease, ForegroundTask};
use tauri_plugin_vcp_mobile::voice::VoiceVadConfig;
use tokio::sync::OnceCell;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceCallSnapshot {
    pub call_id: String,
    pub target: TopicKey,
    pub revision: u64,
    pub active: bool,
    pub mic_state: &'static str,
    pub mic_stop_reason: Option<String>,
    pub input_state: &'static str,
    pub reply_state: &'static str,
    pub tts_state: &'static str,
    pub partial: String,
    pub error: Option<String>,
    pub notice: Option<String>,
    pub input_session_id: Option<String>,
    pub origin_user_message_id: Option<String>,
    pub tts_id: Option<String>,
    pub last_persisted_message_id: Option<String>,
}

/// Type erasure only hides the runtime type of the native RAII resource, not a
/// second lifecycle or cancellation owner. A bounded batch/reply holds this Arc.
#[derive(Default)]
pub(super) struct WorkLease(Mutex<Option<Box<dyn Send + Sync>>>);

impl WorkLease {
    pub(super) fn acquire<R: Runtime>(app: &AppHandle<R>) -> Arc<Self> {
        let work = Arc::new(Self::default());
        let weak = Arc::downgrade(&work);
        let app = app.clone();
        tauri::async_runtime::spawn_blocking(move || {
            if weak.strong_count() == 0 {
                return;
            }
            match ForegroundLease::acquire(&app, ForegroundTask::Stream, "语音识别与回复") {
                Ok(lease) => {
                    if let Some(work) = weak.upgrade() {
                        if let Ok(mut slot) = work.0.lock() {
                            *slot = Some(Box::new(lease));
                        }
                    }
                }
                Err(error) => log::warn!("[VoiceCall] foreground resource unavailable: {error}"),
            }
        });
        work
    }
}

struct Reply {
    origin: String,
    tts_id: String,
    cancel: CancellationToken,
    finished: AtomicBool,
    done: tokio::sync::Notify,
}
struct SubmissionCompletion {
    reply: Arc<Reply>,
    previous: Option<Arc<Reply>>,
}
impl Drop for SubmissionCompletion {
    fn drop(&mut self) {
        let reply = self.reply.clone();
        if let Some(previous) = self
            .previous
            .take()
            .filter(|previous| !previous.finished.load(Ordering::Acquire))
        {
            // A cancelled queued submission stops waiting immediately, but its
            // successor must not jump past the still-settling real owner.
            tauri::async_runtime::spawn(async move {
                previous.released().await;
                reply.mark_finished();
            });
        } else {
            reply.mark_finished();
        }
    }
}
impl Reply {
    fn mark_finished(&self) {
        self.finished.store(true, Ordering::Release);
        self.done.notify_waiters();
    }
    async fn wait_previous(&self, previous: Option<&Reply>) -> bool {
        if let Some(previous) = previous {
            tokio::select! {
                biased;
                _ = self.cancel.cancelled() => return false,
                _ = previous.released() => {}
            }
        }
        !self.cancel.is_cancelled()
    }
    async fn released(&self) {
        loop {
            let notified = self.done.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.finished.load(Ordering::Acquire) {
                return;
            }
            notified.await;
        }
    }
}
struct Pending {
    offer_id: String,
    reply: Arc<Reply>,
}
struct CallInner {
    generation: u64,
    snapshot: VoiceCallSnapshot,
    pending: Option<Pending>,
    reply: Option<Arc<Reply>>,
    last_submission: Option<Arc<Reply>>,
    work: Option<Arc<WorkLease>>,
}
impl CallInner {
    fn begin_speech(&mut self, input: &CallInput) -> Result<(Began, Vec<String>), String> {
        if !self.snapshot.active {
            return Err(network::CANCELLED.into());
        }
        let began = input.begin()?;
        let mut stop = Vec::new();
        if let Some(pending) = self.pending.take() {
            pending.reply.cancel.cancel();
            stop.push(pending.reply.tts_id.clone());
        }
        if began.barge.is_some() {
            if let Some(reply) = &self.reply {
                reply.cancel.cancel();
                stop.push(reply.tts_id.clone());
                self.snapshot.reply_state = "cancelled";
            }
        }
        self.snapshot.input_state = "recording";
        self.snapshot.error = None;
        self.snapshot.notice = None;
        Ok((began, stop))
    }

    fn queue_offer(&mut self, input: &CallInput, offer: &Offer, reply: &Arc<Reply>) -> bool {
        if !self.snapshot.active || !input.has_offer(&offer.offer_id)
            || self.pending.as_ref().is_some_and(|pending| pending.offer_id == offer.offer_id)
        {
            return false;
        }
        if let Some(old) = self.pending.take() {
            old.reply.cancel.cancel();
        }
        self.pending = Some(Pending { offer_id: offer.offer_id.clone(), reply: reply.clone() });
        self.snapshot.input_state = "submitting";
        self.snapshot.partial = offer.text.clone();
        self.snapshot.error = None;
        self.snapshot.notice = None;
        true
    }

    fn submission_failed(&mut self, input: &CallInput, reply: &Arc<Reply>, error: String) -> bool {
        reply.cancel.cancel();
        if !self.pending.as_ref().is_some_and(|pending| Arc::ptr_eq(&pending.reply, reply)) {
            return false;
        }
        if let Some(pending) = self.pending.take() {
            input.discard_offer(&pending.offer_id);
        }
        self.work = None;
        self.snapshot.input_state = "error";
        self.snapshot.partial.clear();
        self.snapshot.error = Some(error);
        true
    }

    fn admit(&mut self, input: &CallInput, offer: &Offer, reply: &Arc<Reply>) -> bool {
        let current = self.pending.as_ref().is_some_and(|pending| {
            pending.offer_id == offer.offer_id && Arc::ptr_eq(&pending.reply, reply)
        });
        if !current
            || !self.snapshot.active
            || reply.cancel.is_cancelled()
            || !input.claim(&offer.offer_id).unwrap_or(false)
        {
            return false;
        }
        self.pending = None;
        self.work = None;
        self.reply = Some(reply.clone());
        self.snapshot.input_state = "idle";
        self.snapshot.partial.clear();
        self.snapshot.origin_user_message_id = Some(reply.origin.clone());
        self.snapshot.tts_id = Some(reply.tts_id.clone());
        self.snapshot.reply_state = "generating";
        true
    }
}
struct Call {
    id: String,
    input: Arc<CallInput>,
    inner: Mutex<CallInner>,
    vad_config: OnceCell<VoiceVadConfig>,
}

impl Call {
    fn input_vad_config(&self) -> Result<VoiceVadConfig, String> {
        let inner = self.inner.lock().map_err(|error| error.to_string())?;
        if !inner.snapshot.active || self.input.cancel.is_cancelled() {
            return Err(network::CANCELLED.into());
        }
        self.vad_config.get().copied().ok_or_else(|| "语音通话仍在准备中。".into())
    }
}
#[derive(Default)]
struct Registry {
    current: Option<Arc<Call>>,
    retired: VecDeque<String>,
}
#[derive(Default)]
pub struct VoiceCallState {
    registry: Mutex<Registry>,
    revision: AtomicU64,
    generation: AtomicU64,
}

fn call<R: Runtime>(app: &AppHandle<R>, id: &str) -> Option<Arc<Call>> {
    app.try_state::<VoiceCallState>()?
        .registry
        .lock()
        .ok()?
        .current
        .as_ref()
        .filter(|call| call.id == id)
        .cloned()
}

fn publish<R: Runtime>(app: &AppHandle<R>, inner: &mut CallInner) {
    let Some(state) = app.try_state::<VoiceCallState>() else {
        return;
    };
    if inner.generation != state.generation.load(Ordering::Acquire) {
        return;
    }
    inner.snapshot.revision = state.revision.fetch_add(1, Ordering::Relaxed) + 1;
    let _ = app.emit("vcp-voice-call", &inner.snapshot);
}

fn reserve_conversation<R: Runtime>(
    app: AppHandle<R>,
    call_id: String,
    target: TopicKey,
) -> Result<VoiceCallSnapshot, String> {
    if call_id.is_empty() || call_id.len() > 128 || !target.is_valid() {
        return Err("语音通话目标无效。".into());
    }
    let state = app.state::<VoiceCallState>();
    let mut registry = state.registry.lock().map_err(|error| error.to_string())?;
    if registry.retired.contains(&call_id) {
        return Err(network::CANCELLED.into());
    }
    if let Some(current) = &registry.current {
        let inner = current.inner.lock().map_err(|error| error.to_string())?;
        if inner.snapshot.active {
            return if current.id == call_id && inner.snapshot.target == target {
                Ok(inner.snapshot.clone())
            } else {
                Err("已有语音通话正在进行。".into())
            };
        }
    }
    let input = Arc::new(CallInput::new(call_id.clone()));
    input::register_call(&app, input.clone())?;
    let mut inner = CallInner {
        generation: state.generation.fetch_add(1, Ordering::AcqRel) + 1,
        snapshot: VoiceCallSnapshot {
            call_id: call_id.clone(),
            target,
            revision: 0,
            active: true,
            mic_state: "closed",
            mic_stop_reason: None,
            input_state: "idle",
            reply_state: "idle",
            tts_state: "idle",
            partial: String::new(),
            error: None,
            notice: None,
            input_session_id: None,
            origin_user_message_id: None,
            tts_id: None,
            last_persisted_message_id: None,
        },
        pending: None,
        reply: None,
        last_submission: None,
        work: None,
    };
    publish(&app, &mut inner);
    let snapshot = inner.snapshot.clone();
    registry.current = Some(Arc::new(Call {
        id: call_id,
        input,
        inner: Mutex::new(inner),
        vad_config: OnceCell::new(),
    }));
    Ok(snapshot)
}

fn retire_conversation<R: Runtime>(
    app: &AppHandle<R>,
    call_id: &str,
) -> Result<Option<Arc<Call>>, String> {
    let state = app.state::<VoiceCallState>();
    let mut registry = state.registry.lock().map_err(|error| error.to_string())?;
    // End can run before begin is polled. Record it under the same admission
    // lock as reserve, even when no matching call has been registered yet.
    if !registry.retired.iter().any(|id| id == call_id) {
        if registry.retired.len() == 128 {
            registry.retired.pop_front();
        }
        registry.retired.push_back(call_id.into());
    }
    Ok(registry.current.as_ref().filter(|call| call.id == call_id).cloned())
}

#[tauri::command]
pub async fn begin_voice_conversation(
    app: AppHandle,
    call_id: String,
    target: TopicKey,
) -> Result<VoiceCallSnapshot, String> {
    reserve_conversation(app.clone(), call_id.clone(), target)?;
    let current = call(&app, &call_id).ok_or_else(|| network::CANCELLED.to_string())?;
    let result = async {
        // Reserve before the first await so hangup can revoke an initializing
        // call. Repeated begin/microphone intents keep the same VAD snapshot.
        tts::stop_bubble_narration(&app).await?;
        current.vad_config.get_or_try_init(|| async {
            let config = read_config(app.clone(), app.state::<SettingsState>()).await?.voice_vad_config();
            config.validate()?;
            Ok::<_, String>(config)
        }).await?;
        validate_input(&app, &call_id)?;
        tauri_plugin_vcp_mobile::voice::begin_voice_call(app.clone(), call_id.clone()).await
    }.await;
    if let Err(error) = result {
        let _ = end_voice_conversation(app.clone(), call_id.clone()).await;
        if let Some(call) = call(&app, &call_id) {
            if let Ok(mut inner) = call.inner.lock() {
                inner.snapshot.error = Some(error.clone());
                publish(&app, &mut inner);
            }
        }
        return Err(error);
    }
    // Hangup may win while the native call reservation is in flight.
    if let Err(error) = validate_input(&app, &call_id) {
        let _ = tauri_plugin_vcp_mobile::voice::end_voice_call(app, call_id).await;
        return Err(error);
    }
    get_voice_conversation_snapshot(app)?.ok_or_else(|| network::CANCELLED.into())
}

pub(super) fn is_active<R: Runtime>(app: &AppHandle<R>) -> Result<bool, String> {
    let Some(state) = app.try_state::<VoiceCallState>() else { return Ok(false); };
    let current = state.registry.lock().map_err(|e| e.to_string())?.current.clone();
    current.map(|call| call.inner.lock().map(|inner| inner.snapshot.active).map_err(|e| e.to_string()))
        .transpose().map(|active| active.unwrap_or(false))
}

#[tauri::command]
pub fn get_voice_conversation_snapshot(
    app: AppHandle,
) -> Result<Option<VoiceCallSnapshot>, String> {
    let state = app.state::<VoiceCallState>();
    let current = state
        .registry
        .lock()
        .map_err(|error| error.to_string())?
        .current
        .clone();
    current
        .map(|call| {
            call.inner
                .lock()
                .map(|inner| inner.snapshot.clone())
                .map_err(|error| error.to_string())
        })
        .transpose()
}

#[tauri::command]
pub async fn end_voice_conversation(app: AppHandle, call_id: String) -> Result<(), String> {
    let Some(call) = retire_conversation(&app, &call_id)? else {
        return Ok(());
    };
    let ids = {
        let mut inner = call.inner.lock().map_err(|error| error.to_string())?;
        call.input.cancel.cancel();
        inner.snapshot.active = false;
        inner.snapshot.mic_state = "closed";
        inner.snapshot.input_state = "idle";
        inner.snapshot.partial.clear();
        inner.work = None;
        let mut ids = Vec::new();
        if let Some(pending) = inner.pending.take() {
            pending.reply.cancel.cancel();
            ids.push(pending.reply.tts_id.clone());
        }
        if let Some(reply) = inner.reply.take() {
            reply.cancel.cancel();
            ids.push(reply.tts_id.clone());
            inner.snapshot.reply_state = "cancelled";
        }
        publish(&app, &mut inner);
        ids
    };
    for id in &ids {
        tts::request_stop(&app, id);
    }
    // Revoke input work immediately. The native call route stays alive until
    // playback stop acknowledges its physical fade/cleanup, then capture and
    // the shared audio session can be torn down together.
    let mut result = input::stop_voice_call_inputs(app.clone(), call_id.clone()).await;
    for id in ids {
        if let Err(error) = tts::stop_tts(app.clone(), id).await {
            result = Err(error);
        }
    }
    let route_result = tauri_plugin_vcp_mobile::voice::end_voice_call(app.clone(), call_id).await;
    // Always finish native cleanup; retain the existing TTS > input > route
    // error precedence when more than one owner reports a failure.
    if result.is_ok() {
        result = route_result;
    }
    if let Err(error) = &result {
        if let Ok(mut inner) = call.inner.lock() {
            inner.snapshot.error = Some(error.clone());
            publish(&app, &mut inner);
        }
    }
    result
}

/// Must precede any native capture/ASR setup, including retries after mute.
pub(super) fn validate_input<R: Runtime>(app: &AppHandle<R>, id: &str) -> Result<(), String> {
    input_vad_config(app, id).map(|_| ())
}

pub(super) fn input_vad_config<R: Runtime>(app: &AppHandle<R>, id: &str) -> Result<VoiceVadConfig, String> {
    let call = call(app, id).ok_or_else(|| "语音通话已结束。".to_string())?;
    call.input_vad_config()
}

pub(super) fn input_work(app: &AppHandle, id: &str) -> Option<Arc<WorkLease>> {
    call(app, id)?.inner.lock().ok()?.work.clone()
}

pub(super) fn input_starting<R: Runtime>(app: &AppHandle<R>, id: &str, session: &str, cancelled: &CancellationToken) {
    let Some(call) = call(app, id) else { return; };
    let Ok(mut inner) = call.inner.lock() else { return; };
    // Cancellation is checked while holding the projection lock, so a close
    // accepted before this publication cannot be overwritten by late readiness.
    if !inner.snapshot.active || cancelled.is_cancelled() { return; }
    inner.snapshot.input_session_id = Some(session.into());
    inner.snapshot.mic_state = "starting";
    inner.snapshot.mic_stop_reason = None;
    inner.snapshot.error = None;
    publish(app, &mut inner);
}

pub(super) fn input_stop_failed<R: Runtime>(app: &AppHandle<R>, id: &str, session: &str, error: &str) {
    let Some(call) = call(app, id) else { return; };
    let Ok(mut inner) = call.inner.lock() else { return; };
    if !inner.snapshot.active || inner.snapshot.input_session_id.as_deref() != Some(session) { return; }
    inner.snapshot.mic_state = "stopping";
    inner.snapshot.mic_stop_reason = Some(error.into());
    publish(app, &mut inner);
}

pub(super) fn input_status<R: Runtime>(
    app: &AppHandle<R>,
    id: &str,
    session: &str,
    stage: &str,
    text: Option<&str>,
    error: Option<&str>,
) {
    let Some(call) = call(app, id) else {
        return;
    };
    let Ok(mut inner) = call.inner.lock() else {
        return;
    };
    if !inner.snapshot.active {
        return;
    }
    match stage {
        "ready" | "local-only" => {
            if inner.snapshot.input_session_id.as_deref() != Some(session)
                || inner.snapshot.mic_state != "starting" { return; }
            inner.snapshot.mic_stop_reason = None;
            inner.snapshot.input_state = "idle";
            inner.snapshot.notice = (stage == "local-only")
                .then(|| "尚未配置语音 API Key，当前仅显示本地麦克风状态。".into());
        }
        "listening" => {
            if inner.snapshot.input_session_id.as_deref() != Some(session)
                || inner.snapshot.mic_state != "starting"
            {
                return;
            }
            inner.snapshot.mic_state = "listening";
            inner.snapshot.mic_stop_reason = None;
        }
        "stopping" => {
            if inner.snapshot.input_session_id.as_deref() != Some(session) { return; }
            inner.snapshot.mic_state = "stopping";
        }
        "closed" => {
            if inner.snapshot.input_session_id.as_deref() == Some(session) {
                inner.snapshot.mic_state = "closed";
                inner.snapshot.input_session_id = None;
            }
        }
        "recording" => inner.snapshot.input_state = "recording",
        "asr" | "recognizing" | "transcribing" => inner.snapshot.input_state = "transcribing",
        "empty" => {
            inner.snapshot.input_state = "idle";
            inner.snapshot.partial.clear();
            inner.work = None;
        }
        _ => {}
    }
    if let Some(text) = text {
        inner.snapshot.partial = text.into();
    }
    if let Some(error) = error {
        inner.snapshot.error = Some(error.into());
        inner.snapshot.input_state = "error";
        // Failed ASR retains text for a spoken retry, but no active background work.
        inner.work = None;
    }
    publish(app, &mut inner);
}

pub(super) fn input_stopped<R: Runtime>(app: &AppHandle<R>, id: &str, session: &str, reason: &str, failed: bool) {
    let Some(call) = call(app, id) else {
        return;
    };
    let Ok(mut inner) = call.inner.lock() else {
        return;
    };
    // A sealed older input may finish after the user has opened a new one.
    if inner
        .snapshot
        .input_session_id
        .as_deref()
        .is_some_and(|current| current != session)
    {
        return;
    }
    if failed
        && inner.snapshot.input_session_id.is_none()
        && inner.snapshot.mic_stop_reason.is_some()
    {
        // The native terminal can precede a failed start's IPC response. Keep
        // that concrete reason (in particular ordinary background closure).
        return;
    }
    inner.snapshot.mic_state = "closed";
    inner.snapshot.input_session_id = None;
    inner.snapshot.mic_stop_reason = Some(reason.into());
    if failed && !reason.contains("background") && !reason.starts_with("cancelled") {
        inner.snapshot.error = Some(reason.into());
    }
    publish(app, &mut inner);
}

pub(super) fn interrupt_reply(app: &AppHandle, id: &str) {
    let Some(call) = call(app, id) else {
        return;
    };
    let tts = {
        let Ok(mut inner) = call.inner.lock() else {
            return;
        };
        let Some(reply) = &inner.reply else {
            return;
        };
        reply.cancel.cancel();
        let tts = reply.tts_id.clone();
        inner.snapshot.reply_state = "cancelled";
        publish(app, &mut inner);
        tts
    };
    tts::request_stop(app, &tts);
}

pub(super) fn begin_speech<R: Runtime>(app: &AppHandle<R>, input: &Arc<CallInput>) -> Result<Began, String> {
    let call = call(app, &input.id).ok_or_else(|| network::CANCELLED.to_string())?;
    let (began, stop, target) = {
        let mut inner = call.inner.lock().map_err(|error| error.to_string())?;
        let (began, stop) = inner.begin_speech(input)?;
        inner.work.get_or_insert_with(|| WorkLease::acquire(app));
        publish(app, &mut inner);
        (began, stop, inner.snapshot.target.clone())
    };
    for id in stop {
        tts::request_stop(app, &id);
    }
    // Cancellation may publish group events; never call it under the call lock.
    if target.owner_type == "group" {
        app.state::<ActiveGroupTurns>().cancel(&target);
    }
    Ok(began)
}

pub(super) fn offer(app: &AppHandle, offer: Offer) {
    let Some(call) = call(app, &offer.call_id) else {
        return;
    };
    let reply = Arc::new(Reply {
        origin: format!("msg_voice_{}", uuid::Uuid::new_v4()),
        tts_id: uuid::Uuid::new_v4().to_string(),
        cancel: CancellationToken::new(),
        finished: AtomicBool::new(false),
        done: tokio::sync::Notify::new(),
    });
    let (target, work, previous_submission, previous_tts) = {
        let Ok(mut inner) = call.inner.lock() else {
            return;
        };
        if !inner.queue_offer(&call.input, &offer, &reply) {
            return;
        }
        inner.work.get_or_insert_with(|| WorkLease::acquire(app));
        let previous = inner.last_submission.replace(reply.clone());
        publish(app, &mut inner);
        (
            inner.snapshot.target.clone(),
            inner.work.clone(),
            previous,
            inner.reply.as_ref().map(|reply| reply.tts_id.clone()),
        )
    };
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let _completion = SubmissionCompletion {
            reply: reply.clone(),
            previous: previous_submission.clone(),
        };
        // Includes cancelled preparations: a successor may only register after
        // its predecessor's real owner has dropped, never after a timer.
        if !reply.wait_previous(previous_submission.as_deref()).await {
            return;
        }
        let narration = match tts::prepare_narration(&app, &reply.cancel).await {
            Ok(prepared) => prepared,
            Err(error) => {
                if !reply.cancel.is_cancelled() { submission_failed(&app, &call, &reply, error); }
                return;
            }
        };
        let prepared = match prepare_voice_submission(
            &app,
            target,
            reply.origin.clone(),
            offer.text.clone(),
            reply.cancel.clone(),
            narration.policy.clone(),
        ).await {
            Ok(prepared) => prepared,
            Err(error) => {
                submission_failed(&app, &call, &reply, error);
                return;
            }
        };
        if let Some(id) = previous_tts {
            let _ = tts::stop_tts(app.clone(), id).await;
        }
        let tts_result = tts::start_prepared_narration(
            app.clone(),
            reply.tts_id.clone(),
            reply.origin.clone(),
            narration,
        )
        .await;
        if let Some(work) = &work {
            tts::attach_work(&app, &reply.tts_id, work.clone());
        }
        let accepted = {
            let Ok(mut inner) = call.inner.lock() else {
                return;
            };
            if !inner.admit(&call.input, &offer, &reply) {
                false
            } else {
                inner.snapshot.tts_state =
                    if tts_result.as_ref().is_ok_and(|result| result.configured) {
                        "ready"
                    } else {
                        "idle"
                    };
                if let Err(error) = &tts_result {
                    inner.snapshot.error = Some(error.clone());
                    inner.snapshot.tts_state = "error";
                }
                publish(&app, &mut inner);
                true
            }
        };
        if !accepted {
            reply.cancel.cancel();
            let _ = tts::stop_tts(app, reply.tts_id.clone()).await;
            return;
        }
        let result = prepared
            .execute(|message, saved| message_persisted(&app, &call, message, saved))
            .await;
        // The producer closes even for an empty group turn and without any UI.
        let _ = tts::finish_tts_chat(app.clone(), reply.tts_id.clone()).await;
        if let Ok(mut inner) = call.inner.lock() {
            if inner
                .reply
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, &reply))
            {
                inner.snapshot.reply_state = if reply.cancel.is_cancelled() {
                    "cancelled"
                } else {
                    match result
                        .as_ref()
                        .ok()
                        .and_then(|value| value["status"].as_str())
                    {
                        Some("recovery_pending") => "recovering",
                        Some("stopping") => "stopping",
                        Some("cancelled") => "cancelled",
                        _ if result.is_ok() => "finished",
                        _ => "error",
                    }
                };
                inner.snapshot.notice =
                    result
                        .as_ref()
                        .ok()
                        .and_then(|value| match value["status"].as_str() {
                            Some("no_ai_response") => Some(
                                match value["reason"].as_str() {
                                    Some("invite_only") => {
                                        "语音文本已保存；当前群聊为邀请发言模式。"
                                    }
                                    Some("no_speakers") => "语音文本已保存；本轮没有发言成员。",
                                    _ => "语音文本已保存；当前群聊模式尚未提供自动回复。",
                                }
                                .into(),
                            ),
                            Some("recovery_pending") => {
                                Some("回复尚未完成保存或连接恢复，返回前台后可继续接管。".into())
                            }
                            Some("stopping") => Some("正在等待这次回复停止。".into()),
                            _ => None,
                        });
                if let Err(error) = result {
                    inner.snapshot.error = Some(error);
                }
                publish(&app, &mut inner);
            }
        }
        drop(work); // narration retains the same lease until actual native cleanup
    });
}

fn submission_failed(app: &AppHandle, call: &Call, reply: &Arc<Reply>, error: String) {
    if let Ok(mut inner) = call.inner.lock() {
        if inner.submission_failed(&call.input, reply, error) {
            publish(app, &mut inner);
        }
    }
}

fn message_persisted(
    app: &AppHandle,
    call: &Call,
    mut message: ChatMessage,
    saved: MessageWriteResultDto,
) {
    message.blocks = serde_json::to_value(saved.blocks).ok();
    if let Ok(mut inner) = call.inner.lock() {
        inner.snapshot.last_persisted_message_id = Some(message.id.clone());
        publish(app, &mut inner);
        let _ = app.emit("vcp-voice-message-persisted", serde_json::json!({
            "callId": call.id, "target": inner.snapshot.target, "revision": inner.snapshot.revision,
            "message": message, "topicUpdatedAt": saved.topic_updated_at,
        }));
    }
}

pub(super) fn tts_status<R: Runtime>(
    app: &AppHandle<R>,
    id: &str,
    stage: &'static str,
    error: Option<&str>,
) {
    let Some(state) = app.try_state::<VoiceCallState>() else {
        return;
    };
    let current = state
        .registry
        .lock()
        .ok()
        .and_then(|registry| registry.current.clone());
    let Some(call) = current else {
        return;
    };
    if let Ok(mut inner) = call.inner.lock() {
        if inner.snapshot.tts_id.as_deref() != Some(id) {
            return;
        }
        inner.snapshot.tts_state = if error.is_some() { "error" } else { stage };
        if let Some(error) = error {
            inner.snapshot.error = Some(error.into());
        }
        publish(app, &mut inner);
    };
}

#[cfg(test)]
mod tests {
    use super::super::batch::BatchCompletion;
    use super::*;

    fn inner() -> CallInner {
        CallInner {
            generation: 1,
            snapshot: VoiceCallSnapshot {
                call_id: "call".into(),
                target: TopicKey::new("agent", "a", "t"),
                revision: 0,
                active: true,
                mic_state: "closed",
                mic_stop_reason: None,
                input_state: "idle",
                reply_state: "idle",
                tts_state: "idle",
                partial: String::new(),
                error: None,
                notice: None,
                input_session_id: None,
                origin_user_message_id: None,
                tts_id: None,
                last_persisted_message_id: None,
            },
            pending: None,
            reply: None,
            last_submission: None,
            work: None,
        }
    }
    fn reply(id: &str) -> Arc<Reply> {
        Arc::new(Reply {
            origin: id.into(),
            tts_id: format!("tts-{id}"),
            cancel: CancellationToken::new(),
            finished: AtomicBool::new(false),
            done: tokio::sync::Notify::new(),
        })
    }
    fn offer(input: &CallInput, began: &Began, text: &str) -> Offer {
        match input
            .complete(&began.batch_id, began.sequence, text.into())
            .unwrap()
        {
            BatchCompletion::Offered(offer) => offer,
            _ => panic!("recognition should offer a complete batch"),
        }
    }
    fn pending(inner: &mut CallInner, offer: &Offer, reply: &Arc<Reply>) {
        inner.pending = Some(Pending {
            offer_id: offer.offer_id.clone(),
            reply: reply.clone(),
        });
    }

    #[test]
    fn input_requires_a_ready_call_and_rejects_hangup() {
        let call = Call {
            id: "call".into(),
            input: Arc::new(CallInput::new("call".into())),
            inner: Mutex::new(inner()),
            vad_config: OnceCell::new(),
        };
        assert!(call.input_vad_config().is_err());
        let mut settings = crate::vcp_modules::settings_manager::Settings::default();
        settings.voice.vad.end_silence_ms = 777;
        call.vad_config.set(settings.voice_vad_config()).unwrap();
        assert_eq!(call.input_vad_config().unwrap().end_silence_ms, 777);
        call.input.cancel.cancel();
        assert!(call.input_vad_config().is_err());
    }

    #[test]
    fn hangup_before_registration_rejects_late_begin_without_blocking_the_next_call() {
        let app = tauri::test::mock_app();
        app.manage(VoiceCallState::default());
        app.manage(super::super::VoiceServiceState::default());
        let target = TopicKey::new("agent", "a", "t");

        // This is the synchronous admission step used by the end command.
        assert!(retire_conversation(app.handle(), "cancelled").unwrap().is_none());
        let late = reserve_conversation(app.handle().clone(), "cancelled".into(), target.clone());
        assert!(matches!(late, Err(error) if error == network::CANCELLED));

        let next = reserve_conversation(app.handle().clone(), "next".into(), target.clone()).unwrap();
        assert!(next.active);
        assert!(retire_conversation(app.handle(), "cancelled").unwrap().is_none());
        let current = call(app.handle(), "next").unwrap();
        assert!(!current.input.cancel.is_cancelled());
        assert!(current.inner.lock().unwrap().snapshot.active);
        assert!(reserve_conversation(app.handle().clone(), "next".into(), target).is_ok());
        assert_eq!(app.state::<VoiceCallState>().registry.lock().unwrap().retired.len(), 1);
    }

    #[test]
    fn capture_snapshot_survives_view_loss_and_late_completions_without_ending_the_call() {
        let app = tauri::test::mock_app();
        app.manage(VoiceCallState::default());
        app.manage(super::super::VoiceServiceState::default());
        reserve_conversation(app.handle().clone(), "call".into(), TopicKey::new("agent", "a", "t")).unwrap();
        let current = call(app.handle(), "call").unwrap();
        let snapshot = || current.inner.lock().unwrap().snapshot.clone();
        let capture = CancellationToken::new();
        input_starting(app.handle(), "call", "old-input", &capture);
        assert_eq!(snapshot().mic_state, "starting");
        input_status(app.handle(), "call", "old-input", "ready", None, None);
        capture.cancel();
        input_status(app.handle(), "call", "old-input", "stopping", None, None);
        input_starting(app.handle(), "call", "old-input", &capture);
        input_status(app.handle(), "call", "old-input", "listening", None, None);
        assert_eq!(snapshot().mic_state, "stopping");
        input_stop_failed(app.handle(), "call", "old-input", "native_stop_failed");
        assert_eq!(snapshot().mic_stop_reason.as_deref(), Some("native_stop_failed"));
        assert_eq!(snapshot().input_session_id.as_deref(), Some("old-input"));
        assert!(snapshot().active);
        assert!(!current.input.cancel.is_cancelled());
        input_stopped(app.handle(), "call", "old-input", "document_replaced", false);
        assert_eq!(snapshot().mic_state, "closed");
        input_starting(app.handle(), "call", "new-input", &CancellationToken::new());
        input_status(app.handle(), "call", "new-input", "listening", None, None);
        let before = snapshot().revision;
        input_stopped(app.handle(), "call", "old-input", "late_old_error", true);
        input_stop_failed(app.handle(), "call", "old-input", "late_old_stop");
        assert_eq!(snapshot().revision, before);
        assert_eq!(snapshot().mic_state, "listening");
        assert_eq!(snapshot().call_id, "call");
    }

    #[test]
    fn supplementary_speech_revokes_prepared_admission_and_preserves_recording_order() {
        let input = CallInput::new("call".into());
        let mut inner = inner();
        let first = inner.begin_speech(&input).unwrap().0;
        let old = offer(&input, &first, "前一句");
        let prepared = reply("old");
        pending(&mut inner, &old, &prepared);
        let (more, stops) = inner.begin_speech(&input).unwrap();
        assert!(prepared.cancel.is_cancelled());
        assert_eq!(stops, vec!["tts-old"]);
        assert!(!inner.admit(&input, &old, &prepared));
        let merged = offer(&input, &more, "补充");
        assert_eq!(merged.text, "前一句 补充");
        let next = reply("new");
        pending(&mut inner, &merged, &next);
        assert!(inner.admit(&input, &merged, &next));
        assert!(!inner.admit(&input, &merged, &next));
    }

    #[test]
    fn failed_preparation_ends_a_full_batch_and_next_speech_starts_fresh() {
        let input = CallInput::new("call".into());
        let mut inner = inner();
        let parts: Vec<_> = (0..super::super::batch::MAX_BATCH_PARTS)
            .map(|_| inner.begin_speech(&input).unwrap().0).collect();
        for part in &parts[..127] {
            assert!(matches!(input.complete(&part.batch_id, part.sequence, "已识别".into()).unwrap(),
                BatchCompletion::Pending));
        }
        let ready = offer(&input, &parts[127], "末句");
        let failed = reply("failed");
        assert!(inner.begin_speech(&input).is_err());
        assert!(inner.queue_offer(&input, &ready, &failed));
        assert!(inner.submission_failed(&input, &failed, "配置读取失败".into()));
        assert_eq!(inner.snapshot.input_state, "error");
        assert_eq!(inner.snapshot.error.as_deref(), Some("配置读取失败"));
        assert!(inner.snapshot.partial.is_empty());
        assert!(!input.has_offer(&ready.offer_id));
        assert!(!inner.admit(&input, &ready, &failed));
        let next = inner.begin_speech(&input).unwrap().0;
        assert_ne!(next.batch_id, parts[127].batch_id);
        let fresh = offer(&input, &next, "新的输入");
        assert_eq!(fresh.text, "新的输入");
        let prepared = reply("fresh");
        assert!(inner.queue_offer(&input, &fresh, &prepared));
        assert!(inner.admit(&input, &fresh, &prepared));
        assert!(!inner.admit(&input, &fresh, &prepared));
    }

    #[test]
    fn stale_preparation_failure_cannot_discard_new_speech() {
        let input = CallInput::new("call".into());
        let mut inner = inner();
        let first = inner.begin_speech(&input).unwrap().0;
        let ready = offer(&input, &first, "第一句");
        let failed = reply("failed");
        assert!(inner.queue_offer(&input, &ready, &failed));
        let more = inner.begin_speech(&input).unwrap().0;
        let merged = offer(&input, &more, "第二句");
        assert_eq!(merged.text, "第一句 第二句");
        let pending = reply("pending");
        assert!(inner.queue_offer(&input, &merged, &pending));
        assert!(!inner.submission_failed(&input, &failed, "过期错误".into()));
        assert!(input.has_offer(&merged.offer_id));
        assert!(inner.snapshot.error.is_none());
        assert!(inner.admit(&input, &merged, &pending));
    }

    #[tokio::test]
    async fn speech_cancels_only_its_target_group() {
        let app = tauri::test::mock_app();
        app.manage(VoiceCallState::default());
        app.manage(super::super::VoiceServiceState::default());
        app.manage(ActiveGroupTurns::default());
        let target = TopicKey::new("group", "g", "t");
        let old = app.state::<ActiveGroupTurns>().try_acquire(target.clone()).unwrap();
        let other = app.state::<ActiveGroupTurns>().try_acquire(TopicKey::new("group", "g", "other")).unwrap();
        reserve_conversation(app.handle().clone(), "call".into(), target).unwrap();
        let current = call(app.handle(), "call").unwrap();
        begin_speech(app.handle(), &current.input).unwrap();
        assert!(old.is_cancelled());
        assert!(!other.is_cancelled());
    }

    #[test]
    fn sealed_input_admits_without_ui_and_next_speech_cancels_the_exact_reply() {
        let input = CallInput::new("call".into());
        let mut inner = inner();
        let recorded = inner.begin_speech(&input).unwrap().0;
        // Same production batch transition used by seal after native background.
        input.end_speech(&recorded.batch_id, recorded.sequence);
        inner.snapshot.mic_state = "closed";
        let ready = offer(&input, &recorded, "后台继续发送");
        let actual = reply("actual");
        pending(&mut inner, &ready, &actual);
        assert!(inner.admit(&input, &ready, &actual));
        assert!(!actual.cancel.is_cancelled());
        let (_, tts_to_stop) = inner.begin_speech(&input).unwrap();
        assert!(actual.cancel.is_cancelled());
        assert_eq!(tts_to_stop, vec!["tts-actual"]);
    }

    #[test]
    fn hangup_rejects_prepared_offer_before_any_execution() {
        let input = CallInput::new("call".into());
        let mut inner = inner();
        let recorded = inner.begin_speech(&input).unwrap().0;
        let ready = offer(&input, &recorded, "未发送");
        let prepared = reply("pending");
        pending(&mut inner, &ready, &prepared);
        input.cancel.cancel();
        inner.snapshot.active = false;
        assert!(!inner.admit(&input, &ready, &prepared));
        assert!(inner.reply.is_none());
    }

    #[tokio::test]
    async fn cancelled_waiter_exits_but_cannot_release_a_still_settling_predecessor() {
        let previous = reply("previous-group");
        let queued = reply("queued");
        queued.cancel.cancel();
        assert!(!queued.wait_previous(Some(&previous)).await);
        drop(SubmissionCompletion {
            reply: queued.clone(),
            previous: Some(previous.clone()),
        });
        assert!(!queued.finished.load(Ordering::Acquire));
        previous.mark_finished();
        tokio::time::timeout(std::time::Duration::from_secs(2), queued.released())
            .await
            .unwrap();
        let next = reply("next");
        assert!(next.wait_previous(Some(&queued)).await);
    }
}

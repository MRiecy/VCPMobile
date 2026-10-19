//! Capture/ASR ownership only. Closing input drains accepted audio; explicit
//! cancellation discards it. Neither path owns model generation or TTS.

use super::{
    batch::{BatchCompletion, CallInput, ClaimedTranscript},
    emit, network,
    segmentation::{CaptureWindow, SegmentWindow},
    workers, VoiceServiceConfig, VoiceServiceEvent, VoiceServiceStart, SERVICE_EVENT,
};
use crate::vcp_modules::settings_manager::{read_settings, SettingsState};
use std::{
    collections::{HashMap, VecDeque},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
};
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tauri_plugin_vcp_mobile::voice::{SpeechTransition, VoiceAudioEvent};
use tokio::sync::{mpsc, Notify, OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

const INPUT_LIMIT: usize = 4;
const AUDIO_SLOTS: usize = 3;
const CANCELLED_IDS: usize = 128;

pub(super) struct PreparedSegment {
    pub sequence: u32,
    pub final_segment: bool,
    pub wav: Vec<u8>,
    pub _slot: OwnedSemaphorePermit,
}

pub(super) struct Utterance {
    pub id: String,
    pub batch_id: String,
    pub sequence: u64,
    pub cancel: CancellationToken,
    pub segments: mpsc::Sender<PreparedSegment>,
}

struct ActiveCapture {
    turn: Arc<Utterance>,
    window: CaptureWindow,
}

#[derive(Default)]
struct SessionInner {
    utterances: HashMap<String, Arc<Utterance>>,
    capture: Option<ActiveCapture>,
    audio_epoch: Option<u32>,
    last_sample: u64,
    allowed_after: u64,
    local_probe_blocked: bool,
}

struct AudioResources {
    slots: Arc<Semaphore>,
    export_lock: tokio::sync::Mutex<()>,
}

pub(super) struct VoiceSession {
    pub id: String,
    pub config: VoiceServiceConfig,
    pub cancel: CancellationToken,
    pub call: Arc<CallInput>,
    inner: Mutex<SessionInner>,
    resources: Arc<AudioResources>,
    input_closed: AtomicBool,
    pending_exports: AtomicUsize,
    export_done: Notify,
    segments_released: AtomicBool,
    closed_event_sent: AtomicBool,
}

struct SessionSlot {
    call_id: String,
    cancel: CancellationToken,
    session: Option<Arc<VoiceSession>>,
    closed: bool,
}

#[derive(Default)]
struct Registry {
    inputs: HashMap<String, SessionSlot>,
    calls: HashMap<String, Arc<CallInput>>,
    active: Option<String>,
    cancelled: VecDeque<String>,
    cancelled_calls: VecDeque<String>,
}

impl Registry {
    fn remember_cancelled(&mut self, id: &str) {
        if self.cancelled.iter().any(|old| old == id) {
            return;
        }
        if self.cancelled.len() == CANCELLED_IDS {
            self.cancelled.pop_front();
        }
        self.cancelled.push_back(id.into());
    }
    fn reserve(
        &mut self,
        id: String,
        call_id: String,
    ) -> Result<(CancellationToken, Arc<CallInput>), String> {
        if self.cancelled.contains(&id) {
            return Err(network::CANCELLED.into());
        }
        if self.cancelled_calls.contains(&call_id) {
            return Err(network::CANCELLED.into());
        }
        if self.active.is_some() || self.inputs.contains_key(&id) {
            return Err("voice_session_busy: voice input is already active".into());
        }
        if self.inputs.len() >= INPUT_LIMIT {
            return Err("仍有语音识别正在处理，请稍后再开启麦克风。".into());
        }
        if !self.calls.contains_key(&call_id) && self.calls.len() >= INPUT_LIMIT {
            return Err("仍有语音通话尚未结束。".into());
        }
        let call = self
            .calls
            .entry(call_id.clone())
            .or_insert_with(|| Arc::new(CallInput::new(call_id.clone())))
            .clone();
        let cancel = call.cancel.child_token();
        self.inputs.insert(
            id.clone(),
            SessionSlot {
                call_id,
                cancel: cancel.clone(),
                session: None,
                closed: false,
            },
        );
        self.active = Some(id);
        Ok((cancel, call))
    }
    fn close(&mut self, id: &str) -> Option<Arc<VoiceSession>> {
        if self.active.as_deref() == Some(id) {
            self.active = None;
        }
        let slot = self.inputs.get_mut(id)?;
        slot.closed = true;
        if slot.session.is_none() {
            slot.cancel.cancel();
            self.inputs.remove(id);
            self.remember_cancelled(id);
            return None;
        }
        slot.session.clone()
    }
    fn cancel(&mut self, id: &str) -> Option<Arc<VoiceSession>> {
        self.remember_cancelled(id);
        if self.active.as_deref() == Some(id) {
            self.active = None;
        }
        let slot = self.inputs.remove(id)?;
        slot.cancel.cancel();
        slot.session
    }

    fn cancel_call(&mut self, call_id: &str) -> Vec<String> {
        if !self.cancelled_calls.iter().any(|old| old == call_id) {
            if self.cancelled_calls.len() == CANCELLED_IDS {
                self.cancelled_calls.pop_front();
            }
            self.cancelled_calls.push_back(call_id.into());
        }
        if let Some(call) = self.calls.remove(call_id) {
            call.cancel.cancel();
        }
        self.inputs
            .iter()
            .filter(|(_, slot)| slot.call_id == call_id)
            .map(|(id, _)| id.clone())
            .collect()
    }
}

pub struct VoiceServiceState {
    registry: Mutex<Registry>,
    resources: Arc<AudioResources>,
}

impl Default for VoiceServiceState {
    fn default() -> Self {
        Self {
            registry: Mutex::new(Registry::default()),
            resources: Arc::new(AudioResources {
                slots: Arc::new(Semaphore::new(AUDIO_SLOTS)),
                export_lock: tokio::sync::Mutex::new(()),
            }),
        }
    }
}

fn validate_id(id: &str) -> Result<(), String> {
    if id.is_empty() || id.len() > 128 {
        Err("语音会话标识无效。".into())
    } else {
        Ok(())
    }
}

fn session<R: Runtime>(app: &AppHandle<R>, id: &str) -> Option<Arc<VoiceSession>> {
    app.state::<VoiceServiceState>()
        .registry
        .lock()
        .ok()?
        .inputs
        .get(id)?
        .session
        .clone()
}

pub(super) fn emit_current<R: Runtime>(
    app: &AppHandle<R>,
    session: &VoiceSession,
    turn: &Arc<Utterance>,
    kind: &'static str,
    stage: Option<&'static str>,
    text: Option<String>,
) {
    let Ok(inner) = session.inner.lock() else {
        return;
    };
    if !turn.cancel.is_cancelled()
        && inner
            .utterances
            .get(&turn.id)
            .is_some_and(|current| Arc::ptr_eq(current, turn))
    {
        if kind == "asr-partial" {
            if let Some(text) = text.and_then(|text| {
                session
                    .call
                    .partial(&turn.batch_id, turn.sequence, text)
                    .ok()
                    .flatten()
            }) {
                let _ = app.emit(
                    SERVICE_EVENT,
                    VoiceServiceEvent {
                        session_id: session.id.clone(),
                        call_id: session.call.id.clone(),
                        kind,
                        turn_id: Some(turn.batch_id.clone()),
                        batch_id: Some(turn.batch_id.clone()),
                        offer_id: None,
                        text: Some(text),
                        stage,
                        message: None,
                    },
                );
            }
        } else {
            emit(
                app,
                &session.id,
                &session.call.id,
                Some(&turn.id),
                kind,
                stage,
                text,
                None,
            );
        }
    }
}

pub(super) fn record_transcript<R: Runtime>(
    app: &AppHandle<R>,
    session: &Arc<VoiceSession>,
    turn: &Arc<Utterance>,
    text: String,
) {
    if turn.cancel.is_cancelled() {
        return;
    }
    match session.call.complete(&turn.batch_id, turn.sequence, text) {
        Ok(BatchCompletion::Offered(offer)) => {
            let _ = app.emit(
                SERVICE_EVENT,
                VoiceServiceEvent {
                    session_id: offer.session_id,
                    call_id: offer.call_id,
                    kind: "transcript",
                    turn_id: Some(offer.offer_id.clone()),
                    batch_id: Some(offer.batch_id),
                    offer_id: Some(offer.offer_id),
                    text: Some(offer.text),
                    stage: Some("ready"),
                    message: None,
                },
            );
        }
        Ok(BatchCompletion::Empty) => {
            emit_current(app, session, turn, "status", Some("listening"), None)
        }
        Ok(BatchCompletion::Pending) => {}
        Err(error) => {
            fail_turn(app, session, turn, "asr", error);
            return;
        }
    }
    complete_turn(app, session, turn);
}

pub(super) fn complete_turn<R: Runtime>(
    app: &AppHandle<R>,
    session: &Arc<VoiceSession>,
    turn: &Arc<Utterance>,
) {
    if let Ok(mut inner) = session.inner.lock() {
        if inner
            .utterances
            .get(&turn.id)
            .is_some_and(|current| Arc::ptr_eq(current, turn))
        {
            inner.utterances.remove(&turn.id);
        }
    }
    cleanup_closed(app, session);
}

pub(super) fn fail_turn<R: Runtime>(
    app: &AppHandle<R>,
    session: &Arc<VoiceSession>,
    turn: &Arc<Utterance>,
    stage: &'static str,
    message: String,
) {
    {
        let Ok(mut inner) = session.inner.lock() else {
            return;
        };
        if inner
            .utterances
            .get(&turn.id)
            .is_none_or(|current| !Arc::ptr_eq(current, turn))
            || turn.cancel.is_cancelled()
        {
            return;
        }
        turn.cancel.cancel();
        session.call.fail(&turn.batch_id, turn.sequence);
        inner.utterances.remove(&turn.id);
        if inner
            .capture
            .as_ref()
            .is_some_and(|capture| Arc::ptr_eq(&capture.turn, turn))
        {
            inner.capture = None;
        }
        emit(
            app,
            &session.id,
            &session.call.id,
            Some(&turn.id),
            "error",
            Some(stage),
            None,
            Some(message),
        );
    }
    cleanup_closed(app, session);
}

fn cleanup_closed<R: Runtime>(app: &AppHandle<R>, session: &Arc<VoiceSession>) {
    if !session.input_closed.load(Ordering::Acquire)
        || session.pending_exports.load(Ordering::Acquire) != 0
    {
        return;
    }
    if !session.segments_released.swap(true, Ordering::AcqRel) {
        let app = app.clone();
        let id = session.id.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(error) = tauri_plugin_vcp_mobile::voice::release_segments(app, id).await {
                log::warn!(
                    "[VoiceService] Sealed input release failed: {}",
                    error.chars().take(160).collect::<String>()
                );
            }
        });
    }
    let empty = session
        .inner
        .lock()
        .is_ok_and(|inner| inner.utterances.is_empty());
    if empty && !session.closed_event_sent.swap(true, Ordering::AcqRel) {
        let state = app.state::<VoiceServiceState>();
        if let Ok(mut registry) = state.registry.lock() {
            if registry
                .inputs
                .get(&session.id)
                .and_then(|slot| slot.session.as_ref())
                .is_some_and(|current| Arc::ptr_eq(current, session))
            {
                registry.inputs.remove(&session.id);
                registry.remember_cancelled(&session.id);
            }
        }
        emit(
            app,
            &session.id,
            &session.call.id,
            None,
            "status",
            Some("closed"),
            None,
            None,
        );
    }
}

pub(super) struct ExportCompletion<R: Runtime> {
    app: AppHandle<R>,
    session: Arc<VoiceSession>,
}
impl<R: Runtime> Drop for ExportCompletion<R> {
    fn drop(&mut self) {
        if self.session.pending_exports.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.session.export_done.notify_waiters();
        }
        cleanup_closed(&self.app, &self.session);
    }
}

pub(super) async fn export_admission<'a>(
    session: &'a VoiceSession,
    turn: &Utterance,
) -> Option<tokio::sync::MutexGuard<'a, ()>> {
    tokio::select! { _ = turn.cancel.cancelled() => None, guard = session.resources.export_lock.lock() => Some(guard) }
}

type ExportJob = (Arc<Utterance>, SegmentWindow);

fn dispatch_exports<R: Runtime>(
    app: &AppHandle<R>,
    session: &Arc<VoiceSession>,
    jobs: Vec<ExportJob>,
) {
    for (turn, window) in jobs {
        let completion = ExportCompletion {
            app: app.clone(),
            session: session.clone(),
        };
        if turn.cancel.is_cancelled() {
            drop(completion);
            continue;
        }
        match session.resources.slots.clone().try_acquire_owned() {
            Ok(slot) => {
                tauri::async_runtime::spawn(workers::export(
                    app.clone(),
                    session.clone(),
                    turn,
                    window,
                    slot,
                    completion,
                ));
            }
            Err(_) => {
                fail_turn(
                    app,
                    session,
                    &turn,
                    "asr",
                    "语音识别处理速度不足，已取消这段识别，请分段重试。".into(),
                );
                drop(completion);
            }
        }
    }
}

fn seal<R: Runtime>(
    app: &AppHandle<R>,
    session: &Arc<VoiceSession>,
    epoch: Option<u32>,
    sample: Option<u64>,
) {
    let mut failed = None;
    let jobs = {
        let Ok(mut inner) = session.inner.lock() else {
            return;
        };
        if session.input_closed.swap(true, Ordering::AcqRel) {
            return;
        }
        let end = sample
            .filter(|index| *index >= inner.last_sample)
            .unwrap_or(inner.last_sample);
        let jobs = if let Some(mut capture) = inner.capture.take() {
            session
                .call
                .end_speech(&capture.turn.batch_id, capture.turn.sequence);
            if epoch.is_some() && epoch != inner.audio_epoch {
                failed = Some(capture.turn);
                Vec::new()
            } else {
                capture
                    .window
                    .advance(end, true)
                    .into_iter()
                    .map(|window| (capture.turn.clone(), window))
                    .collect()
            }
        } else {
            Vec::new()
        };
        session
            .pending_exports
            .fetch_add(jobs.len(), Ordering::AcqRel);
        jobs
    };
    if let Some(turn) = failed {
        fail_turn(
            app,
            session,
            &turn,
            "asr",
            "音频路线发生变化，这段语音未能完整识别。".into(),
        );
    }
    dispatch_exports(app, session, jobs);
    cleanup_closed(app, session);
}

#[tauri::command]
pub async fn start_voice_service<R: Runtime>(
    app: AppHandle<R>,
    session_id: String,
    call_id: String,
) -> Result<VoiceServiceStart, String> {
    validate_id(&session_id)?;
    validate_id(&call_id)?;
    let (cancel, call) = app
        .state::<VoiceServiceState>()
        .registry
        .lock()
        .map_err(|error| error.to_string())?
        .reserve(session_id.clone(), call_id.clone())?;
    let result = read_settings(app.clone(), app.state::<SettingsState>())
        .await
        .map(VoiceServiceConfig::from_settings)
        .and_then(|config| {
            config.validate()?;
            Ok(config)
        });
    if cancel.is_cancelled() {
        return Err(network::CANCELLED.into());
    }
    let config = match result {
        Ok(config) => config,
        Err(error) => {
            let state = app.state::<VoiceServiceState>();
            let mut registry = state.registry.lock().map_err(|lock| lock.to_string())?;
            if !cancel.is_cancelled() {
                registry.cancel(&session_id);
            }
            return Err(error);
        }
    };
    let configured = config.configured();
    let state = app.state::<VoiceServiceState>();
    let input = Arc::new(VoiceSession {
        id: session_id.clone(),
        config,
        call,
        cancel: cancel.clone(),
        inner: Mutex::new(SessionInner::default()),
        resources: state.resources.clone(),
        input_closed: AtomicBool::new(false),
        pending_exports: AtomicUsize::new(0),
        export_done: Notify::new(),
        segments_released: AtomicBool::new(false),
        closed_event_sent: AtomicBool::new(false),
    });
    {
        let mut registry = state.registry.lock().map_err(|error| error.to_string())?;
        let slot = registry
            .inputs
            .get_mut(&session_id)
            .filter(|slot| !slot.closed && !cancel.is_cancelled())
            .ok_or_else(|| network::CANCELLED.to_string())?;
        slot.session = Some(input);
    }
    emit(
        &app,
        &session_id,
        &call_id,
        None,
        "status",
        Some(if configured {
            "listening"
        } else {
            "local-only"
        }),
        None,
        None,
    );
    Ok(VoiceServiceStart { configured })
}

#[tauri::command]
pub async fn close_voice_input<R: Runtime>(
    app: AppHandle<R>,
    session_id: String,
    audio_epoch: Option<u32>,
    sample_index: Option<u64>,
) -> Result<(), String> {
    let input = app
        .state::<VoiceServiceState>()
        .registry
        .lock()
        .map_err(|error| error.to_string())?
        .close(&session_id);
    let Some(input) = input else {
        return Ok(());
    };
    seal(&app, &input, audio_epoch, sample_index);
    loop {
        let notified = input.export_done.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if input.pending_exports.load(Ordering::Acquire) == 0 {
            break;
        }
        notified.await;
    }
    cleanup_closed(&app, &input);
    Ok(())
}

#[tauri::command]
pub async fn stop_voice_service<R: Runtime>(
    app: AppHandle<R>,
    session_id: String,
) -> Result<(), String> {
    validate_id(&session_id)?;
    let input = app
        .state::<VoiceServiceState>()
        .registry
        .lock()
        .map_err(|error| error.to_string())?
        .cancel(&session_id);
    if let Some(input) = input {
        input.input_closed.store(true, Ordering::Release);
        if let Ok(mut inner) = input.inner.lock() {
            for turn in inner.utterances.values() {
                input.call.fail(&turn.batch_id, turn.sequence);
            }
            inner.capture = None;
            inner.utterances.clear();
        }
        cleanup_closed(&app, &input);
    }
    Ok(())
}

#[tauri::command]
pub async fn stop_voice_call_inputs<R: Runtime>(
    app: AppHandle<R>,
    call_id: String,
) -> Result<(), String> {
    validate_id(&call_id)?;
    let sessions = {
        let state = app.state::<VoiceServiceState>();
        let mut registry = state.registry.lock().map_err(|error| error.to_string())?;
        registry.cancel_call(&call_id)
    };
    for id in sessions {
        stop_voice_service(app.clone(), id).await?;
    }
    Ok(())
}

#[tauri::command]
pub fn claim_voice_transcript<R: Runtime>(
    app: AppHandle<R>,
    call_id: String,
    offer_id: String,
) -> Result<ClaimedTranscript, String> {
    let call = app
        .state::<VoiceServiceState>()
        .registry
        .lock()
        .map_err(|error| error.to_string())?
        .calls
        .get(&call_id)
        .cloned();
    if let Some(call) = call {
        call.claim(&offer_id)
    } else {
        Ok(ClaimedTranscript::rejected())
    }
}

pub fn handle_audio_event<R: Runtime>(app: &AppHandle<R>, event: &VoiceAudioEvent) {
    let id = match event {
        VoiceAudioEvent::Frame(event) => &event.frame.session_id,
        VoiceAudioEvent::Stopped(event) | VoiceAudioEvent::Error(event) => &event.session_id,
    };
    let Some(input) = session(app, id) else {
        return;
    };
    let event = match event {
        VoiceAudioEvent::Frame(event) => event,
        VoiceAudioEvent::Stopped(notice) | VoiceAudioEvent::Error(notice) => {
            if let Ok(mut registry) = app.state::<VoiceServiceState>().registry.lock() {
                registry.close(id);
            }
            seal(app, &input, Some(notice.audio_epoch), notice.sample_index);
            return;
        }
    };
    if input.cancel.is_cancelled() || input.input_closed.load(Ordering::Acquire) {
        return;
    }
    let frame = &event.frame;
    let mut failed = None;
    let mut created = None;
    let mut jobs = Vec::new();
    {
        let Ok(mut inner) = input.inner.lock() else {
            return;
        };
        if input.input_closed.load(Ordering::Acquire) {
            return;
        }
        if inner
            .audio_epoch
            .is_some_and(|epoch| frame.audio_epoch < epoch)
            || (inner.audio_epoch == Some(frame.audio_epoch)
                && frame.sample_index <= inner.last_sample)
        {
            return;
        }
        if inner
            .audio_epoch
            .is_some_and(|epoch| epoch != frame.audio_epoch)
            || event.discontinuity
        {
            failed = inner.capture.take().map(|capture| capture.turn);
            inner.allowed_after = frame.sample_index;
        }
        inner.audio_epoch = Some(frame.audio_epoch);
        inner.last_sample = frame.sample_index;
        let probe = frame.playback_active && frame.playback_id.is_none();
        if probe {
            if failed.is_none() {
                failed = inner.capture.take().map(|capture| capture.turn);
            }
            inner.local_probe_blocked = true;
            inner.allowed_after = frame.sample_index;
        }
        if inner.local_probe_blocked {
            if !probe && !event.speech_active {
                inner.local_probe_blocked = false;
                inner.allowed_after = frame.sample_index;
            }
        } else {
            if event.speech_transition == Some(SpeechTransition::Start) {
                // Only this source announces a barge-in. Input close and ASR
                // failures never cancel or signal cancellation of a reply job.
                if input.config.configured() {
                    let began = match input.call.begin(&input.id) {
                        Ok(began) => began,
                        Err(error) => {
                            emit(
                                app,
                                &input.id,
                                &input.call.id,
                                None,
                                "error",
                                Some("asr"),
                                None,
                                Some(error),
                            );
                            return;
                        }
                    };
                    if let Some(old_turn) = began.barge {
                        emit(
                            app,
                            &input.id,
                            &input.call.id,
                            Some(&old_turn),
                            "barge-in",
                            None,
                            None,
                            None,
                        );
                    }
                    let (sender, receiver) = mpsc::channel(AUDIO_SLOTS);
                    let turn = Arc::new(Utterance {
                        id: uuid::Uuid::new_v4().to_string(),
                        batch_id: began.batch_id,
                        sequence: began.sequence,
                        cancel: input.cancel.child_token(),
                        segments: sender,
                    });
                    inner.capture = Some(ActiveCapture {
                        turn: turn.clone(),
                        window: CaptureWindow::new(
                            frame.audio_epoch,
                            frame.sample_index,
                            inner.allowed_after,
                        ),
                    });
                    inner.utterances.insert(turn.id.clone(), turn.clone());
                    created = Some((turn, receiver));
                } else if let Some(old_turn) = input.call.sent_turn() {
                    emit(
                        app,
                        &input.id,
                        &input.call.id,
                        Some(&old_turn),
                        "barge-in",
                        None,
                        None,
                        None,
                    );
                }
            }
            let finished = event.speech_transition == Some(SpeechTransition::End);
            if let Some(capture) = inner.capture.as_mut() {
                jobs = capture
                    .window
                    .advance(frame.sample_index, finished)
                    .into_iter()
                    .map(|window| (capture.turn.clone(), window))
                    .collect();
            }
            if finished {
                if let Some(capture) = inner.capture.take() {
                    input
                        .call
                        .end_speech(&capture.turn.batch_id, capture.turn.sequence);
                }
            }
            input
                .pending_exports
                .fetch_add(jobs.len(), Ordering::AcqRel);
        }
    }
    if let Some(turn) = failed {
        fail_turn(
            app,
            &input,
            &turn,
            "asr",
            "音频输入中断或进入本地诊断，这段语音已取消识别。".into(),
        );
    }
    if let Some((turn, receiver)) = created {
        emit_current(app, &input, &turn, "status", Some("recording"), None);
        tauri::async_runtime::spawn(workers::recognize(
            app.clone(),
            input.clone(),
            turn,
            receiver,
        ));
    }
    dispatch_exports(app, &input, jobs);
}

#[cfg(test)]
mod tests {
    use super::*;
    fn install(registry: &mut Registry, id: &str, call_id: &str) -> Arc<VoiceSession> {
        let (cancel, call) = registry.reserve(id.into(), call_id.into()).unwrap();
        let input = Arc::new(VoiceSession {
            id: id.into(),
            call,
            cancel,
            config: VoiceServiceConfig {
                base_url: "https://example.invalid/v1".into(),
                api_key: "test".into(),
                asr_stream: true,
                tts_stream: true,
                language: "auto".into(),
                voice: "mimo_default".into(),
            },
            inner: Mutex::new(SessionInner::default()),
            resources: VoiceServiceState::default().resources,
            input_closed: AtomicBool::new(false),
            pending_exports: AtomicUsize::new(0),
            export_done: Notify::new(),
            segments_released: AtomicBool::new(false),
            closed_event_sent: AtomicBool::new(false),
        });
        registry.inputs.get_mut(id).unwrap().session = Some(input.clone());
        input
    }
    #[test]
    fn mute_keeps_accepted_asr_alive_and_new_microphone_shares_the_call_batch() {
        let mut registry = Registry::default();
        let old = install(&mut registry, "mic-a", "call");
        let asr = old.cancel.child_token();
        registry.close("mic-a").unwrap();
        assert!(!asr.is_cancelled());
        let current = install(&mut registry, "mic-b", "call");
        assert!(Arc::ptr_eq(&old.call, &current.call));
        registry.cancel("mic-a");
        assert!(asr.is_cancelled());
        assert!(!current.cancel.is_cancelled());
        assert_eq!(registry.active.as_deref(), Some("mic-b"));
    }
    #[test]
    fn closing_during_configuration_does_not_leak_an_input_reservation() {
        let mut registry = Registry::default();
        let (pending, _) = registry.reserve("pending".into(), "call".into()).unwrap();
        assert!(registry.close("pending").is_none());
        assert!(pending.is_cancelled());
        assert!(!registry.inputs.contains_key("pending"));
        registry.reserve("next".into(), "call".into()).unwrap();
        assert!(registry.reserve("pending".into(), "call".into()).is_err());
    }
    #[test]
    fn hangup_cancels_all_input_children_of_only_its_call() {
        let mut registry = Registry::default();
        let first = install(&mut registry, "mic-a", "first-call");
        registry.close("mic-a");
        let second = install(&mut registry, "mic-b", "first-call");
        registry.close("mic-b");
        let other = install(&mut registry, "mic-c", "other-call");
        let ids = registry.cancel_call("first-call");
        assert_eq!(ids.len(), 2);
        assert!(first.cancel.is_cancelled());
        assert!(second.cancel.is_cancelled());
        assert!(!other.cancel.is_cancelled());
        registry.close("mic-c");
        assert!(registry
            .reserve("late".into(), "first-call".into())
            .is_err());
    }
}

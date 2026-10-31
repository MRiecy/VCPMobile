//! Capture/ASR ownership only. Closing input drains accepted audio; explicit
//! cancellation discards it. Neither path owns model generation or TTS.

use super::{
    batch::{BatchCompletion, CallInput},
    call, document::VoiceDocumentState, network,
    segmentation::{CaptureWindow, SegmentWindow},
    workers, VoiceServiceConfig, VoiceServiceStart,
};
use crate::vcp_modules::settings_manager::{read_config, SettingsState};
use std::{
    collections::{HashMap, VecDeque},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
};
use tauri::{AppHandle, Manager};
use tauri_plugin_vcp_mobile::voice::{SpeechTransition, VoiceAudioEvent, VoiceVadConfig};
use tokio::sync::{mpsc, Notify, OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

const INPUT_LIMIT: usize = 16;
// Includes export, encoding, queued segments and ASR requests across inputs.
// 32 full 30-second PCM16 segments occupy about 29.3 MiB before encoding.
const AUDIO_SLOTS: usize = 32;
const CANCELLED_IDS: usize = 128;

pub(super) struct PreparedSegment {
    pub sequence: u32,
    pub final_segment: bool,
    pub audio: super::InputAudio,
    pub _slot: OwnedSemaphorePermit,
}

pub(super) struct Utterance {
    pub id: String,
    pub batch_id: String,
    pub sequence: u64,
    pub cancel: CancellationToken,
    pub segments: mpsc::Sender<PreparedSegment>,
    pub _work: Option<Arc<call::WorkLease>>,
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
}

struct AudioResources {
    slots: Arc<Semaphore>,
    export_lock: tokio::sync::Mutex<()>,
}

pub(super) struct VoiceSession {
    pub id: String,
    pub config: VoiceServiceConfig,
    pub vad_config: VoiceVadConfig,
    pub cancel: CancellationToken,
    capture_cancel: CancellationToken,
    pub call: Arc<CallInput>,
    inner: Mutex<SessionInner>,
    resources: Arc<AudioResources>,
    input_closed: AtomicBool,
    pending_exports: AtomicUsize,
    export_done: Notify,
    segments_released: AtomicBool,
    retired: AtomicBool,
}

struct SessionSlot {
    call_id: String,
    document_id: String,
    cancel: CancellationToken,
    capture_cancel: CancellationToken,
    closing: Arc<tokio::sync::Mutex<()>>,
    session: Option<Arc<VoiceSession>>,
    closed: bool,
}

#[derive(Default)]
struct Registry {
    inputs: HashMap<String, SessionSlot>,
    calls: HashMap<String, Arc<CallInput>>,
    active: Option<String>,
    switch: Option<MicrophoneSwitch>,
    cancelled: VecDeque<String>,
    cancelled_calls: VecDeque<String>,
}

struct MicrophoneSwitch {
    call_id: String,
    previous: String,
    next: String,
}

impl Registry {
    // A mute can arrive after the old recorder stopped and before the new
    // recorder was reserved. Revoke that exact future start in the same lock.
    fn cancel_switch(&mut self, call_id: &str, session_id: Option<&str>) {
        if let Some(switch) = self.switch.take_if(|switch| {
            switch.call_id == call_id
                && session_id.is_none_or(|id| id == switch.previous || id == switch.next)
        }) {
            self.remember_cancelled(&switch.next);
        }
    }

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
        document_id: String,
    ) -> Result<(CancellationToken, CancellationToken, Arc<CallInput>), String> {
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
        if self.switch.as_ref().is_some_and(|switch| switch.next == id) {
            self.switch = None;
        } else {
            self.cancel_switch(&call_id, None);
        }
        let call = self
            .calls
            .entry(call_id.clone())
            .or_insert_with(|| Arc::new(CallInput::new(call_id.clone())))
            .clone();
        let cancel = call.cancel.child_token();
        let capture_cancel = cancel.child_token();
        self.inputs.insert(
            id.clone(),
            SessionSlot {
                call_id,
                document_id,
                cancel: cancel.clone(),
                capture_cancel: capture_cancel.clone(),
                closing: Arc::new(tokio::sync::Mutex::new(())),
                session: None,
                closed: false,
            },
        );
        self.active = Some(id);
        Ok((cancel, capture_cancel, call))
    }
    fn close(&mut self, id: &str) -> Option<Arc<VoiceSession>> {
        if self.active.as_deref() == Some(id) {
            self.active = None;
        }
        let slot = self.inputs.get_mut(id)?;
        slot.closed = true;
        slot.capture_cancel.cancel();
        if slot.session.is_none() {
            slot.cancel.cancel();
            self.inputs.remove(id);
            self.remember_cancelled(id);
            return None;
        }
        slot.session.clone()
    }
    fn prepare_microphone_close(
        &mut self,
        call_id: &str,
        requested_id: Option<&str>,
    ) -> Result<Option<String>, String> {
        // An explicit ID also revokes a start that has not been polled yet.
        // Never replace that exact target with a newer microphone of this call.
        let id = match requested_id {
            Some(id) => {
                if self.inputs.get(id).is_some_and(|slot| slot.call_id != call_id) {
                    return Err("voice_call_not_active: microphone belongs to another call".into());
                }
                id.to_string()
            }
            None => match self.active.as_ref().filter(|id| {
                self.inputs.get(*id).is_some_and(|slot| slot.call_id == call_id)
            }) {
                Some(id) => id.clone(),
                None => return Ok(None),
            },
        };
        self.remember_cancelled(&id);
        if let Some(slot) = self.inputs.get_mut(&id) {
            slot.closed = true;
            slot.capture_cancel.cancel();
            if slot.session.is_none() {
                // Configuration is still loading. It must not publish readiness
                // or acquire hardware after this close intent has been accepted.
                slot.cancel.cancel();
                self.inputs.remove(&id);
                if self.active.as_deref() == Some(id.as_str()) {
                    self.active = None;
                }
            }
            // Ready input keeps its ASR token and registry identity until the
            // physical stop receipt seals it. A failed stop can then be retried.
        }
        Ok(Some(id))
    }
    fn allows_start(&self, id: &str) -> bool {
        self.inputs.get(id).is_some_and(|slot| !slot.closed && !slot.capture_cancel.is_cancelled())
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
        self.cancel_switch(call_id, None);
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

/// A close fixes its resource identity before yielding. It never resolves the
/// current microphone again after another document has taken over.
struct CaptureClose {
    call_id: String,
    session_id: String,
    serial: Arc<tokio::sync::Mutex<()>>,
}

pub(super) struct DeviceSwitch {
    close: CaptureClose,
    next: String,
}

pub(super) fn prepare_device_switch<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>, call_id: &str, expected_session: Option<&str>, document_id: &str,
) -> Result<Option<DeviceSwitch>, String> {
    let state = app.state::<VoiceServiceState>();
    let mut registry = state.registry.lock().map_err(|error| error.to_string())?;
    if registry.switch.is_some() || registry.active.as_deref() != expected_session {
        return Err("voice_session_busy".into());
    }
    let Some(previous) = expected_session else { return Ok(None); };
    if !registry.inputs.get(previous).is_some_and(|slot|
        slot.call_id == call_id && slot.document_id == document_id && !slot.closed) {
        return Err("voice_session_busy".into());
    }
    let close = registry.prepare_close(call_id, Some(previous))?
        .ok_or_else(|| network::CANCELLED.to_string())?;
    let next = uuid::Uuid::new_v4().to_string();
    registry.switch = Some(MicrophoneSwitch {
        call_id: call_id.into(), previous: previous.into(), next: next.clone(),
    });
    Ok(Some(DeviceSwitch { close, next }))
}

pub(super) async fn finish_device_switch(
    app: AppHandle, switch: DeviceSwitch, document_id: String, lifecycle_epoch: Option<u64>,
) -> Result<(), String> {
    let call_id = switch.close.call_id.clone();
    let result = async {
        finish_capture_close(app.clone(), switch.close, "requested", false).await?;
        start_voice_microphone(app.clone(), switch.next.clone(), call_id.clone(), document_id, lifecycle_epoch).await?;
        Ok(())
    }.await;
    // Failed close, stale document, or background admission before reserve().
    let state = app.state::<VoiceServiceState>();
    state.registry.lock().map_err(|error| error.to_string())?.cancel_switch(&call_id, Some(&switch.next));
    result
}

impl Registry {
    fn prepare_mute(&mut self, call_id: &str, requested: Option<&str>) -> Result<Option<CaptureClose>, String> {
        self.cancel_switch(call_id, requested);
        self.prepare_close(call_id, requested)
    }

    fn prepare_close(&mut self, call_id: &str, requested: Option<&str>) -> Result<Option<CaptureClose>, String> {
        let Some(session_id) = self.prepare_microphone_close(call_id, requested)? else { return Ok(None); };
        let serial = self.inputs.get(&session_id).map(|slot| slot.closing.clone())
            .unwrap_or_default();
        Ok(Some(CaptureClose { call_id: call_id.into(), session_id, serial }))
    }

    fn prepare_document_close(&mut self, document_id: &str, other: bool) -> Result<Vec<CaptureClose>, String> {
        let targets: Vec<_> = self.inputs.iter()
            .filter(|(_, slot)| (slot.document_id != document_id) == other
                && slot.session.as_ref().is_none_or(|session| !session.input_closed.load(Ordering::Acquire)))
            .map(|(id, slot)| (slot.call_id.clone(), id.clone())).collect();
        targets.into_iter().filter_map(|(call, id)| self.prepare_close(&call, Some(&id)).transpose()).collect()
    }
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

pub(super) fn register_call<R: tauri::Runtime>(app: &AppHandle<R>, call: Arc<CallInput>) -> Result<(), String> {
    app.state::<VoiceServiceState>()
        .registry
        .lock()
        .map_err(|error| error.to_string())?
        .calls
        .insert(call.id.clone(), call);
    Ok(())
}

fn validate_id(id: &str) -> Result<(), String> {
    if id.is_empty() || id.len() > 128 {
        Err("语音会话标识无效。".into())
    } else {
        Ok(())
    }
}

fn session(app: &AppHandle, id: &str) -> Option<Arc<VoiceSession>> {
    app.state::<VoiceServiceState>()
        .registry
        .lock()
        .ok()?
        .inputs
        .get(id)?
        .session
        .clone()
}

pub(super) fn update_input(
    app: &AppHandle,
    session: &VoiceSession,
    turn: &Arc<Utterance>,
    stage: &'static str,
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
        if text.is_some() {
            if let Some(text) = text.and_then(|text| {
                session
                    .call
                    .partial(&turn.batch_id, turn.sequence, text)
                    .ok()
                    .flatten()
            }) {
                call::input_status(app, &session.call.id, &session.id, "asr", Some(&text), None);
            }
        } else {
            call::input_status(app, &session.call.id, &session.id, stage, None, None);
        }
    }
}

pub(super) fn record_transcript(
    app: &AppHandle,
    session: &Arc<VoiceSession>,
    turn: &Arc<Utterance>,
    text: String,
) {
    if turn.cancel.is_cancelled() {
        return;
    }
    match session.call.complete(&turn.batch_id, turn.sequence, text) {
        Ok(BatchCompletion::Offered(offer)) => {
            call::offer(app, offer);
        }
        Ok(BatchCompletion::Empty) => {
            call::input_status(app, &session.call.id, &session.id, "empty", None, None);
        }
        Ok(BatchCompletion::Pending) => {}
        Err(error) => {
            fail_turn(app, session, turn, "asr", error);
            return;
        }
    }
    complete_turn(app, session, turn);
}

pub(super) fn complete_turn(app: &AppHandle, session: &Arc<VoiceSession>, turn: &Arc<Utterance>) {
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

pub(super) fn fail_turn(
    app: &AppHandle,
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
        call::input_status(
            app,
            &session.call.id,
            &session.id,
            stage,
            None,
            Some(&message),
        );
    }
    cleanup_closed(app, session);
}

fn cleanup_closed(app: &AppHandle, session: &Arc<VoiceSession>) {
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
    if empty && !session.retired.swap(true, Ordering::AcqRel) {
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
        };
    }
}

pub(super) struct ExportCompletion {
    app: AppHandle,
    session: Arc<VoiceSession>,
}
impl Drop for ExportCompletion {
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

fn dispatch_exports(app: &AppHandle, session: &Arc<VoiceSession>, jobs: Vec<ExportJob>) {
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

fn seal(app: &AppHandle, session: &Arc<VoiceSession>, epoch: Option<u32>, sample: Option<u64>) {
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
    call::input_status(app, &session.call.id, &session.id, "closed", None, None);
    cleanup_closed(app, session);
}

async fn start_voice_service(
    app: AppHandle,
    session_id: String,
    call_id: String,
    document_id: &str,
) -> Result<(Arc<VoiceSession>, Option<String>), String> {
    validate_id(&session_id)?;
    validate_id(&call_id)?;
    // Selection and admission share the document lock; a start cannot apply
    // a preference read before another valid device choice.
    let ((vad_config, input_device_id), (cancel, capture_cancel, call)) = app.state::<VoiceDocumentState>().with_current(document_id, || {
        let config = call::input_config(&app, &call_id)?;
        let slot = app.state::<VoiceServiceState>().registry.lock().map_err(|error| error.to_string())?
            .reserve(session_id.clone(), call_id.clone(), document_id.into())?;
        Ok((config, slot))
    })?;
    call::input_starting(&app, &call_id, &session_id, &capture_cancel);
    let result = read_config(app.clone(), app.state::<SettingsState>())
        .await
        .and_then(|settings| {
            let config = VoiceServiceConfig::from_settings(&settings);
            config.validate_asr()?;
            Ok(config)
        });
    if capture_cancel.is_cancelled() {
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
        vad_config,
        call,
        cancel: cancel.clone(),
        capture_cancel: capture_cancel.clone(),
        inner: Mutex::new(SessionInner::default()),
        resources: state.resources.clone(),
        input_closed: AtomicBool::new(false),
        pending_exports: AtomicUsize::new(0),
        export_done: Notify::new(),
        segments_released: AtomicBool::new(false),
        retired: AtomicBool::new(false),
    });
    app.state::<VoiceDocumentState>().with_current(document_id, || {
        let mut registry = state.registry.lock().map_err(|error| error.to_string())?;
        let slot = registry
            .inputs
            .get_mut(&session_id)
            .filter(|slot| !slot.closed && !capture_cancel.is_cancelled())
            .ok_or_else(|| network::CANCELLED.to_string())?;
        slot.session = Some(input.clone());
        Ok(())
    })?;
    call::input_status(
        &app,
        &call_id,
        &session_id,
        if configured { "ready" } else { "local-only" },
        None,
        None,
    );
    Ok((input, input_device_id))
}

#[tauri::command]
pub async fn start_voice_microphone(
    app: AppHandle,
    session_id: String,
    call_id: String,
    document_id: String,
    expected_lifecycle_epoch: Option<u64>,
) -> Result<VoiceServiceStart, String> {
    use crate::vcp_modules::infra::{
        lifecycle_controller::is_app_in_foreground, lifecycle_state::LifecycleState,
    };
    if !is_app_in_foreground(&app) {
        return Err("voice_requires_foreground".into());
    }
    let before = if let Some(state) = app.try_state::<LifecycleState>() {
        state
            .lifecycle_snapshot
            .read()
            .await
            .as_ref()
            .map(|snapshot| snapshot.transition_epoch)
    } else {
        None
    };
    if expected_lifecycle_epoch.is_some() && expected_lifecycle_epoch != before {
        return Err("cancelled: microphone intent crossed a lifecycle transition".into());
    }
    // A new intent may arrive before a preceding close's IPC response. The
    // backend waits for that exact native receipt, never for a permission dialog.
    await_closing_capture(&app, &document_id).await?;
    let (started, input_device_id) = match start_voice_service(app.clone(), session_id.clone(), call_id.clone(), &document_id).await
    {
        Ok(started) => started,
        Err(error) => {
            call::input_stopped(&app, &call_id, &session_id, &error, true);
            return Err(error);
        }
    };
    let after = if let Some(state) = app.try_state::<LifecycleState>() {
        state
            .lifecycle_snapshot
            .read()
            .await
            .as_ref()
            .map(|snapshot| snapshot.transition_epoch)
    } else {
        None
    };
    if !is_app_in_foreground(&app) || before != after {
        stop_voice_service(app.clone(), session_id.clone()).await?;
        call::input_stopped(&app, &call_id, &session_id, "background", false);
        return Err("cancelled: microphone start crossed a lifecycle transition".into());
    }
    let allowed = app.state::<VoiceDocumentState>().with_current(&document_id, || {
        Ok(app.state::<VoiceServiceState>().registry.lock()
            .map_err(|error| error.to_string())?.allows_start(&session_id))
    });
    if !matches!(allowed, Ok(true)) {
        return Err(network::CANCELLED.into());
    }
    if let Err(error) = tauri_plugin_vcp_mobile::voice::start_voice_audio(
        app.clone(), session_id.clone(), call_id.clone(), document_id,
        started.vad_config, input_device_id,
    ).await {
        // A start failure is not a physical-stop receipt. Keep a failed cleanup
        // retryable and retain accepted ASR exactly like an ordinary mute.
        let close = app.state::<VoiceServiceState>().registry.lock()
            .map_err(|lock| lock.to_string())?.prepare_close(&call_id, Some(&session_id))?;
        if let Some(close) = close {
            finish_capture_close(app.clone(), close, &error, true).await?;
        }
        return Err(error);
    }
    if !started.capture_cancel.is_cancelled() {
        call::input_status(&app, &call_id, &session_id, "listening", None, None);
    }
    Ok(VoiceServiceStart { configured: started.config.configured() })
}

#[tauri::command]
pub async fn close_voice_microphone(
    app: AppHandle,
    call_id: String,
    document_id: String,
    session_id: Option<String>,
) -> Result<(), String> {
    validate_id(&call_id)?;
    if let Some(id) = &session_id { validate_id(id)?; }
    let close = app.state::<VoiceDocumentState>().with_current(&document_id, || {
        let state = app.state::<VoiceServiceState>();
        let mut registry = state.registry.lock().map_err(|error| error.to_string())?;
        registry.prepare_mute(&call_id, session_id.as_deref())
    })?;
    let Some(close) = close else { return Ok(()); };
    finish_capture_close(app, close, "requested", false).await
}

async fn finish_capture_close(app: AppHandle, close: CaptureClose, reason: &str, failed: bool) -> Result<(), String> {
    let CaptureClose { call_id, session_id, serial } = close;
    call::input_status(&app, &call_id, &session_id, "stopping", None, None);
    let _closing = serial.lock().await;
    if session(&app, &session_id).is_some_and(|input| input.input_closed.load(Ordering::Acquire)) {
        return Ok(());
    }
    let receipt =
        match tauri_plugin_vcp_mobile::voice::stop_voice_audio(app.clone(), session_id.clone())
            .await
        {
            Ok(receipt) => receipt,
            Err(error) => {
                call::input_stop_failed(&app, &call_id, &session_id, &error);
                return Err(error);
            }
        };
    call::input_stopped(&app, &call_id, &session_id, reason, failed);
    close_voice_input(
        app,
        session_id,
        Some(receipt.audio_epoch),
        Some(receipt.sample_index),
    )
    .await?;
    Ok(())
}

async fn await_closing_capture(app: &AppHandle, document_id: &str) -> Result<(), String> {
    let close = app.state::<VoiceDocumentState>().with_current(document_id, || {
        let state = app.state::<VoiceServiceState>();
        let mut registry = state.registry.lock().map_err(|error| error.to_string())?;
        let closing = registry.active.as_ref().and_then(|id| registry.inputs.get(id)
            .filter(|slot| slot.closed).map(|slot| (slot.call_id.clone(), id.clone())));
        closing.map(|(call, id)| registry.prepare_close(&call, Some(&id))).transpose().map(Option::flatten)
    })?;
    if let Some(close) = close { finish_capture_close(app.clone(), close, "requested", false).await?; }
    Ok(())
}

pub(super) async fn retire_other_documents(app: AppHandle, document_id: &str) -> Result<(), String> {
    let closes = app.state::<VoiceDocumentState>().with_current(document_id, || {
        app.state::<VoiceServiceState>().registry.lock().map_err(|error| error.to_string())?
            .prepare_document_close(document_id, true)
    })?;
    finish_document_closes(app, closes).await
}

async fn finish_document_closes(app: AppHandle, closes: Vec<CaptureClose>) -> Result<(), String> {
    let mut result = Ok(());
    for close in closes {
        if let Err(error) = finish_capture_close(app.clone(), close, "document_replaced", false).await { result = Err(error); }
    }
    result
}

/// Native/host callbacks revoke admission synchronously, before scheduling IO.
/// No JS acknowledgement or replacement document is required for cleanup.
#[cfg(any(target_os = "android", target_os = "ios"))]
pub fn invalidate_document(app: &AppHandle, expected: Option<&str>) {
    // Initial page-load hooks can run before setup has installed domain state.
    let Some(documents) = app.try_state::<VoiceDocumentState>() else { return; };
    let result = (|| {
        let Some(document_id) = documents.invalidate(expected)? else { return Ok(()); };
        let closes = app.state::<VoiceServiceState>().registry.lock().map_err(|error| error.to_string())?
            .prepare_document_close(&document_id, false)?;
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let (native, capture) = tokio::join!(
                tauri_plugin_vcp_mobile::voice::invalidate_voice_document(app.clone(), document_id),
                finish_document_closes(app, closes),
            );
            for result in [native, capture] {
                if let Err(error) = result { log::warn!("[VoiceDocument] capture retirement failed: {error}"); }
            }
        });
        Ok::<_, String>(())
    })();
    if let Err(error) = result { log::warn!("[VoiceDocument] invalidation failed: {error}"); }
}

pub async fn close_voice_input(
    app: AppHandle,
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

pub async fn stop_voice_service(app: AppHandle, session_id: String) -> Result<(), String> {
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
        call::input_status(&app, &input.call.id, &input.id, "closed", None, None);
    }
    Ok(())
}

pub async fn stop_voice_call_inputs(app: AppHandle, call_id: String) -> Result<(), String> {
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

pub fn handle_audio_event(app: &AppHandle, event: &VoiceAudioEvent) {
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
            call::input_stopped(
                app,
                &input.call.id,
                &input.id,
                &notice.reason,
                matches!(event, VoiceAudioEvent::Error(_)),
            );
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
    call::input_status(app, &input.call.id, &input.id, "listening", None, None);
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
        if event.speech_transition == Some(SpeechTransition::Start) {
            // Only this source announces a barge-in. Input close and ASR
            // failures never cancel or signal cancellation of a reply job.
            if input.config.configured() {
                let began = match call::begin_speech(app, &input.call) {
                    Ok(began) => began,
                    Err(error) => {
                        call::input_status(
                            app,
                            &input.call.id,
                            &input.id,
                            "asr",
                            None,
                            Some(&error),
                        );
                        return;
                    }
                };
                let (sender, receiver) = mpsc::channel(AUDIO_SLOTS);
                let turn = Arc::new(Utterance {
                    id: uuid::Uuid::new_v4().to_string(),
                    batch_id: began.batch_id,
                    sequence: began.sequence,
                    cancel: input.cancel.child_token(),
                    segments: sender,
                    _work: call::input_work(app, &input.call.id),
                });
                inner.capture = Some(ActiveCapture {
                    turn: turn.clone(),
                    window: CaptureWindow::new(
                        frame.audio_epoch,
                        frame.sample_index,
                        inner.allowed_after,
                        // Playback may end during a long confirmation and
                        // switch VAD to its shorter ordinary onset. Keep
                        // the whole session's possible leading speech.
                        input.vad_config.onset_samples(true).max(input.vad_config.onset_samples(false)),
                    ),
                });
                inner.utterances.insert(turn.id.clone(), turn.clone());
                created = Some((turn, receiver));
            } else if input.call.has_sent_turn() {
                call::interrupt_reply(app, &input.call.id);
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
            if inner.capture.take().is_some() {
                // Custom short pauses and long onset confirmation must not
                // let the next utterance export already-submitted audio.
                inner.allowed_after = frame.sample_index;
            }
        }
        input
            .pending_exports
            .fetch_add(jobs.len(), Ordering::AcqRel);
    }
    if let Some(turn) = failed {
        fail_turn(
            app,
            &input,
            &turn,
            "asr",
            "音频输入中断，这段语音已取消识别。".into(),
        );
    }
    if let Some((turn, receiver)) = created {
        update_input(app, &input, &turn, "recording", None);
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
        let (cancel, capture_cancel, call) = registry.reserve(id.into(), call_id.into(), "1".into()).unwrap();
        let input = Arc::new(VoiceSession {
            id: id.into(),
            call,
            cancel,
            capture_cancel,
            vad_config: VoiceVadConfig::default(),
            config: VoiceServiceConfig {
                base_url: "https://example.invalid/v1".into(),
                api_key: "test".into(),
                asr_stream: true,
                input_encoding: Default::default(),
                tts_stream: true,
                language: "auto".into(),
                voice: "mimo_default".into(),
                tts_mode: Default::default(),
                clone_sample_id: String::new(),
            },
            inner: Mutex::new(SessionInner::default()),
            resources: VoiceServiceState::default().resources,
            input_closed: AtomicBool::new(false),
            pending_exports: AtomicUsize::new(0),
            export_done: Notify::new(),
            segments_released: AtomicBool::new(false),
            retired: AtomicBool::new(false),
        });
        registry.inputs.get_mut(id).unwrap().session = Some(input.clone());
        input
    }
    #[test]
    fn mute_between_device_release_and_reopen_cancels_only_the_replacement() {
        let app = tauri::test::mock_app();
        app.manage(VoiceServiceState::default());
        let state = app.state::<VoiceServiceState>();
        let old = install(&mut state.registry.lock().unwrap(), "old", "call");
        let change = prepare_device_switch(app.handle(), "call", Some("old"), "1").unwrap().unwrap();
        let mut registry = state.registry.lock().unwrap();
        registry.close("old");
        assert!(registry.prepare_mute("call", None).unwrap().is_none());
        assert!(registry.reserve(change.next, "call".into(), "1".into()).is_err());
        assert!(!old.cancel.is_cancelled());
        assert!(registry.reserve("explicit-unmute".into(), "call".into(), "1".into()).is_ok());
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
        let (pending, _, _) = registry.reserve("pending".into(), "call".into(), "1".into()).unwrap();
        assert!(registry.close("pending").is_none());
        assert!(pending.is_cancelled());
        assert!(!registry.inputs.contains_key("pending"));
        registry.reserve("next".into(), "call".into(), "1".into()).unwrap();
        assert!(registry.reserve("pending".into(), "call".into(), "1".into()).is_err());
    }
    #[test]
    fn recovered_call_closes_its_ready_capture_without_cancelling_accepted_asr() {
        let mut registry = Registry::default();
        let input = install(&mut registry, "surviving-mic", "call");
        let asr = input.cancel.child_token();
        assert_eq!(registry.prepare_microphone_close("call", None).unwrap().as_deref(), Some("surviving-mic"));
        assert!(!registry.allows_start("surviving-mic"));
        assert!(!asr.is_cancelled());
        // Until native acknowledges, keep the exact owner available for a
        // failed-stop retry and prevent a new capture from racing hardware.
        assert!(registry.reserve("next".into(), "call".into(), "1".into()).is_err());
        assert_eq!(registry.prepare_microphone_close("call", None).unwrap().as_deref(), Some("surviving-mic"));
        assert!(Arc::ptr_eq(&registry.close("surviving-mic").unwrap(), &input));
        assert!(!asr.is_cancelled());
        registry.reserve("next".into(), "call".into(), "1".into()).unwrap();
    }
    #[test]
    fn call_close_revokes_configuration_and_an_explicit_start_not_yet_polled() {
        let mut registry = Registry::default();
        let (pending, _, _) = registry.reserve("configuring".into(), "call".into(), "1".into()).unwrap();
        assert_eq!(registry.prepare_microphone_close("call", None).unwrap().as_deref(), Some("configuring"));
        assert!(pending.is_cancelled());
        assert!(!registry.inputs.contains_key("configuring"));
        assert!(!registry.allows_start("configuring"));
        assert_eq!(registry.prepare_microphone_close("call", Some("not-polled")).unwrap().as_deref(), Some("not-polled"));
        assert!(registry.reserve("not-polled".into(), "call".into(), "1".into()).is_err());
        assert!(registry.reserve("configuring".into(), "call".into(), "1".into()).is_err());
        registry.reserve("next".into(), "call".into(), "1".into()).unwrap();
    }
    #[test]
    fn delayed_close_never_retargets_a_new_microphone_or_a_different_call() {
        let mut registry = Registry::default();
        let old = install(&mut registry, "old", "call");
        registry.close("old");
        let next = install(&mut registry, "next", "call");
        registry.prepare_microphone_close("call", Some("old")).unwrap();
        assert!(registry.allows_start("next"));
        assert_eq!(registry.active.as_deref(), Some("next"));
        registry.close("next");
        let other = install(&mut registry, "other", "other-call");
        assert!(registry.prepare_microphone_close("call", None).unwrap().is_none());
        assert!(registry.prepare_microphone_close("call", Some("other")).is_err());
        assert!(registry.allows_start("other"));
        assert_eq!(registry.active.as_deref(), Some("other"));
        assert!(!old.cancel.is_cancelled());
        assert!(!next.cancel.is_cancelled());
        assert!(!other.cancel.is_cancelled());
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
            .reserve("late".into(), "first-call".into(), "1".into())
            .is_err());
    }

    #[test]
    fn document_loss_revokes_configuration_before_native_capture_exists() {
        let documents = VoiceDocumentState::default();
        let old = documents.allocate().unwrap();
        documents.adopt(&old, || Ok(())).unwrap();
        let mut registry = Registry::default();
        let (asr, capture, call) = documents.with_current(&old, || {
            registry.reserve("configuring".into(), "call".into(), old.clone())
        }).unwrap();
        documents.invalidate(Some(&old)).unwrap();
        let closes = registry.prepare_document_close(&old, false).unwrap();
        assert_eq!(closes.len(), 1);
        assert_eq!(closes[0].session_id, "configuring");
        assert!(capture.is_cancelled());
        assert!(asr.is_cancelled()); // No accepted audio exists before configuration.
        assert!(!call.cancel.is_cancelled());
        assert!(registry.active.is_none());
        let next = documents.allocate().unwrap();
        documents.adopt(&next, || Ok(())).unwrap();
        assert!(documents.with_current(&old, || {
            registry.reserve("late".into(), "call".into(), old.clone())
        }).is_err());
        let (_, _, resumed) = documents.with_current(&next, || {
            registry.reserve("new".into(), "call".into(), next.clone())
        }).unwrap();
        assert!(Arc::ptr_eq(&call, &resumed));
    }

    #[test]
    fn document_close_keeps_accepted_asr_and_shares_the_real_stop_barrier() {
        let mut registry = Registry::default();
        let input = install(&mut registry, "old", "call");
        let accepted_asr = input.cancel.child_token();
        let first = registry.prepare_document_close("1", false).unwrap().pop().unwrap();
        let retry = registry.prepare_document_close("1", false).unwrap().pop().unwrap();
        assert!(Arc::ptr_eq(&first.serial, &retry.serial));
        assert!(input.capture_cancel.is_cancelled());
        assert!(!accepted_asr.is_cancelled());
        assert!(!input.call.cancel.is_cancelled());
        assert!(registry.reserve("new".into(), "call".into(), "2".into()).is_err());
        let barrier = first.serial.try_lock().unwrap();
        assert!(retry.serial.try_lock().is_err());
        drop(barrier);
        registry.close("old"); // The same transition used after a real stop receipt.
        input.input_closed.store(true, Ordering::Release);
        registry.reserve("new".into(), "call".into(), "2".into()).unwrap();
        assert!(registry.prepare_document_close("1", false).unwrap().is_empty());
        assert!(registry.allows_start("new"));
        assert!(!accepted_asr.is_cancelled());
    }

    #[test]
    fn unpolled_old_page_close_cannot_select_the_new_pages_microphone() {
        let documents = VoiceDocumentState::default();
        let old = documents.allocate().unwrap();
        documents.adopt(&old, || Ok(())).unwrap();
        let next = documents.allocate().unwrap();
        documents.adopt(&next, || Ok(())).unwrap();
        let mut registry = Registry::default();
        registry.reserve("current".into(), "call".into(), next.clone()).unwrap();
        assert!(documents.with_current(&old, || registry.prepare_close("call", None)).is_err());
        assert!(registry.allows_start("current"));
        let close = documents.with_current(&next, || registry.prepare_close("call", None)).unwrap().unwrap();
        assert_eq!(close.session_id, "current");
    }
}

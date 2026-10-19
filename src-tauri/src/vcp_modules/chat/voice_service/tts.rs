//! Independent narration job. There is no microphone or chat cancellation parent.
use super::{
    chat_feed::ChatFeed,
    network,
    segmentation::{ReplyBuffer, ReplyPiece},
    VoiceServiceConfig, VoiceServiceStart,
};
use crate::vcp_modules::{
    settings_manager::{read_settings, SettingsState},
    vcp_client::StreamEvent,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::Serialize;
use std::{
    collections::VecDeque,
    future::Future,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

const TTS_EVENT: &str = "vcp-tts";

struct TtsJob {
    id: String,
    origin: Option<String>,
    config: VoiceServiceConfig,
    cancel: CancellationToken,
    reply: Mutex<ReplyBuffer>,
    chat: Mutex<ChatFeed>,
    ready: Notify,
    worker_started: AtomicBool,
    player_started: AtomicBool,
    terminal: AtomicBool,
    stop_lock: tokio::sync::Mutex<()>,
    work: Mutex<Option<Arc<super::call::WorkLease>>>,
}

struct Slot {
    id: String,
    cancel: CancellationToken,
    job: Option<Arc<TtsJob>>,
}
#[derive(Default)]
struct Registry {
    current: Option<Slot>,
    retired: VecDeque<String>,
}
#[derive(Default)]
pub struct TtsServiceState(Mutex<Registry>);

impl Registry {
    fn reserve(&mut self, id: String) -> Result<CancellationToken, String> {
        if self.retired.contains(&id) {
            return Err(network::CANCELLED.into());
        }
        if self.current.is_some() {
            return Err("仍有朗读正在进行，请先停止当前朗读。".into());
        }
        let cancel = CancellationToken::new();
        self.current = Some(Slot {
            id,
            cancel: cancel.clone(),
            job: None,
        });
        Ok(cancel)
    }

    fn admit_player(&self, job: &Arc<TtsJob>) -> Result<(), String> {
        if job.cancel.is_cancelled()
            || self
                .current
                .as_ref()
                .and_then(|slot| slot.job.as_ref())
                .is_none_or(|current| !Arc::ptr_eq(current, job))
        {
            return Err(network::CANCELLED.into());
        }
        job.player_started.store(true, Ordering::Release);
        Ok(())
    }

    fn complete(&mut self, id: &str) {
        if self.current.as_ref().is_some_and(|slot| slot.id == id) {
            self.current = None;
            self.remember(id);
        }
    }

    fn remember(&mut self, id: &str) {
        if self.retired.iter().any(|old| old == id) {
            return;
        }
        if self.retired.len() == 128 {
            self.retired.pop_front();
        }
        self.retired.push_back(id.into());
    }
    fn stop(&mut self, id: &str) -> Option<Arc<TtsJob>> {
        self.remember(id);
        if self.current.as_ref().is_none_or(|slot| slot.id != id) {
            return None;
        }
        let slot = self.current.as_ref()?;
        slot.cancel.cancel();
        let job = slot.job.clone();
        // Native stop must acknowledge its tombstone before a new job starts.
        if job.is_none() {
            self.current = None;
        }
        job
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TtsEvent {
    tts_id: String,
    kind: &'static str,
    stage: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

fn emit<R: Runtime>(
    app: &AppHandle<R>,
    job: &TtsJob,
    kind: &'static str,
    stage: &'static str,
    message: Option<String>,
) {
    super::call::tts_status(app, &job.id, stage, message.as_deref());
    let _ = app.emit(
        TTS_EVENT,
        TtsEvent {
            tts_id: job.id.clone(),
            kind,
            stage,
            message,
        },
    );
}

fn job<R: Runtime>(app: &AppHandle<R>, id: &str) -> Option<Arc<TtsJob>> {
    app.state::<TtsServiceState>()
        .0
        .lock()
        .ok()?
        .current
        .as_ref()
        .filter(|slot| slot.id == id)?
        .job
        .clone()
}

pub fn has_chat_listener<R: Runtime>(app: &AppHandle<R>, origin: &str) -> bool {
    let Some(state) = app.try_state::<TtsServiceState>() else {
        return false;
    };
    let listening = state.0.lock().is_ok_and(|registry| {
        registry
            .current
            .as_ref()
            .and_then(|slot| slot.job.as_ref())
            .is_some_and(|job| {
                job.origin.as_deref() == Some(origin)
                    && !job.cancel.is_cancelled()
                    && !job.terminal.load(Ordering::Acquire)
            })
    });
    listening
}

fn retire<R: Runtime>(app: &AppHandle<R>, job: &TtsJob) {
    let state = app.state::<TtsServiceState>();
    if let Ok(mut registry) = state.0.lock() {
        registry.complete(&job.id);
    };
    if let Ok(mut work) = job.work.lock() {
        work.take();
    }
}

pub(super) fn attach_work<R: Runtime>(
    app: &AppHandle<R>,
    id: &str,
    work: Arc<super::call::WorkLease>,
) {
    if let Some(job) = job(app, id) {
        if let Ok(mut held) = job.work.lock() {
            *held = Some(work);
        }
    }
}

/// Revoke admission immediately; native stop acknowledgement runs off the
/// native event callback and never blocks confirmed speech processing.
pub(super) fn request_stop<R: Runtime>(app: &AppHandle<R>, id: &str) {
    let narration = app
        .state::<TtsServiceState>()
        .0
        .lock()
        .ok()
        .and_then(|mut registry| registry.stop(id));
    if let Some(narration) = narration {
        if !narration.terminal.swap(true, Ordering::AcqRel) {
            emit(app, &narration, "stopped", "cancelled", None);
        }
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            if flush_player(&app, &narration).await.is_ok() {
                retire(&app, &narration);
            }
        });
    }
}

async fn flush_player<R: Runtime>(app: &AppHandle<R>, job: &TtsJob) -> Result<(), String> {
    let _stop = job.stop_lock.lock().await;
    if job.player_started.swap(false, Ordering::AcqRel) {
        if let Err(error) =
            tauri_plugin_vcp_mobile::voice::stop_playback(app.clone(), job.id.clone()).await
        {
            job.player_started.store(true, Ordering::Release);
            return Err(error);
        }
    }
    Ok(())
}

fn fail<R: Runtime>(app: &AppHandle<R>, job: &Arc<TtsJob>, message: String) {
    if job.terminal.swap(true, Ordering::AcqRel) {
        return;
    }
    if let Ok(mut registry) = app.state::<TtsServiceState>().0.lock() {
        registry.stop(&job.id);
    }
    emit(app, job, "error", "tts", Some(message));
    let app = app.clone();
    let job = job.clone();
    tauri::async_runtime::spawn(async move {
        if flush_player(&app, &job).await.is_ok() {
            retire(&app, &job);
        }
    });
}

fn wake<R: Runtime>(app: &AppHandle<R>, job: &Arc<TtsJob>) {
    job.ready.notify_one();
    if !job.worker_started.swap(true, Ordering::AcqRel) {
        tauri::async_runtime::spawn(speak(app.clone(), job.clone()));
    }
}

#[tauri::command]
pub async fn start_tts_session<R: Runtime>(
    app: AppHandle<R>,
    tts_id: String,
    origin_user_message_id: Option<String>,
) -> Result<VoiceServiceStart, String> {
    if tts_id.is_empty()
        || tts_id.len() > 128
        || !tts_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err("朗读任务标识无效。".into());
    }
    if origin_user_message_id
        .as_ref()
        .is_some_and(|id| id.is_empty() || id.len() > 128)
    {
        return Err("朗读来源标识无效。".into());
    }
    let cancel = {
        let state = app.state::<TtsServiceState>();
        let mut registry = state.0.lock().map_err(|error| error.to_string())?;
        registry.reserve(tts_id.clone())?
    };
    let configured = read_settings(app.clone(), app.state::<SettingsState>())
        .await
        .map(VoiceServiceConfig::from_settings)
        .and_then(|config| {
            config.validate()?;
            Ok(config)
        });
    if cancel.is_cancelled() {
        return Err(network::CANCELLED.into());
    }
    let config = match configured {
        Ok(config) => config,
        Err(error) => {
            let state = app.state::<TtsServiceState>();
            let mut registry = state.0.lock().map_err(|lock| lock.to_string())?;
            if !cancel.is_cancelled() {
                registry.stop(&tts_id);
            }
            return Err(error);
        }
    };
    let configured = config.configured();
    if !configured {
        let state = app.state::<TtsServiceState>();
        let mut registry = state.0.lock().map_err(|error| error.to_string())?;
        if !cancel.is_cancelled() {
            registry.stop(&tts_id);
        }
        return Ok(VoiceServiceStart { configured: false });
    }
    let narration = Arc::new(TtsJob {
        id: tts_id.clone(),
        work: Mutex::new(
            origin_user_message_id
                .is_none()
                .then(|| super::call::WorkLease::acquire(&app)),
        ),
        origin: origin_user_message_id,
        config,
        cancel: cancel.clone(),
        reply: Mutex::new(ReplyBuffer::default()),
        chat: Mutex::new(ChatFeed::default()),
        ready: Notify::new(),
        worker_started: AtomicBool::new(false),
        player_started: AtomicBool::new(false),
        terminal: AtomicBool::new(false),
        stop_lock: tokio::sync::Mutex::new(()),
    });
    {
        let state = app.state::<TtsServiceState>();
        let mut registry = state.0.lock().map_err(|error| error.to_string())?;
        let slot = registry
            .current
            .as_mut()
            .filter(|slot| slot.id == tts_id && !cancel.is_cancelled())
            .ok_or_else(|| network::CANCELLED.to_string())?;
        slot.job = Some(narration.clone());
    }
    emit(&app, &narration, "status", "ready", None);
    Ok(VoiceServiceStart { configured })
}

#[tauri::command]
pub async fn submit_tts_text<R: Runtime>(
    app: AppHandle<R>,
    tts_id: String,
    text: String,
    finished: bool,
) -> Result<(), String> {
    let narration = job(&app, &tts_id)
        .filter(|job| !job.cancel.is_cancelled())
        .ok_or_else(|| network::CANCELLED.to_string())?;
    if !narration.config.configured() {
        return Err("请先配置语音 API Key。".into());
    }
    let updated = narration
        .reply
        .lock()
        .map_err(|error| error.to_string())?
        .update(text, finished);
    if let Err(error) = updated {
        fail(&app, &narration, error.clone());
        return Err(error);
    }
    wake(&app, &narration);
    Ok(())
}

#[tauri::command]
pub async fn stop_tts<R: Runtime>(app: AppHandle<R>, tts_id: String) -> Result<(), String> {
    let narration = app
        .state::<TtsServiceState>()
        .0
        .lock()
        .map_err(|error| error.to_string())?
        .stop(&tts_id);
    if let Some(narration) = narration {
        if !narration.terminal.swap(true, Ordering::AcqRel) {
            emit(&app, &narration, "stopped", "cancelled", None);
        }
        flush_player(&app, &narration).await?;
        retire(&app, &narration);
    }
    Ok(())
}

/// Seal an empty chat attempt or a composed invite-only workflow without
/// replacing the cumulative text supplied by the real Rust output owner.
pub async fn finish_tts_chat<R: Runtime>(app: AppHandle<R>, tts_id: String) -> Result<(), String> {
    let Some(narration) = job(&app, &tts_id).filter(|job| !job.cancel.is_cancelled()) else {
        return Ok(());
    };
    if narration.origin.is_none() {
        return Err("这不是聊天朗读任务。".into());
    }
    narration
        .chat
        .lock()
        .map_err(|error| error.to_string())?
        .finish_submission();
    wake(&app, &narration);
    Ok(())
}

pub async fn stop_chat_reply_by_origin<R: Runtime>(
    app: &AppHandle<R>,
    origin: &str,
) -> Result<(), String> {
    let Some(state) = app.try_state::<TtsServiceState>() else {
        return Ok(());
    };
    let id = state
        .0
        .lock()
        .map_err(|error| error.to_string())?
        .current
        .as_ref()
        .and_then(|slot| slot.job.as_ref())
        .filter(|job| job.origin.as_deref() == Some(origin))
        .map(|job| job.id.clone());
    if let Some(id) = id {
        stop_tts(app.clone(), id).await?;
    }
    Ok(())
}

pub fn feed_chat_event<R: Runtime>(app: &AppHandle<R>, event: &StreamEvent) {
    let Some(origin) = event
        .context
        .as_ref()
        .and_then(|context| context["originUserMessageId"].as_str())
    else {
        return;
    };
    let Some(state) = app.try_state::<TtsServiceState>() else {
        return;
    };
    let narration = state.0.lock().ok().and_then(|registry| {
        registry
            .current
            .as_ref()
            .and_then(|slot| slot.job.as_ref())
            .filter(|job| {
                job.origin.as_deref() == Some(origin)
                    && job.config.configured()
                    && !job.cancel.is_cancelled()
            })
            .cloned()
    });
    let Some(narration) = narration else {
        return;
    };
    let result = narration
        .chat
        .lock()
        .map_err(|_| "朗读状态已失效。".to_string())
        .and_then(|mut feed| feed.accept(event));
    match result {
        Ok(true) => wake(app, &narration),
        Ok(false) => {}
        Err(error) => fail(app, &narration, error),
    }
}

async fn playback_call<F: Future<Output = Result<(), String>>>(
    cancel: &CancellationToken,
    call: F,
) -> Result<(), String> {
    tokio::select! { _=cancel.cancelled()=>Err(network::CANCELLED.into()), result=call=>result.map_err(|_|"语音播放失败，请重新发起朗读。".into()) }
}

async fn speak<R: Runtime>(app: AppHandle<R>, narration: Arc<TtsJob>) {
    let result = speak_inner(&app, &narration).await;
    match result {
        Ok(()) => {
            if !narration.terminal.swap(true, Ordering::AcqRel) {
                retire(&app, &narration);
                emit(&app, &narration, "stopped", "finished", None);
            }
        }
        Err(error) if !narration.cancel.is_cancelled() => fail(&app, &narration, error),
        Err(_) => {}
    }
    if narration.cancel.is_cancelled() {
        let _ = flush_player(&app, &narration).await;
    }
}

async fn speak_inner<R: Runtime>(
    app: &AppHandle<R>,
    narration: &Arc<TtsJob>,
) -> Result<(), String> {
    let mut started = false;
    let first_audio = Arc::new(AtomicBool::new(true));
    loop {
        if narration.cancel.is_cancelled() {
            return Err(network::CANCELLED.into());
        }
        let notified = narration.ready.notified();
        let update = narration
            .chat
            .lock()
            .map_err(|error| error.to_string())?
            .take_update();
        if let Some((text, finished)) = update {
            narration
                .reply
                .lock()
                .map_err(|error| error.to_string())?
                .update(text, finished)?;
        }
        let piece = narration
            .reply
            .lock()
            .map_err(|error| error.to_string())?
            .next();
        match piece {
            ReplyPiece::Wait => {
                tokio::select! {_=narration.cancel.cancelled()=>return Err(network::CANCELLED.into()), _=notified=>{}}
            }
            ReplyPiece::Finished => {
                if started {
                    playback_call(
                        &narration.cancel,
                        tauri_plugin_vcp_mobile::voice::finish_playback(
                            app.clone(),
                            narration.id.clone(),
                        ),
                    )
                    .await?;
                    narration.player_started.store(false, Ordering::Release);
                }
                return Ok(());
            }
            ReplyPiece::Text(text) => {
                if !started {
                    // Mark before the async native admission; stop can create its
                    // tombstone even if native start has not been dispatched yet.
                    {
                        let state = app.state::<TtsServiceState>();
                        let registry = state.0.lock().map_err(|error| error.to_string())?;
                        registry.admit_player(narration)?;
                    }
                    playback_call(
                        &narration.cancel,
                        tauri_plugin_vcp_mobile::voice::start_playback(
                            app.clone(),
                            narration.id.clone(),
                        ),
                    )
                    .await?;
                    started = true;
                    emit(app, narration, "status", "synthesizing", None);
                }
                network::synthesize(&narration.config, &text, &narration.cancel, |pcm| {
                    let app = app.clone();
                    let narration = narration.clone();
                    let first_audio = first_audio.clone();
                    async move {
                        playback_call(
                            &narration.cancel,
                            tauri_plugin_vcp_mobile::voice::append_playback(
                                app.clone(),
                                narration.id.clone(),
                                STANDARD.encode(pcm),
                            ),
                        )
                        .await?;
                        if first_audio.swap(false, Ordering::AcqRel) {
                            emit(&app, &narration, "status", "speaking", None);
                        }
                        Ok(())
                    }
                })
                .await?;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn install(registry: &mut Registry, id: &str) -> Arc<TtsJob> {
        let cancel = registry.reserve(id.into()).unwrap();
        let job = Arc::new(TtsJob {
            id: id.into(),
            origin: Some("origin".into()),
            config: VoiceServiceConfig {
                base_url: "https://example.invalid/v1".into(),
                api_key: "test".into(),
                asr_stream: true,
                input_encoding: Default::default(),
                tts_stream: true,
                language: "auto".into(),
                voice: "mimo_default".into(),
            },
            cancel,
            reply: Mutex::new(ReplyBuffer::default()),
            chat: Mutex::new(ChatFeed::default()),
            ready: Notify::new(),
            worker_started: AtomicBool::new(false),
            player_started: AtomicBool::new(false),
            terminal: AtomicBool::new(false),
            stop_lock: tokio::sync::Mutex::new(()),
            work: Mutex::new(None),
        });
        registry.current.as_mut().unwrap().job = Some(job.clone());
        job
    }
    #[test]
    fn stopping_reserves_the_output_until_native_tombstone_ack_and_rejects_late_admission() {
        let mut registry = Registry::default();
        let old = install(&mut registry, "old");
        registry.admit_player(&old).unwrap();
        assert!(old.player_started.load(Ordering::Acquire));
        registry.stop("old").unwrap();
        assert!(old.cancel.is_cancelled());
        assert!(registry.reserve("new".into()).is_err());
        assert!(registry.admit_player(&old).is_err());
        // Production completes this phase only after flush_player acknowledges.
        registry.complete("old");
        let new = install(&mut registry, "new");
        assert!(registry.admit_player(&old).is_err());
        registry.stop("old");
        registry.complete("old");
        assert!(!new.cancel.is_cancelled());
        registry.admit_player(&new).unwrap();
    }
    #[test]
    fn unconfigured_or_cancelled_pending_setup_does_not_keep_a_busy_slot() {
        let mut registry = Registry::default();
        let pending = registry.reserve("unconfigured".into()).unwrap();
        assert!(registry.stop("unconfigured").is_none());
        assert!(pending.is_cancelled());
        registry.reserve("next".into()).unwrap();
        assert!(registry.reserve("unconfigured".into()).is_err());
    }
}

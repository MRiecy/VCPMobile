//! Independent narration job. There is no microphone or chat cancellation parent.
use super::{
    chat_feed::{ChatFeed, MessageSpeech, SpeechPiece},
    network, samples,
    segmentation::{ReplyBuffer, ReplyPiece},
    VoiceReplyPolicy, VoiceServiceConfig, VoiceServiceStart,
};
use crate::vcp_modules::{
    settings_manager::{
        Settings, VoiceConfig, VoiceConnectionConfig, VoiceTtsConfig,
        VoiceTtsMode,
    },
    topic_types::TopicKey,
    vcp_client::StreamEvent,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
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
    sample: Option<samples::PreparedCloneSample>,
    cancel: CancellationToken,
    input: Mutex<NarrationInput>,
    ready: Notify,
    worker_started: AtomicBool,
    player_started: AtomicBool,
    terminal: AtomicBool,
    stop_lock: tokio::sync::Mutex<()>,
    work: Mutex<Option<Arc<super::call::WorkLease>>>,
}

enum NarrationInput {
    Chat(ChatFeed),
    Bubble(MessageSpeech),
    Preview(ReplyBuffer),
}

impl NarrationInput {
    fn submit_bubble(&mut self, text: &str, finished: bool) -> Result<(), String> {
        let Self::Bubble(message) = self else {
            return Err("此任务不接收气泡朗读文本。".into());
        };
        message.update(text, finished)?;
        if finished && !message.has_spoken_text() {
            return Err("此消息没有可朗读的正文内容。".into());
        }
        Ok(())
    }

    fn submit_preview(&mut self, text: String) -> Result<(), String> {
        let Self::Preview(reply) = self else {
            return Err("此任务不是试听。".into());
        };
        reply.update(text, true)
    }
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BubbleSource {
    topic: TopicKey,
    message_id: String,
}

impl BubbleSource {
    async fn read<R: Runtime>(&self, app: &AppHandle<R>) -> Result<String, String> {
        crate::vcp_modules::message_service::fetch_raw_message_content(
            app.clone(),
            self.topic.owner_id.clone(),
            self.topic.owner_type.clone(),
            self.topic.topic_id.clone(),
            self.message_id.clone(),
        ).await
    }
}

#[derive(Clone)]
enum NarrationKind {
    Chat,
    Bubble(BubbleSource),
    Preview,
}

impl NarrationKind {
    fn validate_admission(&self, call_active: bool) -> Result<(), String> {
        match self {
            Self::Bubble(_) if call_active => {
                Err("语音通话进行中，请先结束通话再单独朗读气泡。".into())
            }
            Self::Preview if call_active => Err("通话结束后可试听声音。".into()),
            _ => Ok(()),
        }
    }
}

struct Slot {
    id: String,
    cancel: CancellationToken,
    job: Option<Arc<TtsJob>>,
    kind: NarrationKind,
}
#[derive(Default)]
struct Registry {
    current: Option<Slot>,
    retired: VecDeque<String>,
}
#[derive(Default)]
pub struct TtsServiceState(Mutex<Registry>);

impl Registry {
    fn reserve(&mut self, id: String, kind: NarrationKind) -> Result<CancellationToken, String> {
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
            kind,
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

pub(super) struct PreparedNarration {
    pub(super) policy: Arc<VoiceReplyPolicy>,
    sample: Result<Option<samples::PreparedCloneSample>, String>,
}

/// Sample retirement and the settings read share the sample lock. A settings
/// change cannot delete the sample selected by this immutable reply snapshot.
pub(super) async fn prepare_narration<R: Runtime>(
    app: &AppHandle<R>,
    cancel: &CancellationToken,
) -> Result<PreparedNarration, String> {
    let snapshot = samples::snapshot_voice_settings(app, cancel).await?;
    Ok(PreparedNarration {
        policy: Arc::new(VoiceReplyPolicy::from_settings(&snapshot.settings)),
        sample: snapshot.sample,
    })
}

enum NarrationSource {
    Stored,
    Prepared(PreparedNarration),
    Preview(Arc<VoiceReplyPolicy>),
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TtsPreviewSettings {
    connection: VoiceConnectionConfig,
    tts: VoiceTtsConfig,
}

impl TtsPreviewSettings {
    fn into_policy(self) -> Arc<VoiceReplyPolicy> {
        Arc::new(VoiceReplyPolicy::from_settings(&Settings {
            voice: VoiceConfig {
                connection: self.connection,
                tts: self.tts,
                ..Default::default()
            },
            ..Settings::default()
        }))
    }
}

#[tauri::command]
pub async fn preview_tts<R: Runtime>(
    app: AppHandle<R>,
    tts_id: String,
    text: String,
    settings: TtsPreviewSettings,
) -> Result<VoiceServiceStart, String> {
    let text = text.trim();
    if text.is_empty() || text.chars().count() > 500 {
        return Err("试听文本需要包含 1–500 个字符。".into());
    }
    let started = start_narration(
        app.clone(),
        tts_id.clone(),
        None,
        NarrationSource::Preview(settings.into_policy()),
        None,
    )
    .await?;
    if !started.configured {
        return Err("请先配置语音 API Key。".into());
    }
    if super::call::is_active(&app)? {
        stop_tts(app, tts_id).await?;
        return Err("通话结束后可试听声音。".into());
    }
    if let Err(error) = submit_preview_text(&app, &tts_id, text.to_string()) {
        let _ = stop_tts(app, tts_id).await;
        return Err(error);
    }
    Ok(started)
}

/// Calls retire only independent message narration. Preview belongs to its page.
pub(super) async fn stop_bubble_narration<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let id = app
        .state::<TtsServiceState>()
        .0
        .lock()
        .map_err(|e| e.to_string())?
        .current
        .as_ref()
        .filter(|slot| matches!(&slot.kind, NarrationKind::Bubble(_)))
        .map(|slot| slot.id.clone());
    if let Some(id) = id {
        stop_tts(app.clone(), id).await?;
    }
    Ok(())
}

/// Mutation owners pass the committed affected IDs, including off-screen messages.
pub(crate) async fn stop_bubble_for_messages<R: Runtime>(
    app: &AppHandle<R>,
    topic: &TopicKey,
    message_ids: &[String],
) -> Result<(), String> {
    let Some(state) = app.try_state::<TtsServiceState>() else {
        return Ok(());
    };
    let id = state.0.lock().map_err(|error| error.to_string())?
        .current.as_ref().filter(|slot| matches!(&slot.kind,
            NarrationKind::Bubble(source) if &source.topic == topic
                && message_ids.contains(&source.message_id)))
        .map(|slot| slot.id.clone());
    if let Some(id) = id {
        stop_tts(app.clone(), id).await?;
    }
    Ok(())
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
    bubble_source: Option<BubbleSource>,
) -> Result<VoiceServiceStart, String> {
    start_narration(app, tts_id, origin_user_message_id, NarrationSource::Stored, bubble_source).await
}

pub(super) async fn start_prepared_narration<R: Runtime>(
    app: AppHandle<R>,
    tts_id: String,
    origin_user_message_id: String,
    prepared: PreparedNarration,
) -> Result<VoiceServiceStart, String> {
    start_narration(
        app,
        tts_id,
        Some(origin_user_message_id),
        NarrationSource::Prepared(prepared),
        None,
    )
    .await
}

async fn start_narration<R: Runtime>(
    app: AppHandle<R>,
    tts_id: String,
    origin_user_message_id: Option<String>,
    source: NarrationSource,
    bubble_source: Option<BubbleSource>,
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
    let kind = if matches!(&source, NarrationSource::Preview(_)) {
        NarrationKind::Preview
    } else if origin_user_message_id.is_some() {
        NarrationKind::Chat
    } else {
        let source = bubble_source.filter(|source| source.topic.is_valid()
            && !source.message_id.is_empty() && source.message_id.len() <= 128)
            .ok_or("朗读来源消息无效。")?;
        source.read(&app).await?;
        NarrationKind::Bubble(source)
    };
    let cancel = {
        let state = app.state::<TtsServiceState>();
        let mut registry = state.0.lock().map_err(|error| error.to_string())?;
        // A call reserves itself before stopping the current bubble. Holding
        // the TTS slot lock around this check closes the opposite admission order.
        kind.validate_admission(super::call::is_active(&app)?)?;
        registry.reserve(tts_id.clone(), kind.clone())?
    };
    let work = origin_user_message_id
        .is_none()
        .then(|| super::call::WorkLease::acquire(&app));
    let prepared = match source {
        NarrationSource::Stored => prepare_narration(&app, &cancel).await,
        NarrationSource::Prepared(prepared) => Ok(prepared),
        NarrationSource::Preview(policy) => {
            let sample =
                if policy.config.tts_mode == VoiceTtsMode::Clone && policy.config.configured() {
                    samples::prepare_clone_sample(&app, &policy.config.clone_sample_id, &cancel)
                        .await
                        .map(Some)
                } else {
                    Ok(None)
                };
            Ok(PreparedNarration { policy, sample })
        }
    };
    let prepared = prepared.and_then(|prepared| {
        prepared.policy.config.validate_tts()?;
        kind.validate_admission(super::call::is_active(&app)?)?;
        let sample = prepared.sample?;
        Ok((prepared.policy, sample))
    });
    if cancel.is_cancelled() {
        return Err(network::CANCELLED.into());
    }
    let (policy, sample) = match prepared {
        Ok(prepared) => prepared,
        Err(error) => {
            let state = app.state::<TtsServiceState>();
            let mut registry = state.0.lock().map_err(|lock| lock.to_string())?;
            if !cancel.is_cancelled() {
                registry.stop(&tts_id);
            }
            return Err(error);
        }
    };
    let config = policy.config.clone();
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
        work: Mutex::new(work),
        origin: origin_user_message_id,
        input: Mutex::new(match kind {
            NarrationKind::Chat => NarrationInput::Chat(ChatFeed::new(config.tts_streaming())),
            NarrationKind::Bubble(_) => NarrationInput::Bubble(MessageSpeech::new(config.tts_streaming())),
            NarrationKind::Preview => NarrationInput::Preview(ReplyBuffer::new_with_mode(false, config.tts_streaming())),
        }),
        config,
        sample,
        cancel: cancel.clone(),
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
    let source = {
        let state = app.state::<TtsServiceState>();
        let registry = state.0.lock().map_err(|error| error.to_string())?;
        registry.current.as_ref().filter(|slot| slot.id == tts_id)
            .and_then(|slot| match &slot.kind {
                NarrationKind::Bubble(source) => Some(source.clone()),
                _ => None,
            }).ok_or("此任务不接收气泡朗读文本。")?
    };
    let updated = async {
        // Reading may finish after an edit/delete. Only the persisted source
        // snapshot selected by this bubble can be admitted for playback.
        if source.read(&app).await? != text {
            return Err("消息内容已变化，请重新发起朗读。".into());
        }
        if narration.cancel.is_cancelled() {
            return Err(network::CANCELLED.into());
        }
        narration.input.lock().map_err(|error| error.to_string())?
            .submit_bubble(&text, finished)
    }.await;
    if let Err(error) = updated {
        fail(&app, &narration, error.clone());
        stop_tts(app, tts_id).await?;
        return Err(error);
    }
    wake(&app, &narration);
    Ok(())
}

fn submit_preview_text<R: Runtime>(
    app: &AppHandle<R>,
    tts_id: &str,
    text: String,
) -> Result<(), String> {
    let narration = job(app, tts_id)
        .filter(|job| !job.cancel.is_cancelled())
        .ok_or_else(|| network::CANCELLED.to_string())?;
    narration.input.lock().map_err(|error| error.to_string())?.submit_preview(text)?;
    wake(app, &narration);
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
    {
        let mut input = narration.input.lock().map_err(|error| error.to_string())?;
        let NarrationInput::Chat(feed) = &mut *input else {
            return Err("这不是聊天朗读任务。".into());
        };
        feed.finish_submission();
    }
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
    let result = narration.input.lock()
        .map_err(|_| "朗读状态已失效。".to_string())
        .and_then(|mut input| match &mut *input {
            NarrationInput::Chat(feed) => feed.accept(event),
            _ => Ok(false),
        });
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
    let mut speech_queue = VecDeque::<SpeechPiece>::new();
    let mut chat_finished = false;
    loop {
        if narration.cancel.is_cancelled() {
            return Err(network::CANCELLED.into());
        }
        let notified = narration.ready.notified();
        let (piece, guidance) = {
            let mut input = narration.input.lock().map_err(|error| error.to_string())?;
            match &mut *input {
                NarrationInput::Chat(feed) => {
                    if let Some(update) = feed.take_update()? {
                        speech_queue.extend(update.pieces);
                        chat_finished = update.finished;
                    }
                    if let Some(piece) = speech_queue.pop_front() {
                        log::debug!("[VoiceTts] synthesizing message={}", piece.message_id);
                        (ReplyPiece::Text(piece.text), piece.instruction)
                    } else if chat_finished {
                        (ReplyPiece::Finished, None)
                    } else {
                        (ReplyPiece::Wait, None)
                    }
                }
                NarrationInput::Bubble(message) => message.next(),
                NarrationInput::Preview(reply) => (reply.next(), None),
            }
        };
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
                network::synthesize(
                    &narration.config,
                    &text,
                    guidance.as_deref(),
                    narration.sample.as_ref(),
                    &narration.cancel,
                    |pcm| {
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
                    },
                )
                .await?;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn install(registry: &mut Registry, id: &str) -> Arc<TtsJob> {
        install_kind(registry, id, NarrationKind::Chat)
    }
    fn bubble_source() -> BubbleSource {
        BubbleSource { topic: TopicKey::new("agent", "a", "t"), message_id: "message".into() }
    }
    fn install_kind(registry: &mut Registry, id: &str, kind: NarrationKind) -> Arc<TtsJob> {
        let cancel = registry.reserve(id.into(), kind.clone()).unwrap();
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
                tts_mode: VoiceTtsMode::Builtin,
                clone_sample_id: String::new(),
            },
            sample: None,
            cancel,
            input: Mutex::new(match kind {
                NarrationKind::Chat => NarrationInput::Chat(ChatFeed::new(true)),
                NarrationKind::Bubble(_) => NarrationInput::Bubble(MessageSpeech::new(true)),
                NarrationKind::Preview => NarrationInput::Preview(ReplyBuffer::new_with_mode(false, true)),
            }),
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
        assert!(registry.reserve("new".into(), NarrationKind::Chat).is_err());
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
        let pending = registry.reserve("unconfigured".into(), NarrationKind::Chat).unwrap();
        assert!(registry.stop("unconfigured").is_none());
        assert!(pending.is_cancelled());
        registry.reserve("next".into(), NarrationKind::Chat).unwrap();
        assert!(registry.reserve("unconfigured".into(), NarrationKind::Chat).is_err());
    }

    #[tokio::test]
    async fn opening_a_call_cancels_pending_bubble_but_leaves_its_chat_narration() {
        let app = tauri::test::mock_app();
        app.manage(TtsServiceState::default());
        let state = app.state::<TtsServiceState>();
        let bubble = state
            .0
            .lock()
            .unwrap()
            .reserve("bubble".into(), NarrationKind::Bubble(bubble_source()))
            .unwrap();
        stop_bubble_narration(app.handle()).await.unwrap();
        assert!(bubble.is_cancelled());
        let independent = {
            let mut registry = state.0.lock().unwrap();
            assert!(registry.current.is_none());
            let job = install(&mut registry, "independent");
            job.cancel.clone()
        };
        stop_bubble_narration(app.handle()).await.unwrap();
        assert!(!independent.is_cancelled());
        assert_eq!(
            state.0.lock().unwrap().current.as_ref().unwrap().id,
            "independent"
        );
    }

    #[tokio::test]
    async fn failed_clone_preparation_releases_the_slot_for_a_later_reply() {
        let app = tauri::test::mock_app();
        app.manage(TtsServiceState::default());
        let policy = Arc::new(VoiceReplyPolicy::from_settings(&Settings {
            voice: VoiceConfig {
                connection: VoiceConnectionConfig {
                    api_key: "fixture-only-key".into(),
                    ..Default::default()
                },
                tts: VoiceTtsConfig {
                    mode: VoiceTtsMode::Clone,
                    clone_sample_id: "missing-reference".into(),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Settings::default()
        }));
        let prepared = PreparedNarration {
            policy,
            sample: Err("fixture missing sample".into()),
        };
        assert!(start_prepared_narration(
            app.handle().clone(),
            "missing".into(),
            "origin".into(),
            prepared
        )
        .await
        .is_err());
        let state = app.state::<TtsServiceState>();
        let mut registry = state.0.lock().unwrap();
        assert!(registry.current.is_none());
        assert!(registry.reserve("next-reply".into(), NarrationKind::Chat).is_ok());
    }

    #[test]
    fn bubble_input_projects_once_and_keeps_guidance_with_each_piece() {
        let mut input = NarrationInput::Bubble(MessageSpeech::new(false));
        let raw = format!("<voice_guidance>轻声</voice_guidance>\n(慵懒){}", "这是一句需要朗读的正文。".repeat(35));
        input.submit_bubble(&raw, true).unwrap();
        let NarrationInput::Bubble(message) = &mut input else { unreachable!() };
        let mut count = 0;
        loop {
            let (piece, instruction) = message.next();
            match piece {
                ReplyPiece::Text(text) => {
                    assert_eq!(instruction.as_deref(), Some("轻声"));
                    assert!(text.starts_with("(慵懒)"));
                    assert!(!text.contains("voice_guidance"));
                    count += 1;
                }
                ReplyPiece::Finished => break,
                ReplyPiece::Wait => panic!("complete bubble must drain"),
            }
        }
        assert!(count > 2);
    }

    #[test]
    fn preview_reads_literal_input_and_never_uses_the_bubble_projection() {
        let mut input = NarrationInput::Preview(ReplyBuffer::new_with_mode(false, true));
        let raw = "<voice_guidance>轻声</voice_guidance>照读";
        input.submit_preview(raw.into()).unwrap();
        assert!(input.submit_bubble(raw, true).is_err());
        let NarrationInput::Preview(reply) = &mut input else { unreachable!() };
        assert!(matches!(reply.next(), ReplyPiece::Text(text) if text == raw));
    }

    #[test]
    fn completed_empty_bubble_is_rejected_but_unclosed_content_is_read() {
        let mut input = NarrationInput::Bubble(MessageSpeech::new(true));
        assert!(input.submit_bubble("<think>不读</think>", true).is_err());
        let mut input = NarrationInput::Bubble(MessageSpeech::new(true));
        input.submit_bubble("<think>模型忘记闭合，这段内容仍然需要朗读。", true).unwrap();
        let NarrationInput::Bubble(message) = &mut input else { unreachable!() };
        assert!(matches!(message.next(), (ReplyPiece::Text(text), None) if text.contains("模型忘记闭合")));
    }

    #[test]
    fn active_call_rejects_bubble_admission_but_accepts_its_own_reply() {
        assert!(NarrationKind::Bubble(bubble_source()).validate_admission(true).is_err());
        assert!(NarrationKind::Bubble(bubble_source()).validate_admission(false).is_ok());
        assert!(NarrationKind::Chat.validate_admission(true).is_ok());
    }

    #[tokio::test]
    async fn mutations_stop_only_the_bubble_in_the_affected_topic_and_ids() {
        let app = tauri::test::mock_app();
        app.manage(TtsServiceState::default());
        let state = app.state::<TtsServiceState>();
        let source = bubble_source();
        let narration = install_kind(&mut state.0.lock().unwrap(), "bubble", NarrationKind::Bubble(source.clone()));
        stop_bubble_for_messages(app.handle(), &TopicKey::new("agent", "a", "other"), &[source.message_id.clone()]).await.unwrap();
        stop_bubble_for_messages(app.handle(), &source.topic, &["unrelated".into()]).await.unwrap();
        assert!(!narration.cancel.is_cancelled());
        stop_bubble_for_messages(app.handle(), &source.topic, &[source.message_id.clone()]).await.unwrap();
        assert!(narration.cancel.is_cancelled());
        assert!(state.0.lock().unwrap().current.is_none());
    }

    #[tokio::test]
    async fn deleted_source_cannot_start_after_mutation_missed_a_pending_frontend_read() {
        let app = tauri::test::mock_app();
        app.manage(TtsServiceState::default());
        let pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1)
            .connect("sqlite::memory:").await.unwrap();
        sqlx::query("CREATE TABLE messages (owner_type TEXT, owner_id TEXT, topic_id TEXT, msg_id TEXT, content TEXT, deleted_at INTEGER);
            INSERT INTO messages VALUES ('agent', 'a', 't', 'message', '原文保留在软删除行', NULL);")
            .execute(&pool).await.unwrap();
        app.manage(crate::vcp_modules::db_manager::DbState::new(pool.clone(), Default::default()));
        let source = bubble_source();
        assert_eq!(source.read(app.handle()).await.unwrap(), "原文保留在软删除行");
        // The frontend has read the source, but no TTS slot exists yet.
        sqlx::query("UPDATE messages SET deleted_at = 1 WHERE msg_id = 'message'")
            .execute(&pool).await.unwrap();
        stop_bubble_for_messages(app.handle(), &source.topic, &[source.message_id.clone()]).await.unwrap();
        assert!(start_tts_session(app.handle().clone(), "late".into(), None, Some(source)).await.is_err());
        assert!(app.state::<TtsServiceState>().0.lock().unwrap().current.is_none());
    }
}

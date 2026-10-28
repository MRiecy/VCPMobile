//! Independent narration job. There is no microphone or chat cancellation parent.
use super::{
    audio_codec::{self, PCM_PACKET_BYTES},
    cache::{self, TtsCache},
    chat_feed::{ChatFeed, MessageSpeech, SpeechPiece},
    network, pipeline::{self, PacketConsumer, SegmentBuffer, SegmentSource}, samples,
    segmentation::{ReplyBuffer, ReplyPiece},
    VoiceReplyPolicy, VoiceServiceConfig, VoiceServiceStart,
};
use crate::vcp_modules::{
    settings_manager::{
        Settings, VoiceConfig, VoiceConnectionConfig, VoiceTtsConfig,
        VoiceTtsMode,
    },
    topic_types::{MessageKey, TopicKey},
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
    Sample(Option<Vec<u8>>),
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
    async fn author<R: Runtime>(&self, app: &AppHandle<R>) -> Result<Option<String>, String> {
        let db = app.state::<crate::vcp_modules::db_manager::DbState>();
        crate::vcp_modules::persistence::message_repository::read_narration_author(
            &db.pool, &MessageKey::new(self.topic.clone(), self.message_id.clone()),
        ).await
    }

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
    SamplePreview,
}

impl NarrationKind {
    fn validate_admission(&self, call_active: bool) -> Result<(), String> {
        match self {
            Self::Bubble(_) if call_active => {
                Err("语音通话进行中，请先结束通话再单独朗读气泡。".into())
            }
            Self::Preview | Self::SamplePreview if call_active => Err("通话结束后可试听声音。".into()),
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

/// Freeze the connection, guidance and inherited defaults once per reply.
/// The actual speaker selects and leases audio only when their message is read.
pub(super) async fn prepare_narration<R: Runtime>(
    app: &AppHandle<R>,
    cancel: &CancellationToken,
) -> Result<PreparedNarration, String> {
    let settings = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Err(network::CANCELLED.into()),
        settings = crate::vcp_modules::settings_manager::read_config(app.clone(), app.state()) => settings?,
    };
    Ok(PreparedNarration {
        policy: Arc::new(VoiceReplyPolicy::from_settings(&settings)),
        sample: Ok(None),
    })
}

struct PreparedVoice {
    config: VoiceServiceConfig,
    sample: Option<samples::PreparedCloneSample>,
}

async fn prepare_speaker<R: Runtime>(
    app: &AppHandle<R>,
    defaults: &VoiceServiceConfig,
    agent_id: Option<&str>,
    cancel: &CancellationToken,
) -> Result<PreparedVoice, String> {
    let mut config = if let Some(agent_id) = agent_id {
        let state = app.state::<crate::vcp_modules::agent_service::AgentConfigState>();
        let agent = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(network::CANCELLED.into()),
            agent = crate::vcp_modules::agent_service::read_agent_config_internal(app, &state, agent_id, Some(false)) => agent?,
        };
        defaults.with_agent_voice(&agent.voice)
    } else {
        defaults.clone()
    };
    let sample = if config.configured() && config.tts_mode == VoiceTtsMode::Clone {
        let sample = if config.clone_sample_id.is_empty() { None } else {
            samples::prepare_clone_sample(app, &config.clone_sample_id, cancel).await?
        };
        if sample.is_none() { config.tts_mode = VoiceTtsMode::None; }
        sample
    } else {
        None
    };
    // None is a resolved selection. The consumer decides whether that means a
    // skipped group member or an explicit error for independent narration.
    if config.tts_mode != VoiceTtsMode::None { config.validate_tts()?; }
    Ok(PreparedVoice { config, sample })
}

enum NarrationSource {
    Stored,
    Prepared(PreparedNarration),
    Preview(Arc<VoiceReplyPolicy>),
    Sample(String),
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

#[tauri::command]
pub async fn preview_voice_clone_sample<R: Runtime>(
    app: AppHandle<R>,
    tts_id: String,
    sample_id: String,
) -> Result<VoiceServiceStart, String> {
    let started = start_narration(
        app.clone(), tts_id.clone(), None, NarrationSource::Sample(sample_id), None,
    ).await?;
    if super::call::is_active(&app)? {
        stop_tts(app, tts_id).await?;
        return Err("通话结束后可试听声音。".into());
    }
    if let Some(narration) = job(&app, &tts_id) {
        wake(&app, &narration);
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
    // The job owns its lease until its final Arc is dropped. The registry holds
    // it through native stop acknowledgement; the worker holds it through IO
    // and decode cleanup. Neither can release the other's outstanding work.
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
    let kind = if matches!(&source, NarrationSource::Sample(_)) {
        NarrationKind::SamplePreview
    } else if matches!(&source, NarrationSource::Preview(_)) {
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
    let mut sample_audio = None;
    let prepared = match source {
        NarrationSource::Stored => {
            let prepared = prepare_narration(&app, &cancel).await;
            match (prepared, &kind) {
                (Ok(mut prepared), NarrationKind::Bubble(source)) => {
                    let voice = async {
                        let agent_id = source.author(&app).await?;
                        prepare_speaker(&app, &prepared.policy.config, agent_id.as_deref(), &cancel).await
                    }.await;
                    match voice {
                        Ok(voice) => {
                            Arc::make_mut(&mut prepared.policy).config = voice.config;
                            prepared.sample = Ok(voice.sample);
                            Ok(prepared)
                        }
                        Err(error) => Err(error),
                    }
                }
                (prepared, _) => prepared,
            }
        }
        NarrationSource::Prepared(prepared) => Ok(prepared),
        NarrationSource::Preview(policy) => {
            let sample =
                if policy.config.tts_mode == VoiceTtsMode::Clone && policy.config.configured() {
                    samples::prepare_clone_sample(&app, &policy.config.clone_sample_id, &cancel)
                        .await
                        .and_then(|sample| sample.map(Some).ok_or_else(|| "当前音色已不存在，请重新选择朗读音色。".into()))
                } else {
                    Ok(None)
                };
            Ok(PreparedNarration { policy, sample })
        }
        NarrationSource::Sample(sample_id) => {
            match samples::read_clone_sample_audio(&app, &sample_id, &cancel).await {
                Ok(Some((_, bytes))) => {
                    sample_audio = Some(bytes);
                    Ok(PreparedNarration {
                        policy: Arc::new(VoiceReplyPolicy::from_settings(&Settings::default())),
                        sample: Ok(None),
                    })
                }
                Ok(None) => Err("该声音模型已不存在，请重新选择。".into()),
                Err(error) => Err(error),
            }
        }
    };
    let prepared = prepared.and_then(|prepared| {
        if !matches!(&kind, NarrationKind::Chat | NarrationKind::SamplePreview) {
            prepared.policy.config.validate_tts()?;
        }
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
    let configured = matches!(&kind, NarrationKind::SamplePreview) || config.configured();
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
            NarrationKind::Chat => NarrationInput::Chat(ChatFeed::new()),
            NarrationKind::Bubble(_) => NarrationInput::Bubble(MessageSpeech::new()),
            NarrationKind::Preview => NarrationInput::Preview(ReplyBuffer::new(false)),
            NarrationKind::SamplePreview => NarrationInput::Sample(sample_audio),
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
    tokio::select! { biased; _=cancel.cancelled()=>Err(network::CANCELLED.into()), result=call=>result.map_err(|_|"语音播放失败，请重新发起朗读。".into()) }
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
    if narration.cancel.is_cancelled() || narration.terminal.load(Ordering::Acquire) {
        return Err(network::CANCELLED.into());
    }
    let (preview, sample_audio) = {
        let mut input = narration.input.lock().map_err(|error| error.to_string())?;
        match &mut *input {
            NarrationInput::Sample(audio) => (false, Some(audio.take().ok_or("原声试听已经结束。")?)),
            NarrationInput::Preview(_) => (true, None),
            _ => (false, None),
        }
    };
    let mut source = NarrationReader {
        app, narration, speech_queue: VecDeque::new(), chat_finished: false, current_voice: None,
    };
    let mut player = NativeConsumer { app, narration, started: false, first_audio: true };
    if let Some(audio) = sample_audio {
        audio_codec::decode_stream(audio.into(), &narration.cancel, &mut player).await?;
        return player.finish().await;
    }
    let mut synthesis_announced = false;
    if preview {
        // Preview is literal input and deliberately never opens the cache or
        // enters the prefetch scheduler.
        while let Some(piece) = source.next(&narration.cancel).await? {
            if !synthesis_announced {
                synthesis_announced = true;
                emit(app, narration, "status", "synthesizing", None);
            }
            // The output closure cannot lend its mutable consumer across calls;
            // the mutex only serializes this one direct synthesize callback.
            let player = tokio::sync::Mutex::new(&mut player);
            network::synthesize(
                &narration.config, &piece.text, None, narration.sample.as_ref(),
                &narration.cancel, |pcm| {
                    let player = &player;
                    async move { player.lock().await.append(pcm, &narration.cancel).await }
                },
            ).await?;
        }
    } else {
        let cache = match crate::vcp_modules::file_manager::get_tts_cache_root_dir(app) {
            Ok(root) => match TtsCache::open(root).await {
                Ok(cache) => Some(cache),
                Err(error) => { log::warn!("[VoiceTts] cache unavailable: {error}"); None }
            },
            Err(error) => { log::warn!("[VoiceTts] cache path unavailable: {error}"); None }
        };
        pipeline::run(&narration.cancel, source, |piece, segment, cancel| {
            // Waiting for stable text is not synthesis. Announce only the
            // first actual piece; later prefetch must not overwrite speaking.
            if !synthesis_announced {
                synthesis_announced = true;
                emit(app, narration, "status", "synthesizing", None);
            }
            prepare_segment(narration, cache.as_ref(), piece, segment, cancel)
        }, &mut player).await?;
    }
    player.finish().await
}

struct NarrationPiece {
    text: String,
    guidance: Option<Arc<str>>,
    voice: Option<Arc<PreparedVoice>>,
}

struct NarrationReader<'a, R: Runtime> {
    app: &'a AppHandle<R>,
    narration: &'a Arc<TtsJob>,
    speech_queue: VecDeque<SpeechPiece>,
    chat_finished: bool,
    // Only the current message is retained. Prefetched pieces own their own
    // Arc lease, so a group turn never accumulates every imported audio file.
    current_voice: Option<(String, Arc<PreparedVoice>)>,
}

impl<R: Runtime> SegmentSource for NarrationReader<'_, R> {
    type Piece = NarrationPiece;

    async fn next(&mut self, cancel: &CancellationToken) -> Result<Option<NarrationPiece>, String> {
        loop {
            if cancel.is_cancelled() { return Err(network::CANCELLED.into()); }
            let notified = self.narration.ready.notified();
            let mut speaker = None;
            let (piece, guidance) = {
                let mut input = self.narration.input.lock().map_err(|error| error.to_string())?;
                match &mut *input {
                    NarrationInput::Chat(feed) => {
                        if let Some(update) = feed.take_update()? {
                            self.speech_queue.extend(update.pieces);
                            self.chat_finished = update.finished;
                        }
                        if let Some(piece) = self.speech_queue.pop_front() {
                            log::debug!("[VoiceTts] synthesizing message={}", piece.message_id);
                            speaker = Some((piece.message_id, piece.agent_id, feed.is_group()));
                            (ReplyPiece::Text(piece.text), piece.instruction)
                        } else if self.chat_finished {
                            (ReplyPiece::Finished, None)
                        } else { (ReplyPiece::Wait, None) }
                    }
                    NarrationInput::Bubble(message) => message.next(),
                    NarrationInput::Preview(reply) => (reply.next(), None),
                    NarrationInput::Sample(_) => return Err("原声试听不接受合成文本。".into()),
                }
            };
            match piece {
                ReplyPiece::Text(text) => {
                    let voice = if let Some((message_id, agent_id, group)) = speaker {
                        if self.current_voice.as_ref().is_none_or(|(id, _)| id != &message_id) {
                            let agent_id = agent_id.as_deref().ok_or("无法确定回复的发言者，已停止朗读。")?;
                            let voice = prepare_speaker(self.app, &self.narration.config, Some(agent_id), cancel).await?;
                            self.current_voice = Some((message_id, Arc::new(voice)));
                        }
                        if self.current_voice.as_ref().is_some_and(|(_, voice)| voice.config.tts_mode == VoiceTtsMode::None) {
                            if group { continue; }
                            return Err("当前未选择朗读音色，请先选择音色。".into());
                        }
                        self.current_voice.as_ref().map(|(_, voice)| voice.clone())
                    } else {
                        None
                    };
                    return Ok(Some(NarrationPiece { text, guidance, voice }));
                }
                ReplyPiece::Finished => return Ok(None),
                ReplyPiece::Wait => tokio::select! {
                    biased;
                    _ = cancel.cancelled() => return Err(network::CANCELLED.into()),
                    _ = notified => {},
                },
            }
        }
    }
}

async fn prepare_segment(
    narration: &TtsJob,
    cache: Option<&TtsCache>,
    piece: NarrationPiece,
    segment: Arc<SegmentBuffer>,
    cancel: CancellationToken,
) -> Result<(), String> {
    let config = piece.voice.as_ref().map_or(&narration.config, |voice| &voice.config);
    let sample = piece.voice.as_ref().map_or(narration.sample.as_ref(), |voice| voice.sample.as_ref());
    let request = network::prepare_synthesis(config, &piece.text, piece.guidance.as_deref(), sample)?;
    let key = cache::segment_key(&request.identity);
    if let Some(cache) = cache {
        match cache.lookup(&key, &cancel).await {
            Ok(Some(pcm)) => {
                segment.push(pcm)?;
                return segment.finish();
            }
            Ok(None) => {},
            Err(_) if cancel.is_cancelled() => return Err(network::CANCELLED.into()),
            Err(error) => log::warn!("[VoiceTts] cache lookup failed: {error}"),
        }
    }
    let audio = network::synthesize_prepared(config, &request, &cancel, |pcm| {
        std::future::ready(if cancel.is_cancelled() {
            Err(network::CANCELLED.into())
        } else { segment.push(pcm) })
    }).await?;
    // The consumer can hand over the segment as soon as its last PCM packet is
    // accepted; cache publication is owned and awaited separately by producer.
    segment.finish()?;
    if let Some(cache) = cache {
        let encoded = match audio {
            network::SynthesizedAudio::Encoded { format, bytes } => Ok((format, bytes)),
            network::SynthesizedAudio::StreamedPcm => segment.wav(&cancel).await
                .map(|bytes| (super::audio_codec::ContainerFormat::Wav, bytes.into())),
        };
        let committed = match encoded {
            Ok((format, bytes)) => cache.commit(&key, format, &bytes, &cancel).await,
            Err(error) => Err(error),
        };
        if let Err(error) = committed {
            if cancel.is_cancelled() { return Err(network::CANCELLED.into()); }
            log::warn!("[VoiceTts] segment cache write failed; playback continues: {error}");
        }
    }
    Ok(())
}

struct NativeConsumer<'a, R: Runtime> {
    app: &'a AppHandle<R>,
    narration: &'a Arc<TtsJob>,
    started: bool,
    first_audio: bool,
}

impl<R: Runtime> PacketConsumer for NativeConsumer<'_, R> {
    async fn append(&mut self, pcm: Vec<u8>, cancel: &CancellationToken) -> Result<(), String> {
        if !self.started {
            // Mark before async admission so stop can install its native
            // tombstone even when native start has not been dispatched yet.
            {
                let state = self.app.state::<TtsServiceState>();
                let registry = state.0.lock().map_err(|error| error.to_string())?;
                registry.admit_player(self.narration)?;
            }
            playback_call(cancel, tauri_plugin_vcp_mobile::voice::start_playback(
                self.app.clone(), self.narration.id.clone())).await?;
            self.started = true;
        }
        // Preview may deliver a complete decoded response here. Bound each
        // native handoff without splitting and rebuilding the PCM upstream.
        for packet in pcm.chunks(PCM_PACKET_BYTES) {
            playback_call(cancel, tauri_plugin_vcp_mobile::voice::append_playback(
                self.app.clone(), self.narration.id.clone(), STANDARD.encode(packet))).await?;
            if self.first_audio {
                self.first_audio = false;
                emit(self.app, self.narration, "status", "speaking", None);
            }
        }
        Ok(())
    }
}

impl<R: Runtime> NativeConsumer<'_, R> {
    async fn finish(&mut self) -> Result<(), String> {
        if self.started {
            playback_call(&self.narration.cancel, tauri_plugin_vcp_mobile::voice::finish_playback(
                self.app.clone(), self.narration.id.clone())).await?;
            self.narration.player_started.store(false, Ordering::Release);
        }
        Ok(())
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
                NarrationKind::Chat => NarrationInput::Chat(ChatFeed::new()),
                NarrationKind::Bubble(_) => NarrationInput::Bubble(MessageSpeech::new()),
                NarrationKind::Preview => NarrationInput::Preview(ReplyBuffer::new(false)),
                NarrationKind::SamplePreview => NarrationInput::Sample(None),
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

    #[test]
    fn native_stop_can_retire_admission_without_releasing_a_running_workers_lease() {
        let app = tauri::test::mock_app();
        app.manage(TtsServiceState::default());
        let state = app.state::<TtsServiceState>();
        let job = install(&mut state.0.lock().unwrap(), "running");
        let lease = Arc::new(super::super::call::WorkLease::default());
        *job.work.lock().unwrap() = Some(lease.clone());
        job.worker_started.store(true, Ordering::Release);
        state.0.lock().unwrap().admit_player(&job).unwrap();
        state.0.lock().unwrap().stop("running");
        let worker = job.clone();
        drop(job);
        // A pending or failed native stop leaves the registry and lease alive.
        assert!(state.0.lock().unwrap().current.is_some());
        assert_eq!(Arc::strong_count(&lease), 2);
        // Once native stop acknowledges, outstanding worker cleanup still owns
        // the same job. Retiring admission must not take its lease.
        retire(app.handle(), &worker);
        assert!(state.0.lock().unwrap().current.is_none());
        assert_eq!(Arc::strong_count(&lease), 2);
        // Once its joined worker exits, the same job no longer holds the lease.
        drop(worker);
        assert_eq!(Arc::strong_count(&lease), 1);

        let pending = install(&mut state.0.lock().unwrap(), "pending");
        *pending.work.lock().unwrap() = Some(lease.clone());
        retire(app.handle(), &pending);
        assert_eq!(Arc::strong_count(&lease), 2);
        drop(pending);
        assert_eq!(Arc::strong_count(&lease), 1);
    }

    #[tokio::test]
    async fn worker_scheduled_after_stop_does_not_emit_synthesizing_or_open_resources() {
        use tauri::Listener;
        let app = tauri::test::mock_app();
        app.manage(TtsServiceState::default());
        let state = app.state::<TtsServiceState>();
        let job = install(&mut state.0.lock().unwrap(), "late-worker");
        let events = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = events.clone();
        app.listen(TTS_EVENT, move |_| { observed.fetch_add(1, Ordering::SeqCst); });
        job.cancel.cancel();
        assert_eq!(speak_inner(app.handle(), &job).await.unwrap_err(), network::CANCELLED);
        assert_eq!(events.load(Ordering::SeqCst), 0);
    }

    struct SegmentHttpFixture {
        base_url: String,
        requests: Arc<Mutex<Vec<serde_json::Value>>>,
        shutdown: CancellationToken,
        server: tokio::task::JoinHandle<()>,
    }

    impl SegmentHttpFixture {
        async fn start(pcm: &[u8], fail_first: bool) -> Self {
            use axum::{http::StatusCode, routing::post, Json, Router};
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base_url = format!("http://{}/v1/", listener.local_addr().unwrap());
            let requests = Arc::new(Mutex::new(Vec::new()));
            let captured = requests.clone();
            let audio = serde_json::json!({"choices": [{"delta": {"audio": {
                "data": STANDARD.encode(pcm)
            }}, "finish_reason": null}]});
            let body = format!("data: {audio}\n\ndata: [DONE]\n\n");
            let router = Router::new().route("/v1/chat/completions", post(
                move |Json(request): Json<serde_json::Value>| {
                    let captured = captured.clone();
                    let body = body.clone();
                    async move {
                        let attempt = {
                            let mut requests = captured.lock().unwrap();
                            requests.push(request);
                            requests.len()
                        };
                        if fail_first && attempt == 1 {
                            (StatusCode::SERVICE_UNAVAILABLE,
                                [("content-type", "application/json")], "{}".to_owned())
                        } else {
                            (StatusCode::OK, [("content-type", "text/event-stream")], body)
                        }
                    }
                },
            ));
            let shutdown = CancellationToken::new();
            let stopped = shutdown.clone();
            let server = tokio::spawn(async move {
                axum::serve(listener, router)
                    .with_graceful_shutdown(stopped.cancelled_owned())
                    .await.unwrap();
            });
            Self { base_url, requests, shutdown, server }
        }

        async fn stop(&mut self) {
            self.shutdown.cancel();
            tokio::time::timeout(std::time::Duration::from_secs(3), &mut self.server)
                .await.expect("fixture server stops").unwrap();
        }
    }

    impl Drop for SegmentHttpFixture {
        fn drop(&mut self) { self.shutdown.cancel(); }
    }

    fn segment_fixture_job(base_url: &str) -> Arc<TtsJob> {
        let mut registry = Registry::default();
        let mut job = install(&mut registry, "segment-fixture");
        registry.complete("segment-fixture");
        let job_mut = Arc::get_mut(&mut job).unwrap();
        job_mut.config.base_url = base_url.into();
        job_mut.config.voice = "白桦".into();
        job
    }

    fn fixture_piece() -> NarrationPiece {
        NarrationPiece { text: "这是同一句待合成的文本。".into(), guidance: Some(Arc::from("轻声")), voice: None }
    }

    async fn buffered_pcm(segment: &Arc<SegmentBuffer>, cancel: &CancellationToken) -> Vec<u8> {
        super::super::wav::pcm16_mono(&segment.wav(cancel).await.unwrap(), 24000).unwrap()
    }

    async fn clone_library_fixture() -> (tauri::App<tauri::test::MockRuntime>, tempfile::TempDir, String) {
        use sha2::Digest;
        let directory = tempfile::tempdir_in(".").unwrap();
        let mut context = tauri::test::mock_context(tauri::test::noop_assets());
        context.config_mut().app.app_directories_override = Some(
            tauri::utils::config::AppDirectoriesOverride::Root(directory.path().canonicalize().unwrap()),
        );
        let app = tauri::test::mock_builder().build(context).unwrap();
        let root = app.path().app_data_dir().unwrap().join("voice_library");
        tokio::fs::create_dir_all(root.join("samples")).await.unwrap();
        let id = "a".repeat(32);
        let audio = audio_codec::pcm_wav(&vec![0; 4800]).unwrap();
        tokio::fs::write(root.join("samples").join(format!("{id}.wav")), &audio).await.unwrap();
        let sample = samples::VoiceCloneModelDto {
            id: id.clone(), name: "Reference".into(), file_name: "reference.wav".into(),
            format: samples::CloneSampleFormat::Wav, file_bytes: audio.len() as u64, duration_ms: 100,
            file_hash: hex::encode(sha2::Sha256::digest(&audio)), created_at: 1, updated_at: 1,
        };
        tokio::fs::write(root.join("registry.json"), serde_json::to_vec(
            &serde_json::json!({"version":1,"samples":[sample]})).unwrap()).await.unwrap();
        (app, directory, id)
    }

    fn group_narration_fixture(
        app: &tauri::App<tauri::test::MockRuntime>,
        base_url: &str,
        voice: crate::vcp_modules::agent_types::AgentVoiceConfig,
    ) -> Arc<TtsJob> {
        use crate::vcp_modules::agent_service::{create_default_config, AgentConfigState};
        let agents = AgentConfigState::new();
        let mut first = create_default_config("a");
        first.voice = voice;
        agents.caches.insert("a".into(), first);
        let mut second = create_default_config("b");
        second.voice.enabled = true;
        second.voice.mode = VoiceTtsMode::Builtin;
        second.voice.voice = "苏打".into();
        agents.caches.insert("b".into(), second);
        app.manage(agents);
        let mut narration = segment_fixture_job(base_url);
        Arc::get_mut(&mut narration).unwrap().config.tts_mode = VoiceTtsMode::None;
        narration.worker_started.store(true, Ordering::Release);
        app.manage(TtsServiceState(Mutex::new(Registry {
            current: Some(Slot { id: narration.id.clone(), cancel: narration.cancel.clone(),
                job: Some(narration.clone()), kind: NarrationKind::Chat }),
            ..Default::default()
        })));
        narration
    }

    fn feed_member(app: &tauri::App<tauri::test::MockRuntime>, id: &str, agent: &str, text: &str) {
        feed_chat_event(app.handle(), &StreamEvent::end(
            id.into(), Some(serde_json::json!({"isGroupMessage":true,"agentId":agent,"originUserMessageId":"origin"})),
            Some("completed".into()), Some(text.into()), None, None, None,
        ));
    }

    #[tokio::test]
    async fn group_none_or_deleted_clone_skips_only_that_member_before_valid_speaker_http() {
        use crate::vcp_modules::agent_types::AgentVoiceConfig;
        let mut server = SegmentHttpFixture::start(&[1, 0, 2, 0], false).await;
        for deleted_clone in [false, true] {
            let (app, _directory, id) = clone_library_fixture().await;
            let voice = if deleted_clone {
                assert!(samples::delete_voice_clone_sample(app.handle().clone(), id.clone()).await.unwrap());
                AgentVoiceConfig { enabled: true, mode: VoiceTtsMode::Clone, clone_sample_id: id, ..Default::default() }
            } else { AgentVoiceConfig::default() };
            let narration = group_narration_fixture(&app, &server.base_url, voice);
            feed_member(&app, "first", "a", "第一位没有声音但仍有文字。");
            feed_member(&app, "second", "b", "第二位继续朗读。");
            finish_tts_chat(app.handle().clone(), narration.id.clone()).await.unwrap();
            let mut reader = NarrationReader { app: app.handle(), narration: &narration,
                speech_queue: VecDeque::new(), chat_finished: false, current_voice: None };
            let piece = reader.next(&narration.cancel).await.unwrap().unwrap();
            assert_eq!(piece.text, "第二位继续朗读。");
            assert_eq!(piece.voice.as_ref().unwrap().config.voice, "苏打");
            prepare_segment(&narration, None, piece, Arc::new(SegmentBuffer::default()), narration.cancel.clone()).await.unwrap();
            assert!(reader.next(&narration.cancel).await.unwrap().is_none());
            assert!(!narration.cancel.is_cancelled());
            assert!(!narration.terminal.load(Ordering::Acquire));
        }
        assert_eq!(server.requests.lock().unwrap().len(), 2);
        assert!(server.requests.lock().unwrap().iter().all(|request| request["audio"]["voice"] == "苏打"));
        server.stop().await;
    }

    #[tokio::test]
    async fn deleting_a_prepared_group_voice_keeps_its_message_but_skips_its_next_message() {
        use crate::vcp_modules::agent_types::AgentVoiceConfig;
        let (app, _directory, id) = clone_library_fixture().await;
        let narration = group_narration_fixture(&app, "https://example.invalid/v1", AgentVoiceConfig {
            enabled: true, mode: VoiceTtsMode::Clone, clone_sample_id: id.clone(), ..Default::default()
        });
        feed_member(&app, "first", "a", &"第一位的长回复继续使用同一份声音。".repeat(40));
        let mut reader = NarrationReader { app: app.handle(), narration: &narration,
            speech_queue: VecDeque::new(), chat_finished: false, current_voice: None };
        let first = reader.next(&narration.cancel).await.unwrap().unwrap().voice.unwrap();
        assert!(first.sample.is_some());
        assert!(samples::delete_voice_clone_sample(app.handle().clone(), id).await.unwrap());
        let second = reader.next(&narration.cancel).await.unwrap().unwrap().voice.unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert!(second.sample.as_ref().unwrap().data_url.starts_with("data:audio/wav;base64,"));
        feed_member(&app, "later", "a", "下一条已经没有音色。");
        feed_member(&app, "other", "b", "另一位照常朗读。");
        finish_tts_chat(app.handle().clone(), narration.id.clone()).await.unwrap();
        let mut saw_other = false;
        while let Some(piece) = reader.next(&narration.cancel).await.unwrap() {
            assert_ne!(piece.text, "下一条已经没有音色。");
            if piece.text == "另一位照常朗读。" { saw_other = true; }
        }
        assert!(saw_other);
    }

    #[tokio::test]
    async fn durable_nonstream_end_delivers_the_actual_author_to_the_narration_feed() {
        use crate::vcp_modules::{db_manager::DbState, message_service,
            vcp_client::{ActiveRequestLease, ActiveRequests}};
        for (owner_type, owner_id, author) in [("agent", "a", "a"), ("group", "g", "b")] {
            let pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1)
                .connect("sqlite::memory:").await.unwrap();
            sqlx::raw_sql(include_str!("../../../../migrations/0100_baseline_v2.sql"))
                .execute(&pool).await.unwrap();
            sqlx::raw_sql(include_str!("../../../../migrations/0102_stream_stop_intent.sql"))
                .execute(&pool).await.unwrap();
            sqlx::raw_sql("INSERT INTO agents (owner_type,agent_id,name,model,updated_at) VALUES ('agent','a','A','test',1),('agent','b','B','test',1);
                INSERT INTO groups (owner_type,group_id,name,updated_at) VALUES ('group','g','Group',1);
                INSERT INTO topics (owner_type,owner_id,topic_id,title,created_at,updated_at) VALUES ('agent','a','t','Topic',1,1),('group','g','t','Topic',1,1);")
                .execute(&pool).await.unwrap();
            let app = tauri::test::mock_app();
            app.manage(DbState::new(pool.clone(), Default::default()));
            app.manage(ActiveRequests::default());
            app.manage(TtsServiceState::default());
            let job = install(&mut app.state::<TtsServiceState>().0.lock().unwrap(), "durable-end");
            job.worker_started.store(true, Ordering::Release);
            let key = MessageKey::new(TopicKey::new(owner_type, owner_id, "t"), "reply");
            let (lease, _) = ActiveRequestLease::try_acquire(app.state::<ActiveRequests>().0.clone(), key.clone()).unwrap();
            lease.set_origin_user_message("origin").unwrap();
            message_service::begin_stream_message(&app.state::<DbState>(), &key, Some(author), None).await.unwrap();
            // No Aurora/thinking event precedes this durable final result.
            message_service::finalize_stream_message(app.handle().clone(), &pool, &key,
                "完整回复。".into(), false, Some("completed".into()), None, Some(author.into())).await.unwrap();
            let mut input = job.input.lock().unwrap();
            let NarrationInput::Chat(feed) = &mut *input else { unreachable!() };
            let update = feed.take_update().unwrap().unwrap();
            assert_eq!(update.pieces.len(), 1);
            assert_eq!(update.pieces[0].agent_id.as_deref(), Some(author));
            assert_eq!(update.pieces[0].text, "完整回复。");
            drop(input);
            drop(lease);
        }
    }

    #[tokio::test]
    async fn group_message_speakers_select_distinct_voices_before_http_and_cache() {
        use crate::vcp_modules::agent_service::{create_default_config, AgentConfigState};
        use serde_json::json;
        let pcm = vec![1, 0, 2, 0];
        let mut server = SegmentHttpFixture::start(&pcm, false).await;
        let app = tauri::test::mock_app();
        let agents = AgentConfigState::new();
        for (id, voice) in [("a", "冰糖"), ("b", "苏打")] {
            let mut agent = create_default_config(id);
            agent.voice.enabled = true;
            agent.voice.mode = VoiceTtsMode::Builtin;
            agent.voice.voice = voice.into();
            agents.caches.insert(id.into(), agent);
        }
        app.manage(agents);
        let mut job = segment_fixture_job(&server.base_url);
        // An unusable inherited clone must not block independent built-in voices.
        Arc::get_mut(&mut job).unwrap().config.tts_mode = VoiceTtsMode::Clone;
        Arc::get_mut(&mut job).unwrap().config.clone_sample_id.clear();
        {
            let mut input = job.input.lock().unwrap();
            let NarrationInput::Chat(feed) = &mut *input else { unreachable!() };
            for id in ["a", "b"] {
                feed.accept(&StreamEvent::end(
                    format!("message-{id}"), Some(json!({"isGroupMessage":true,"agentId":id})),
                    Some("completed".into()), Some("相同的正文。".into()), None, None, None,
                )).unwrap();
            }
            feed.finish_submission();
        }
        let temporary = tempfile::tempdir().unwrap();
        let cache = TtsCache::open(temporary.path().into()).await.unwrap();
        let mut reader = NarrationReader {
            app: app.handle(), narration: &job, speech_queue: VecDeque::new(),
            chat_finished: false, current_voice: None,
        };
        while let Some(piece) = reader.next(&job.cancel).await.unwrap() {
            let segment = Arc::new(SegmentBuffer::default());
            prepare_segment(&job, Some(&cache), piece, segment.clone(), job.cancel.clone()).await.unwrap();
            assert_eq!(buffered_pcm(&segment, &job.cancel).await, pcm);
        }
        let voices: Vec<_> = server.requests.lock().unwrap().iter()
            .map(|request| request["audio"]["voice"].as_str().unwrap().to_owned()).collect();
        assert_eq!(voices, ["冰糖", "苏打"], "identical text must not reuse another speaker's cache");
        server.stop().await;
    }

    #[tokio::test]
    async fn later_segments_keep_the_message_voice_when_agent_settings_change() {
        use crate::vcp_modules::agent_service::{create_default_config, AgentConfigState};
        let app = tauri::test::mock_app();
        let agents = AgentConfigState::new();
        let mut agent = create_default_config("a");
        agent.voice.enabled = true;
        agent.voice.mode = VoiceTtsMode::Builtin;
        agent.voice.voice = "冰糖".into();
        agents.caches.insert("a".into(), agent);
        app.manage(agents);
        let job = segment_fixture_job("https://example.invalid/v1");
        {
            let mut input = job.input.lock().unwrap();
            let NarrationInput::Chat(feed) = &mut *input else { unreachable!() };
            feed.accept(&StreamEvent::end(
                "message".into(), Some(serde_json::json!({"agentId":"a"})),
                Some("completed".into()), Some("这是一段足够长的语音正文。".repeat(50)), None, None, None,
            )).unwrap();
        }
        let mut reader = NarrationReader {
            app: app.handle(), narration: &job, speech_queue: VecDeque::new(),
            chat_finished: false, current_voice: None,
        };
        let first = reader.next(&job.cancel).await.unwrap().unwrap();
        app.state::<AgentConfigState>().caches.get_mut("a").unwrap().voice.voice = "苏打".into();
        let second = reader.next(&job.cancel).await.unwrap().unwrap();
        let first_voice = first.voice.unwrap();
        let second_voice = second.voice.unwrap();
        assert!(Arc::ptr_eq(&first_voice, &second_voice));
        assert_eq!(second_voice.config.voice, "冰糖");
    }

    #[tokio::test]
    async fn real_segment_http_error_is_requested_once_and_does_not_publish_cache() {
        let pcm = vec![1, 0, 2, 0, 3, 0, 4, 0];
        let mut server = SegmentHttpFixture::start(&pcm, true).await;
        let temporary = tempfile::tempdir().unwrap();
        let cache = TtsCache::open(temporary.path().to_path_buf()).await.unwrap();
        let job = segment_fixture_job(&server.base_url);
        let segment = Arc::new(SegmentBuffer::default());
        let error = prepare_segment(&job, Some(&cache), fixture_piece(), segment.clone(), job.cancel.clone())
            .await.unwrap_err();
        assert!(error.contains("503"));
        assert_eq!(server.requests.lock().unwrap().len(), 1, "a network error ends this segment immediately");
        assert!(segment.wav(&job.cancel).await.is_err(), "failed preparation must not mark audio complete");
        assert!(tokio::fs::read_dir(temporary.path()).await.unwrap().next_entry().await.unwrap().is_none());
        server.stop().await;
    }

    #[tokio::test]
    async fn real_segment_reuses_stream_cache_offline_in_nonstream_mode() {
        let pcm = vec![1, 0, 2, 0, 3, 0, 4, 0];
        let mut server = SegmentHttpFixture::start(&pcm, false).await;
        let temporary = tempfile::tempdir().unwrap();
        let cache = TtsCache::open(temporary.path().to_path_buf()).await.unwrap();
        let mut job = segment_fixture_job(&server.base_url);
        let first = Arc::new(SegmentBuffer::default());
        prepare_segment(&job, Some(&cache), fixture_piece(), first.clone(), job.cancel.clone())
            .await.unwrap();
        assert_eq!(buffered_pcm(&first, &job.cancel).await, pcm);
        let request = network::prepare_synthesis(&job.config, &fixture_piece().text,
            fixture_piece().guidance.as_deref(), None).unwrap();
        let key = cache::segment_key(&request.identity);
        let file = temporary.path().join(format!("{key}.wav"));
        assert_eq!(super::super::wav::pcm16_mono(&tokio::fs::read(file).await.unwrap(), 24000).unwrap(), pcm);
        {
            let requests = server.requests.lock().unwrap();
            assert_eq!(requests.len(), 1);
            assert!(requests.iter().all(|request| request["stream"] == true));
            assert!(requests.iter().all(|request| request["audio"]["format"] == "pcm16"));
        }
        server.stop().await;
        Arc::get_mut(&mut job).unwrap().config.tts_stream = false;
        let second = Arc::new(SegmentBuffer::default());
        tokio::time::timeout(std::time::Duration::from_secs(1), prepare_segment(
            &job, Some(&cache), fixture_piece(), second.clone(), job.cancel.clone(),
        )).await.expect("cache hit must not attempt offline HTTP").unwrap();
        assert_eq!(buffered_pcm(&second, &job.cancel).await, pcm);
        assert_eq!(server.requests.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn real_segment_keeps_audio_when_cache_lookup_and_atomic_publish_fail() {
        let pcm = vec![10, 0, 20, 0, 30, 0, 40, 0];
        let mut server = SegmentHttpFixture::start(&pcm, false).await;
        let temporary = tempfile::tempdir().unwrap();
        let cache = TtsCache::open(temporary.path().to_path_buf()).await.unwrap();
        let job = segment_fixture_job(&server.base_url);
        let request = network::prepare_synthesis(&job.config, &fixture_piece().text,
            fixture_piece().guidance.as_deref(), None).unwrap();
        let key = cache::segment_key(&request.identity);
        // A directory at the final filename causes both real lookup and real
        // rename to fail, regardless of platform file permission conventions.
        let blocked = temporary.path().join(format!("{key}.wav"));
        tokio::fs::create_dir(&blocked).await.unwrap();
        let segment = Arc::new(SegmentBuffer::default());
        prepare_segment(&job, Some(&cache), fixture_piece(), segment.clone(), job.cancel.clone())
            .await.expect("cache failures must not interrupt valid synthesis");
        assert_eq!(buffered_pcm(&segment, &job.cancel).await, pcm);
        assert!(tokio::fs::metadata(&blocked).await.unwrap().is_dir());
        assert_eq!(server.requests.lock().unwrap().len(), 1);
        let mut entries = tokio::fs::read_dir(temporary.path()).await.unwrap();
        let mut names = Vec::new();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
        assert_eq!(names, [format!("{key}.wav")], "failed commit removes its pending file");
        server.stop().await;
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
        let mut input = NarrationInput::Bubble(MessageSpeech::new());
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
        let mut input = NarrationInput::Preview(ReplyBuffer::new(false));
        let raw = "<voice_guidance>轻声</voice_guidance>照读";
        input.submit_preview(raw.into()).unwrap();
        assert!(input.submit_bubble(raw, true).is_err());
        let NarrationInput::Preview(reply) = &mut input else { unreachable!() };
        assert!(matches!(reply.next(), ReplyPiece::Text(text) if text == raw));
    }

    #[test]
    fn completed_empty_bubble_is_rejected_but_unclosed_content_is_read() {
        let mut input = NarrationInput::Bubble(MessageSpeech::new());
        assert!(input.submit_bubble("<think>不读</think>", true).is_err());
        let mut input = NarrationInput::Bubble(MessageSpeech::new());
        input.submit_bubble("<think>模型忘记闭合，这段内容仍然需要朗读。", true).unwrap();
        let NarrationInput::Bubble(message) = &mut input else { unreachable!() };
        assert!(matches!(message.next(), (ReplyPiece::Text(text), None) if text.contains("模型忘记闭合")));
    }

    #[test]
    fn active_call_rejects_bubble_admission_but_accepts_its_own_reply() {
        assert!(NarrationKind::Bubble(bubble_source()).validate_admission(true).is_err());
        assert!(NarrationKind::Bubble(bubble_source()).validate_admission(false).is_ok());
        assert!(NarrationKind::Chat.validate_admission(true).is_ok());
        assert!(NarrationKind::SamplePreview.validate_admission(true).is_err());
        assert!(NarrationKind::SamplePreview.validate_admission(false).is_ok());
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

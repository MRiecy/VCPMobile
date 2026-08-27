use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::db_write_queue::DbWriteQueue;
use crate::vcp_modules::settings_manager::{read_settings, Settings, SettingsState};
use crate::vcp_modules::sync_error::{
    build_local_error_payload, build_wire_error_payload, decode_wire_sync_error,
    encode_wire_sync_error, SyncErrorPayload, WireSyncError,
};
use crate::vcp_modules::sync_logger::{redact_sync_diagnostic, LogLevel, SyncLogger};
use crate::vcp_modules::sync_pipeline::{Phase1Metadata, Phase3Message, SyncPipeline};
use crate::vcp_modules::sync_types::{
    DeleteNotificationFrame, DeleteTarget, ManifestRequestFrame, ManifestResultFrame, ManifestType,
    MessageDiffRequestFrame, MessageDiffTopicState, OwnerType, SyncPhase, TopicDiffRequestFrame,
    TopicDiffResultFrame, TopicDiffState,
};
use crate::vcp_modules::topic_types::{OwnerKey, TopicKey};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashSet;
use std::future::Future;
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, Runtime, State};
use tokio::sync::{mpsc, Mutex as AsyncMutex, RwLock};
use tokio::task::{JoinHandle, JoinSet};
use tokio_tungstenite::{
    connect_async, tungstenite::protocol::Message, MaybeTlsStream, WebSocketStream,
};
use tokio_util::sync::CancellationToken;

const WIRE_PROTOCOL_VERSION: &str = "1.4";
const VERSION_CHECK_TIMEOUT: Duration = Duration::from_secs(5);
const WS_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const WS_OPERATION_TIMEOUT: Duration = Duration::from_secs(5);
const SYNC_HTTP_REQUEST_TIMEOUT: Duration = Duration::from_secs(270);
const PHASE_RESPONSE_TIMEOUT: Duration = Duration::from_secs(60);
const FINAL_ACK_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_SYNC_RETRIES: u32 = 3;
const MAX_SYNC_TOPICS: usize = 10_000;
#[cfg(target_os = "android")]
const SYNC_GUARDIAN_LABEL: &str = "[数据同步] VCP Mobile";
type RoutedSyncCommand = (u64, mpsc::UnboundedSender<SyncCommand>);
type SyncWebSocket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct VersionCheckFrame {
    #[serde(rename = "type")]
    frame_type: &'static str,
    mobile_version: &'static str,
    protocol_version: &'static str,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct VersionAck {
    #[serde(rename = "type")]
    frame_type: String,
    plugin_version: String,
    protocol_version: String,
}

#[derive(Debug, Deserialize)]
struct FrameHeader {
    #[serde(rename = "type")]
    frame_type: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RemoteSyncErrorFrame {
    #[serde(rename = "type")]
    _frame_type: String,
    error: WireSyncError,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PhaseFrame {
    #[serde(rename = "type")]
    frame_type: &'static str,
    phase: SyncPhase,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PhaseAckFrame {
    #[serde(rename = "type")]
    frame_type: String,
    phase: SyncPhase,
    #[serde(default)]
    session_id: Option<u64>,
    #[serde(default)]
    attempt_id: Option<u64>,
    #[serde(default)]
    nonce: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SyncLogEventFrame {
    #[serde(rename = "type")]
    _frame_type: String,
    level: String,
    #[serde(rename = "phase")]
    _phase: String,
    message: String,
    #[serde(rename = "ts")]
    _ts: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DesktopPhaseEventFrame {
    #[serde(rename = "type")]
    frame_type: String,
    phase: String,
    #[serde(rename = "ts")]
    _ts: i64,
}

#[derive(Debug, PartialEq, Eq)]
enum VersionHandshakeError {
    Protocol(String),
    Remote(String),
    Closed { code: Option<u16>, reason: String },
    Transport(String),
}

fn parse_version_ack(text: &str) -> Result<VersionAck, String> {
    let ack = serde_json::from_str::<VersionAck>(text)
        .map_err(|error| format!("Invalid VERSION_ACK: {error}"))?;
    if ack.frame_type != "VERSION_ACK" {
        return Err("expected VERSION_ACK".to_string());
    }
    if ack.plugin_version.is_empty() {
        return Err("VERSION_ACK.pluginVersion must be a non-empty string".to_string());
    }
    if ack.protocol_version.is_empty() {
        return Err("VERSION_ACK.protocolVersion must be a non-empty string".to_string());
    }
    Ok(ack)
}

fn parse_version_handshake_text(text: &str) -> Result<Option<VersionAck>, VersionHandshakeError> {
    let header = serde_json::from_str::<FrameHeader>(text).map_err(|error| {
        VersionHandshakeError::Protocol(format!("Malformed handshake frame: {error}"))
    })?;
    match header.frame_type.as_str() {
        "SYNC_ERROR" => {
            let frame = serde_json::from_str::<RemoteSyncErrorFrame>(text).map_err(|error| {
                VersionHandshakeError::Protocol(format!("Invalid SYNC_ERROR: {error}"))
            })?;
            let encoded =
                encode_wire_sync_error(&frame.error).map_err(VersionHandshakeError::Protocol)?;
            Err(VersionHandshakeError::Remote(encoded))
        }
        "SYNC_LOG_EVENT" => {
            serde_json::from_str::<SyncLogEventFrame>(text).map_err(|error| {
                VersionHandshakeError::Protocol(format!("Invalid SYNC_LOG_EVENT: {error}"))
            })?;
            Ok(None)
        }
        _ => parse_version_ack(text)
            .map(Some)
            .map_err(VersionHandshakeError::Protocol),
    }
}

fn is_wire_compatible(version_ack: &VersionAck) -> bool {
    version_ack.protocol_version == WIRE_PROTOCOL_VERSION
}

async fn send_ws_with_deadline(
    ws_stream: &mut SyncWebSocket,
    message: Message,
) -> Result<(), String> {
    tokio::time::timeout(WS_OPERATION_TIMEOUT, ws_stream.send(message))
        .await
        .map_err(|_| "WebSocket send timed out".to_string())?
        .map_err(|error| error.to_string())
}

async fn send_ws_frame<T: Serialize>(
    ws_stream: &mut SyncWebSocket,
    frame: &T,
) -> Result<(), String> {
    let text = serde_json::to_string(frame)
        .map_err(|error| format!("Failed to serialize sync protocol frame: {error}"))?;
    send_ws_with_deadline(ws_stream, Message::Text(text.into())).await
}

async fn close_ws_with_deadline(ws_stream: &mut SyncWebSocket) -> Result<(), String> {
    tokio::time::timeout(WS_OPERATION_TIMEOUT, ws_stream.close(None))
        .await
        .map_err(|_| "WebSocket close timed out".to_string())?
        .map_err(|error| error.to_string())
}

fn protocol_send_failure_message(context: &str, error: &str) -> String {
    format!("Failed to send {context}: {error}")
}

async fn terminate_after_protocol_send_failure<R: Runtime>(
    app_handle: &AppHandle<R>,
    ws_stream: &mut SyncWebSocket,
    context: &str,
    error: &str,
) {
    let message = protocol_send_failure_message(context, error);
    emit_sync_log(app_handle, "warning", &message);
    let _ = close_ws_with_deadline(ws_stream).await;
}

fn take_retry_slot(retry_count: &mut u32, retry_delay: &mut Duration) -> Option<Duration> {
    if *retry_count >= MAX_SYNC_RETRIES {
        return None;
    }
    *retry_count += 1;
    let backoff = *retry_delay;
    *retry_delay = (*retry_delay * 2).min(Duration::from_secs(5));
    Some(backoff)
}

#[allow(clippy::too_many_arguments)]
async fn schedule_sync_retry<R: Runtime>(
    app_handle: &AppHandle<R>,
    session_id: u64,
    status: &Arc<RwLock<String>>,
    cancel_token: &CancellationToken,
    retry_count: &mut u32,
    retry_delay: &mut Duration,
    error_code: &str,
    message: &str,
) -> bool {
    if cancel_token.is_cancelled() {
        return false;
    }
    let Some(backoff) = take_retry_slot(retry_count, retry_delay) else {
        let final_message =
            format!("{message}; retry budget exhausted after {MAX_SYNC_RETRIES} automatic retries");
        emit_sync_log(app_handle, "error", &final_message);
        publish_sync_error(
            app_handle,
            session_id,
            status,
            error_code,
            &final_message,
            Vec::new(),
        )
        .await;
        return false;
    };
    emit_sync_log(
        app_handle,
        "warning",
        &format!(
            "{message}; reconnecting after {backoff:?} ({}/{MAX_SYNC_RETRIES})",
            *retry_count
        ),
    );
    emit_operator_sync_log(
        app_handle,
        session_id,
        "warning",
        &format!(
            "连接中断，正在进行第 {}/{} 次自动重试",
            *retry_count, MAX_SYNC_RETRIES
        ),
    );
    !cancelled_during(cancel_token, backoff).await
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FinalAckKey {
    session_id: u64,
    attempt_id: u64,
    phase: String,
    nonce: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FinalPhaseCompletedFrame<'a> {
    #[serde(rename = "type")]
    frame_type: &'static str,
    phase: SyncPhase,
    session_id: u64,
    attempt_id: u64,
    nonce: &'a str,
}

impl FinalAckKey {
    fn new(session_id: u64, attempt_id: u64) -> Self {
        Self {
            session_id,
            attempt_id,
            phase: SyncPhase::Messages.to_string(),
            nonce: uuid::Uuid::new_v4().to_string(),
        }
    }

    fn message(&self) -> FinalPhaseCompletedFrame<'_> {
        FinalPhaseCompletedFrame {
            frame_type: "PHASE_COMPLETED",
            phase: SyncPhase::Messages,
            session_id: self.session_id,
            attempt_id: self.attempt_id,
            nonce: &self.nonce,
        }
    }

    fn matches_payload(&self, payload: &PhaseAckFrame) -> bool {
        payload.frame_type == "PHASE_ACK"
            && payload.phase == SyncPhase::Messages
            && payload.session_id == Some(self.session_id)
            && payload.attempt_id == Some(self.attempt_id)
            && payload.nonce.as_deref() == Some(self.nonce.as_str())
    }
}

type PendingFinalAck = Arc<Mutex<Option<FinalAckKey>>>;

fn consume_final_ack(pending: &PendingFinalAck, payload: &PhaseAckFrame) -> bool {
    let Ok(mut guard) = pending.lock() else {
        return false;
    };
    if guard
        .as_ref()
        .is_some_and(|expected| expected.matches_payload(payload))
    {
        guard.take();
        true
    } else {
        false
    }
}

async fn enforce_final_ack_deadline(
    pending: PendingFinalAck,
    expected: FinalAckKey,
    tx: mpsc::UnboundedSender<SyncCommand>,
    deadline: Duration,
) {
    tokio::time::sleep(deadline).await;
    let expired = pending
        .lock()
        .map(|mut guard| {
            if guard.as_ref() == Some(&expected) {
                guard.take();
                true
            } else {
                false
            }
        })
        .unwrap_or(false);
    if expired {
        let _ = tx.send(SyncCommand::FailAttempt {
            attempt_id: expected.attempt_id,
            code: "FINAL_ACK_TIMEOUT",
            message: format!(
                "Desktop final sync acknowledgement timed out after {} seconds",
                deadline.as_secs()
            ),
        });
    }
}

async fn enforce_manifest_response_deadline(
    expected_types: Arc<Mutex<HashSet<ManifestType>>>,
    manifest_phase: Arc<AtomicU8>,
    expected_phase: u8,
    tx: mpsc::UnboundedSender<SyncCommand>,
    attempt_id: u64,
    deadline: Duration,
) {
    tokio::time::sleep(deadline).await;
    if manifest_phase.load(Ordering::SeqCst) != expected_phase {
        return;
    }
    let missing = match expected_types.lock() {
        Ok(expected) if expected.is_empty() => return,
        Ok(expected) => {
            let mut missing = expected.iter().map(ToString::to_string).collect::<Vec<_>>();
            missing.sort();
            missing
        }
        Err(_) => {
            let _ = tx.send(SyncCommand::FailAttemptDetailed {
                attempt_id,
                code: "SYNC_STATE_POISONED".to_string(),
                message: "Expected manifest type state is poisoned".to_string(),
                failed_topic_ids: Vec::new(),
            });
            return;
        }
    };
    let _ = tx.send(SyncCommand::FailAttemptDetailed {
        attempt_id,
        code: "MANIFEST_RESPONSE_TIMEOUT".to_string(),
        message: format!("Desktop manifest response timed out with missing data types {missing:?}"),
        failed_topic_ids: Vec::new(),
    });
}

async fn enforce_topic_hash_response_deadline(
    expected_results: Arc<AsyncMutex<Option<HashSet<TopicKey>>>>,
    manifest_phase: Arc<AtomicU8>,
    expected_phase: u8,
    tx: mpsc::UnboundedSender<SyncCommand>,
    attempt_id: u64,
    deadline: Duration,
) {
    tokio::time::sleep(deadline).await;
    if manifest_phase.load(Ordering::SeqCst) != expected_phase {
        return;
    }
    let pending_count = expected_results.lock().await.as_ref().map(HashSet::len);
    if let Some(pending_count) = pending_count {
        let _ = tx.send(SyncCommand::FailAttemptDetailed {
            attempt_id,
            code: "TOPIC_HASH_RESPONSE_TIMEOUT".to_string(),
            message: format!(
                "Desktop topic hash response timed out for {pending_count} expected topics"
            ),
            failed_topic_ids: Vec::new(),
        });
    }
}

#[derive(Clone, Default)]
pub struct SyncCommandRouter {
    current: Arc<std::sync::RwLock<Option<RoutedSyncCommand>>>,
}

impl SyncCommandRouter {
    pub fn send(&self, command: SyncCommand) -> Result<(), String> {
        let current = self
            .current
            .read()
            .map_err(|_| "同步命令路由锁已损坏".to_string())?;
        let Some((_, sender)) = current.as_ref() else {
            return Err("同步会话未运行".to_string());
        };
        sender.send(command).map_err(|e| e.to_string())
    }

    fn install(&self, session_id: u64, sender: mpsc::UnboundedSender<SyncCommand>) {
        if let Ok(mut current) = self.current.write() {
            *current = Some((session_id, sender));
        }
    }

    fn clear(&self) {
        if let Ok(mut current) = self.current.write() {
            *current = None;
        }
    }

    fn clear_if_owner(&self, session_id: u64) {
        if let Ok(mut current) = self.current.write() {
            if current.as_ref().map(|(id, _)| *id) == Some(session_id) {
                *current = None;
            }
        }
    }
}

pub struct SyncState {
    pub ws_sender: SyncCommandRouter,
    pub connection_status: Arc<RwLock<String>>,
    pub current_log_path: Arc<RwLock<Option<String>>>,
    pub current_logger: Arc<std::sync::RwLock<Option<Arc<std::sync::Mutex<SyncLogger>>>>>,
    lifecycle: AsyncMutex<()>,
    owner_commit: AsyncMutex<()>,
    session: AsyncMutex<Option<SyncSessionHandle>>,
    next_session_id: AtomicU64,
    current_session_id: AtomicU64,
}

struct SyncSessionHandle {
    session_id: u64,
    cancel_token: CancellationToken,
    command_tx: mpsc::UnboundedSender<SyncCommand>,
    join_handle: JoinHandle<Result<(), String>>,
}

struct SyncSessionConfig {
    ws_url: String,
    http_url: String,
    sync_token: String,
    sync_prerender_enabled: bool,
    sync_log_level: String,
}

#[derive(Debug, PartialEq, Eq)]
struct SyncConfigValidationError {
    code: &'static str,
    detail: String,
}

impl SyncConfigValidationError {
    fn new(code: &'static str, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }
}

pub(crate) struct SyncTaskTracker {
    cancel_token: CancellationToken,
    closed: AtomicBool,
    tasks: AsyncMutex<JoinSet<()>>,
}

impl SyncTaskTracker {
    fn new(cancel_token: CancellationToken) -> Self {
        Self {
            cancel_token,
            closed: AtomicBool::new(false),
            tasks: AsyncMutex::new(JoinSet::new()),
        }
    }

    pub(crate) async fn spawn<F>(&self, future: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        if self.closed.load(Ordering::SeqCst) {
            return;
        }

        let cancel_token = self.cancel_token.clone();
        let mut tasks = self.tasks.lock().await;
        if self.closed.load(Ordering::SeqCst) {
            return;
        }
        tasks.spawn(async move {
            tokio::select! {
                biased;
                _ = cancel_token.cancelled() => {}
                _ = future => {}
            }
        });
    }

    async fn close_and_wait(&self) {
        self.closed.store(true, Ordering::SeqCst);
        let mut tasks = self.tasks.lock().await;
        while let Some(result) = tasks.join_next().await {
            if let Err(error) = result {
                log::warn!("[SyncService] Session child task failed: {}", error);
            }
        }
    }
}

/// 追踪 Phase 3 中已处理完成的 topic，替代 AtomicU32 避免双重递减下溢
pub struct Phase3Tracker {
    pub session_id: u64,
    pub attempt_id: u64,
    pub completed: tokio::sync::Mutex<HashSet<TopicKey>>,
    pub modified: tokio::sync::Mutex<HashSet<TopicKey>>,
    pub failed: tokio::sync::Mutex<HashSet<TopicKey>>,
    pub legacy_attachment_warnings: std::sync::atomic::AtomicUsize,
    pub total: std::sync::atomic::AtomicUsize,
}

impl Phase3Tracker {
    /// 标记某个 topic 为数据已修改（实际发生了 pull/push）
    pub async fn mark_modified(&self, topic: &TopicKey) {
        let mut modified = self.modified.lock().await;
        modified.insert(topic.clone());
    }

    pub async fn mark_failed(&self, topic: &TopicKey) {
        self.failed.lock().await.insert(topic.clone());
    }

    pub fn add_legacy_attachment_warnings(&self, count: usize) {
        self.legacy_attachment_warnings
            .fetch_add(count, Ordering::SeqCst);
    }

    async fn completion_summary(&self) -> SyncCompletionSummary {
        let successful_topics = self.completed.lock().await.len();
        let failed = self.failed.lock().await;
        let mut failed_topic_ids = failed
            .iter()
            .map(|topic| topic.topic_id.clone())
            .collect::<Vec<_>>();
        failed_topic_ids.sort();
        failed_topic_ids.truncate(8);
        SyncCompletionSummary {
            successful_topics,
            total_topics: self.total.load(Ordering::SeqCst),
            failed_topics: failed.len(),
            legacy_attachment_warnings: self.legacy_attachment_warnings.load(Ordering::SeqCst),
            failed_topic_ids,
        }
    }

    /// 标记某个 topic 已完成。如果是首次标记，返回 true；否则返回 false。
    /// 当所有 topic 都完成时，触发同步收尾命令。
    pub async fn mark_completed(
        &self,
        topic: &TopicKey,
        logger: &Arc<Mutex<SyncLogger>>,
        tx: &mpsc::UnboundedSender<SyncCommand>,
        app_handle: &AppHandle,
        quiet: bool,
    ) -> bool {
        let mut completed = self.completed.lock().await;
        let is_new = completed.insert(topic.clone());
        if is_new {
            let done = completed.len();
            let total = self.total.load(Ordering::SeqCst);

            if !quiet {
                if let Ok(mut logger) = logger.lock() {
                    logger.log(
                        LogLevel::Debug,
                        "messages",
                        &format!("topic:{} -> success", topic.topic_id),
                    );
                }
            }

            // 发送实时进度事件
            let _ = app_handle.emit(
                "vcp-sync-progress",
                json!({
                    "sessionId": self.session_id,
                    "phase": "messages",
                    "total": total,
                    "completed": done,
                    "successfulTopics": done,
                    "totalTopics": total,
                    "failedTopics": self.failed.lock().await.len(),
                    "legacyAttachmentWarnings": self.legacy_attachment_warnings.load(Ordering::SeqCst)
                }),
            );

            if done == total {
                if let Ok(mut logger) = logger.lock() {
                    logger.log(LogLevel::Info, "messages", "Phase 3 completed");
                }
                let _ = tx.send(SyncCommand::Finalize {
                    attempt_id: self.attempt_id,
                });
            }
            true
        } else {
            false
        }
    }
}

pub enum SyncCommand {
    StartAvatarMetadata {
        attempt_id: u64,
    }, // Internal owner-metadata durability barrier
    StartTopicMetadata {
        attempt_id: u64,
    }, // Phase 2 start
    StartTopicValidation {
        attempt_id: u64,
    }, // Phase 2.5 start
    StartMessages {
        attempt_id: u64,
    }, // Phase 3 start
    Finalize {
        attempt_id: u64,
    }, // Current attempt only
    NotifyDelete {
        target: DeleteTarget,
        deleted_at: i64,
    },
    StartManualSync,
    SendMessageDiff {
        attempt_id: u64,
        topics: Vec<MessageDiffTopicState>,
    },
    Phase3BatchFinished {
        attempt_id: u64,
        result:
            Result<(), crate::vcp_modules::sync_executor::batch_diff_handler::Phase3ProtocolError>,
    },
    FailAttempt {
        attempt_id: u64,
        code: &'static str,
        message: String,
    },
    FailAttemptDetailed {
        attempt_id: u64,
        code: String,
        message: String,
        failed_topic_ids: Vec<String>,
    },
    Cancel,
}

fn validate_unique_topic_keys(
    values: Vec<TopicKey>,
    field: &str,
    max_items: usize,
) -> Result<Vec<TopicKey>, String> {
    if values.len() > max_items {
        return Err(format!("{field} exceeds {max_items} item budget"));
    }
    let mut seen = HashSet::new();
    let mut result = Vec::with_capacity(values.len());
    for key in values {
        if !key.is_valid() {
            return Err(format!("{field} contains an invalid topic identity"));
        }
        if !seen.insert(key.clone()) {
            return Err(format!("{field} contains a duplicate topic identity"));
        }
        result.push(key);
    }
    Ok(result)
}

fn build_sync_error_payload(
    code: &str,
    failed_topic_ids: Vec<String>,
    log_file: Option<String>,
) -> SyncErrorPayload {
    build_local_error_payload(code, failed_topic_ids, log_file)
}

fn encode_sync_command_error(code: &str, detail: &str) -> String {
    log::error!(
        "[SyncCommand] [{}] {}",
        code,
        redact_sync_diagnostic(detail)
    );
    let payload = build_sync_error_payload(code, Vec::new(), None);
    match serde_json::to_string(&payload) {
        Ok(json) => format!("SYNC_ERROR:{json}"),
        Err(_) => "SYNC_ERROR:{\"code\":\"SYNC_ATTEMPT_FAILED\",\"category\":\"internal\",\"origin\":\"mobile_sync\",\"stage\":\"startup\",\"retryAction\":\"manual\",\"message\":\"同步组件未能正常完成本次任务\",\"guidance\":\"重启应用后重新同步；若仍失败，请保留最新日志。\",\"failedTopicIds\":[],\"logFile\":null}".to_string(),
    }
}

async fn publish_sync_nonterminal_status<R: Runtime>(
    app_handle: &AppHandle<R>,
    session_id: u64,
    status: &Arc<RwLock<String>>,
    next_status: &str,
) {
    let sync_state = app_handle.state::<SyncState>();
    let _owner_commit = sync_state.owner_commit.lock().await;
    if sync_state.current_session_id.load(Ordering::SeqCst) != session_id {
        return;
    }
    publish_sync_status_inner(app_handle, session_id, status, next_status, None).await;
}

async fn publish_sync_error<R: Runtime>(
    app_handle: &AppHandle<R>,
    session_id: u64,
    status: &Arc<RwLock<String>>,
    code: &str,
    message: &str,
    failed_topic_ids: Vec<String>,
) {
    let sync_state = app_handle.state::<SyncState>();
    let _owner_commit = sync_state.owner_commit.lock().await;
    if sync_state.current_session_id.load(Ordering::SeqCst) != session_id {
        return;
    }
    let wire_error = decode_wire_sync_error(message);
    let diagnostic_code = wire_error.as_ref().map_or(code, |wire| wire.code.as_str());
    emit_sync_log(
        app_handle,
        "error",
        &format!("[{diagnostic_code}] {message}"),
    );
    let log_file = sync_state
        .current_log_path
        .read()
        .await
        .as_deref()
        .and_then(|path| std::path::Path::new(path).file_name())
        .map(|name| name.to_string_lossy().into_owned());
    let error = match wire_error {
        Some(wire) => build_wire_error_payload(&wire, failed_topic_ids, log_file),
        None => build_sync_error_payload(code, failed_topic_ids, log_file),
    };
    publish_sync_status_inner(app_handle, session_id, status, "error", Some(error)).await;
}

async fn publish_sync_status_inner<R: Runtime>(
    app_handle: &AppHandle<R>,
    session_id: u64,
    status: &Arc<RwLock<String>>,
    next_status: &str,
    error: Option<SyncErrorPayload>,
) {
    {
        let mut guard = status.write().await;
        if guard.as_str() == next_status {
            return;
        }
        if matches!(
            guard.as_str(),
            "error" | "completed" | "completed_with_warnings"
        ) {
            return;
        }
        *guard = next_status.to_string();
    }

    let mut payload = json!({
        "status": next_status,
        "source": "Sync",
        "sessionId": session_id,
    });
    if let Some(error) = error {
        payload["error"] = json!(error);
    }

    let _ = app_handle.emit("vcp-sync-status", payload);
}

async fn publish_sync_completed(
    app_handle: &AppHandle,
    session_id: u64,
    status: &Arc<RwLock<String>>,
    summary: SyncCompletionSummary,
) -> bool {
    let sync_state = app_handle.state::<SyncState>();
    let _owner_commit = sync_state.owner_commit.lock().await;
    if sync_state.current_session_id.load(Ordering::SeqCst) != session_id {
        return false;
    }

    {
        let mut guard = status.write().await;
        if matches!(
            guard.as_str(),
            "error" | "completed" | "completed_with_warnings"
        ) {
            return false;
        }
        *guard = if summary.legacy_attachment_warnings > 0 {
            "completed_with_warnings".to_string()
        } else {
            "completed".to_string()
        };
    }

    let terminal_status = if summary.legacy_attachment_warnings > 0 {
        "completed_with_warnings"
    } else {
        "completed"
    };
    let _ = app_handle.emit(
        "vcp-sync-completed",
        json!({
            "source": "Sync",
            "sessionId": session_id,
            "status": terminal_status,
            "summary": summary,
        }),
    );
    let _ = app_handle.emit(
        "vcp-sync-status",
        json!({
            "status": terminal_status,
            "source": "Sync",
            "sessionId": session_id,
        }),
    );
    true
}

#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SyncCompletionSummary {
    successful_topics: usize,
    total_topics: usize,
    failed_topics: usize,
    legacy_attachment_warnings: usize,
    failed_topic_ids: Vec<String>,
}

pub fn init_sync_service(_app_handle: AppHandle) -> SyncState {
    SyncState {
        ws_sender: SyncCommandRouter::default(),
        connection_status: Arc::new(RwLock::new(String::from("disconnected"))),
        current_log_path: Arc::new(RwLock::new(None)),
        current_logger: Arc::new(std::sync::RwLock::new(None)),
        lifecycle: AsyncMutex::new(()),
        owner_commit: AsyncMutex::new(()),
        session: AsyncMutex::new(None),
        next_session_id: AtomicU64::new(0),
        current_session_id: AtomicU64::new(0),
    }
}

async fn cancelled_during(token: &CancellationToken, duration: Duration) -> bool {
    tokio::select! {
        biased;
        _ = token.cancelled() => true,
        _ = tokio::time::sleep(duration) => false,
    }
}

fn parse_sync_endpoint(
    raw: &str,
    label: &str,
    allowed_schemes: &[&str],
) -> Result<url::Url, SyncConfigValidationError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(SyncConfigValidationError::new(
            "SYNC_CONFIG_MISSING",
            format!("{label} is empty"),
        ));
    }
    let endpoint = url::Url::parse(raw).map_err(|error| {
        SyncConfigValidationError::new(
            "SYNC_CONFIG_INVALID",
            format!("{label} is not a valid URL: {error}"),
        )
    })?;
    if !allowed_schemes.contains(&endpoint.scheme()) || endpoint.host().is_none() {
        return Err(SyncConfigValidationError::new(
            "SYNC_CONFIG_INVALID",
            format!(
                "{label} must use {} and include a host",
                allowed_schemes.join(" or ")
            ),
        ));
    }
    Ok(endpoint)
}

fn endpoint_has_loopback_host(endpoint: &url::Url) -> bool {
    match endpoint.host() {
        Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    }
}

fn build_sync_session_config(
    settings: &Settings,
    is_android: bool,
) -> Result<SyncSessionConfig, SyncConfigValidationError> {
    let mut ws_endpoint =
        parse_sync_endpoint(&settings.sync_server_url, "WebSocket URL", &["ws", "wss"])?;
    let http_endpoint =
        parse_sync_endpoint(&settings.sync_http_url, "HTTP URL", &["http", "https"])?;
    if settings.sync_token.trim().is_empty() {
        return Err(SyncConfigValidationError::new(
            "SYNC_TOKEN_MISSING",
            "sync token is empty",
        ));
    }
    if ws_endpoint.fragment().is_some()
        || http_endpoint.query().is_some()
        || http_endpoint.fragment().is_some()
    {
        return Err(SyncConfigValidationError::new(
            "SYNC_CONFIG_INVALID",
            "sync endpoint contains an unsupported query or fragment",
        ));
    }
    if is_android
        && (endpoint_has_loopback_host(&ws_endpoint) || endpoint_has_loopback_host(&http_endpoint))
    {
        return Err(SyncConfigValidationError::new(
            "CONFIG_LOOPBACK_ON_MOBILE",
            "sync endpoint resolves to the Android device loopback interface",
        ));
    }

    ws_endpoint.set_query(None);
    ws_endpoint
        .query_pairs_mut()
        .append_pair("token", &settings.sync_token);

    Ok(SyncSessionConfig {
        ws_url: ws_endpoint.to_string(),
        http_url: http_endpoint.as_str().trim_end_matches('/').to_string(),
        sync_token: settings.sync_token.clone(),
        sync_prerender_enabled: settings.sync_prerender_enabled,
        sync_log_level: settings.sync_log_level.clone(),
    })
}

fn classify_connection_failure(err: &tokio_tungstenite::tungstenite::error::Error) -> &'static str {
    if let tokio_tungstenite::tungstenite::error::Error::Http(response) = err {
        let status = response.status();
        if status == 401 || status == 403 {
            return "TOKEN_MISMATCH";
        } else if status == 404 {
            return "WS_PATH_INVALID";
        } else {
            return "HTTP_HANDSHAKE_REJECTED";
        }
    }

    "CONNECTION_REFUSED"
}

async fn run_sync_session(
    app_handle: AppHandle,
    session_id: u64,
    session_config: SyncSessionConfig,
    cancel_token: CancellationToken,
    tx: mpsc::UnboundedSender<SyncCommand>,
    mut rx: mpsc::UnboundedReceiver<SyncCommand>,
    connection_status: Arc<RwLock<String>>,
) -> Result<(), String> {
    let handle_clone = app_handle.clone();
    let tx_internal = tx.clone();
    let connection_status_for_task = connection_status.clone();
    let SyncSessionConfig {
        ws_url,
        http_url,
        sync_token,
        sync_prerender_enabled,
        sync_log_level: configured_log_level,
    } = session_config;

    let http_client = match reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(SYNC_HTTP_REQUEST_TIMEOUT)
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            let message = format!("Failed to initialize sync HTTP client: {error}");
            publish_sync_error(
                &app_handle,
                session_id,
                &connection_status,
                "HTTP_CLIENT_INIT_FAILED",
                &message,
                Vec::new(),
            )
            .await;
            return Ok(());
        }
    };
    let mut retry_count = 0u32;
    let mut retry_delay = Duration::from_millis(500);
    let mut next_attempt_id = 0u64;

    let db = app_handle.state::<DbState>();
    let mut write_queue = DbWriteQueue::new(db.pool.clone(), db.path.clone());
    let sync_log_level = LogLevel::parse(&configured_log_level).unwrap_or(LogLevel::Info);
    let invalid_log_level = LogLevel::parse(&configured_log_level).is_none();
    let log_dir = app_handle
        .path()
        .app_log_dir()
        .ok()
        .map(|d| d.join("sync_logs"));
    let sync_logger = Arc::new(std::sync::Mutex::new(SyncLogger::new_session(
        sync_log_level,
        log_dir,
        session_id,
    )));
    let (log_path, log_initialization_error) = {
        let logger = sync_logger.lock();
        match logger {
            Ok(logger) => (
                logger.log_path().cloned(),
                logger.initialization_error().map(str::to_string),
            ),
            Err(_) => (None, Some("Sync logger state lock is poisoned".to_string())),
        }
    };
    {
        let sync_state = app_handle.state::<SyncState>();
        let _owner_commit = sync_state.owner_commit.lock().await;
        if sync_state.current_session_id.load(Ordering::SeqCst) != session_id {
            return Ok(());
        }
        let mut path_guard = sync_state.current_log_path.write().await;
        *path_guard = log_path.map(|path| path.to_string_lossy().to_string());
        drop(path_guard);
        let mut logger_guard = sync_state.current_logger.write().unwrap();
        *logger_guard = Some(sync_logger.clone());
    }
    if invalid_log_level {
        emit_sync_log(
            &app_handle,
            "warning",
            &format!(
                "Unsupported sync log level '{}'; falling back to INFO",
                configured_log_level
            ),
        );
    }
    if let Some(detail) = log_initialization_error {
        log::warn!(
            "[SyncLogger] Session {} continues without a diagnostic file: {}",
            session_id,
            redact_sync_diagnostic(&detail)
        );
        emit_operator_sync_log(
            &app_handle,
            session_id,
            "warning",
            "本次未生成诊断日志，同步过程仍可继续",
        );
    }
    write_queue.set_logger(sync_logger.clone());
    let write_queue = Arc::new(write_queue);

    #[cfg(target_os = "android")]
    let sync_guardian_acquired = match tauri_plugin_vcp_mobile::stream::start_stream_service_inner(
        &app_handle,
        SYNC_GUARDIAN_LABEL,
    ) {
        Ok(()) => true,
        Err(error) => {
            let message = format!(
                "Failed to acquire Android sync foreground lease; continuing without keepalive: {}",
                redact_sync_diagnostic(&error)
            );
            log::warn!("[SyncService] Session {session_id}: {message}");
            emit_sync_log(&app_handle, "warning", &message);
            emit_operator_sync_log(
                &app_handle,
                session_id,
                "warning",
                "后台保活申请失败，本次同步仍将继续",
            );
            false
        }
    };
    #[cfg(not(target_os = "android"))]
    let sync_guardian_acquired = true;

    let write_queue_task = write_queue.clone();
    let sync_logger_task = sync_logger.clone();

    'session: loop {
        if cancel_token.is_cancelled() {
            break;
        }

        publish_sync_nonterminal_status(
            &handle_clone,
            session_id,
            &connection_status_for_task,
            "connecting",
        )
        .await;

        let phase_gate: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));

        let connect_result = tokio::select! {
            biased;
            _ = cancel_token.cancelled() => break,
            result = tokio::time::timeout(WS_CONNECT_TIMEOUT, connect_async(&ws_url)) => result,
        };

        match connect_result {
            Ok(Ok((mut ws_stream, _))) => {
                // ── 版本验证握手 ──
                {
                    let version_req = VersionCheckFrame {
                        frame_type: "VERSION_CHECK",
                        mobile_version: env!("CARGO_PKG_VERSION"),
                        protocol_version: WIRE_PROTOCOL_VERSION,
                    };
                    if let Err(error) = send_ws_frame(&mut ws_stream, &version_req).await {
                        terminate_after_protocol_send_failure(
                            &handle_clone,
                            &mut ws_stream,
                            "version check",
                            &error,
                        )
                        .await;
                        let message = protocol_send_failure_message("version check", &error);
                        if schedule_sync_retry(
                            &handle_clone,
                            session_id,
                            &connection_status_for_task,
                            &cancel_token,
                            &mut retry_count,
                            &mut retry_delay,
                            "WS_SEND_FAILED",
                            &message,
                        )
                        .await
                        {
                            continue 'session;
                        }
                        break 'session;
                    }
                    emit_sync_log(&handle_clone, "info", "正在验证桌面端插件版本...");

                    let version_result = tokio::select! {
                        biased;
                        _ = cancel_token.cancelled() => {
                            let _ = close_ws_with_deadline(&mut ws_stream).await;
                            break;
                        }
                        result = tokio::time::timeout(VERSION_CHECK_TIMEOUT, async {
                            while let Some(res) = ws_stream.next().await {
                                match res {
                                    Ok(Message::Text(text)) => {
                                        match parse_version_handshake_text(&text)? {
                                            Some(ack) => return Ok(ack),
                                            None => continue,
                                        }
                                    }
                                    Ok(Message::Close(close_frame)) => {
                                        return Err(match close_frame {
                                            Some(frame) => VersionHandshakeError::Closed {
                                                code: Some(frame.code.into()),
                                                reason: frame.reason.to_string(),
                                            },
                                            None => VersionHandshakeError::Closed {
                                                code: None,
                                                reason: String::new(),
                                            },
                                        });
                                    }
                                    Err(error) => {
                                        return Err(VersionHandshakeError::Transport(
                                            error.to_string(),
                                        ));
                                    }
                                    _ => {}
                                }
                            }
                            Err(VersionHandshakeError::Closed {
                                code: None,
                                reason: String::new(),
                            })
                        }) => result,
                    };

                    match version_result {
                        Ok(Ok(version_ack)) => {
                            if is_wire_compatible(&version_ack) {
                                emit_sync_log(
                                    &handle_clone,
                                    "success",
                                    &format!(
                                        "桌面端插件 v{} / 同步协议 {} 验证通过",
                                        version_ack.plugin_version, version_ack.protocol_version
                                    ),
                                );
                            } else {
                                publish_sync_error(
                                    &handle_clone,
                                    session_id,
                                    &connection_status_for_task,
                                    "SYNC_VERSION_INCOMPATIBLE",
                                    &format!(
                                        "桌面端插件 v{} 声明的同步协议 {} 与 Mobile 要求的协议 {} 不兼容",
                                        version_ack.plugin_version,
                                        version_ack.protocol_version,
                                        WIRE_PROTOCOL_VERSION,
                                    ),
                                    Vec::new(),
                                )
                                .await;
                                emit_sync_log(
                                    &handle_clone,
                                    "error",
                                    &format!(
                                        "❌ 同步协议不匹配: 桌面端插件 v{} / 协议 {}，Mobile 要求协议 {}",
                                        version_ack.plugin_version,
                                        version_ack.protocol_version,
                                        WIRE_PROTOCOL_VERSION,
                                    ),
                                );
                                emit_sync_log(&handle_clone, "error", "👉 排查建议: 请前往 https://github.com/MRiecy/VCPMobile/releases 下载最新同步插件");
                                break;
                            }
                        }
                        Ok(Err(VersionHandshakeError::Protocol(message))) => {
                            emit_sync_log(
                                &handle_clone,
                                "error",
                                &format!("❌ 同步连接失败 [VERSION_ACK_INVALID]: {message}"),
                            );
                            publish_sync_error(
                                &handle_clone,
                                session_id,
                                &connection_status_for_task,
                                "VERSION_ACK_INVALID",
                                &message,
                                Vec::new(),
                            )
                            .await;
                            let _ = close_ws_with_deadline(&mut ws_stream).await;
                            break;
                        }
                        Ok(Err(VersionHandshakeError::Remote(encoded))) => {
                            publish_sync_error(
                                &handle_clone,
                                session_id,
                                &connection_status_for_task,
                                "REMOTE_SYNC_FAILED",
                                &encoded,
                                Vec::new(),
                            )
                            .await;
                            let _ = close_ws_with_deadline(&mut ws_stream).await;
                            break;
                        }
                        Ok(Err(VersionHandshakeError::Closed { code, reason })) => {
                            if code == Some(4001) {
                                emit_sync_log(
                                    &handle_clone,
                                    "error",
                                    "❌ 同步连接失败 [TOKEN_MISMATCH]: 身份认证失败（Token 错误）",
                                );
                                emit_sync_log(&handle_clone, "error", "👉 排查建议: 移动端设置的同步令牌与桌面端不匹配。请检查移动端设置中的『同步令牌』是否与电脑端 VCPMobileSync 插件的 config.env 中的 SYNC_TOKEN 完全一致。");
                                publish_sync_error(
                                    &handle_clone,
                                    session_id,
                                    &connection_status_for_task,
                                    "TOKEN_MISMATCH",
                                    "身份认证失败（Token 错误）",
                                    Vec::new(),
                                )
                                .await;
                                break 'session;
                            } else {
                                let err_msg = format!(
                                    "连接被服务器关闭 (code: {}, reason: {})",
                                    code.map_or_else(
                                        || "none".to_string(),
                                        |value| value.to_string()
                                    ),
                                    reason
                                );
                                emit_sync_log(
                                    &handle_clone,
                                    "warning",
                                    &format!("同步握手连接关闭 [WS_CLOSED]: {}", err_msg),
                                );
                                let _ = close_ws_with_deadline(&mut ws_stream).await;
                                if schedule_sync_retry(
                                    &handle_clone,
                                    session_id,
                                    &connection_status_for_task,
                                    &cancel_token,
                                    &mut retry_count,
                                    &mut retry_delay,
                                    "WS_CLOSED",
                                    &err_msg,
                                )
                                .await
                                {
                                    continue 'session;
                                }
                                break 'session;
                            }
                        }
                        Ok(Err(VersionHandshakeError::Transport(message))) => {
                            emit_sync_log(
                                &handle_clone,
                                "warning",
                                &format!("同步握手接收失败 [WS_RECEIVE_FAILED]: {message}"),
                            );
                            let _ = close_ws_with_deadline(&mut ws_stream).await;
                            if schedule_sync_retry(
                                &handle_clone,
                                session_id,
                                &connection_status_for_task,
                                &cancel_token,
                                &mut retry_count,
                                &mut retry_delay,
                                "WS_RECEIVE_FAILED",
                                &message,
                            )
                            .await
                            {
                                continue 'session;
                            }
                            break 'session;
                        }
                        Err(_) => {
                            emit_sync_log(
                                &handle_clone,
                                "warning",
                                "同步握手超时 [VERSION_CHECK_TIMEOUT]",
                            );
                            let _ = close_ws_with_deadline(&mut ws_stream).await;
                            if schedule_sync_retry(
                                &handle_clone,
                                session_id,
                                &connection_status_for_task,
                                &cancel_token,
                                &mut retry_count,
                                &mut retry_delay,
                                "VERSION_CHECK_TIMEOUT",
                                "版本验证超时",
                            )
                            .await
                            {
                                continue 'session;
                            }
                            break 'session;
                        }
                    }
                }

                if let Ok(mut logger) = sync_logger_task.lock() {
                    logger.log(LogLevel::Info, "sync", "=== Phase 1: Owner Metadata ===");
                }
                if let Err(error) = send_ws_frame(
                    &mut ws_stream,
                    &PhaseFrame {
                        frame_type: "PHASE_START",
                        phase: SyncPhase::OwnerMetadata,
                    },
                )
                .await
                {
                    terminate_after_protocol_send_failure(
                        &handle_clone,
                        &mut ws_stream,
                        "owner metadata phase start",
                        &error,
                    )
                    .await;
                    let message =
                        protocol_send_failure_message("owner metadata phase start", &error);
                    if schedule_sync_retry(
                        &handle_clone,
                        session_id,
                        &connection_status_for_task,
                        &cancel_token,
                        &mut retry_count,
                        &mut retry_delay,
                        "WS_SEND_FAILED",
                        &message,
                    )
                    .await
                    {
                        continue 'session;
                    }
                    break 'session;
                }
                publish_sync_nonterminal_status(
                    &handle_clone,
                    session_id,
                    &connection_status_for_task,
                    "open",
                )
                .await;
                emit_sync_phase_activity(&handle_clone, session_id, "owner_metadata");

                // Every reconnect gets a fresh owner set. Cancelling and joining this tracker
                // before retry prevents late phase commands and writes from crossing attempts.
                next_attempt_id = next_attempt_id.wrapping_add(1);
                let attempt_id = next_attempt_id;
                let attempt_cancel = cancel_token.child_token();
                let task_tracker = Arc::new(SyncTaskTracker::new(attempt_cancel.clone()));
                let (pipeline_tx, mut pipeline_rx) = mpsc::unbounded_channel::<
                    crate::vcp_modules::sync_pipeline::pipeline::PipelineCommand,
                >();
                let pipeline_task = Arc::new(SyncPipeline::new(pipeline_tx));
                let pending_tasks_task = Arc::new(AtomicU32::new(0));
                let total_tasks_task = Arc::new(AtomicU32::new(0));
                let pending_msg_topics_task = Arc::new(Phase3Tracker {
                    session_id,
                    attempt_id,
                    completed: tokio::sync::Mutex::new(HashSet::new()),
                    modified: tokio::sync::Mutex::new(HashSet::new()),
                    failed: tokio::sync::Mutex::new(HashSet::new()),
                    legacy_attachment_warnings: std::sync::atomic::AtomicUsize::new(0),
                    total: std::sync::atomic::AtomicUsize::new(0),
                });
                let expected_phase3_batch =
                    Arc::new(tokio::sync::Mutex::new(HashSet::<TopicKey>::new()));
                let expected_topic_hash_results =
                    Arc::new(tokio::sync::Mutex::new(None::<HashSet<TopicKey>>));
                let phase3_batch_inflight = Arc::new(AtomicBool::new(false));
                let awaiting_final_ack: PendingFinalAck = Arc::new(Mutex::new(None));

                // Phase3 分批 diff 的待发送批次队列
                let pending_diff_batches: Arc<
                    tokio::sync::Mutex<std::collections::VecDeque<Phase3DiffBatch>>,
                > = Arc::new(tokio::sync::Mutex::new(std::collections::VecDeque::new()));

                // Phase 2 筛选出的需要消息同步的 topic 列表
                let changed_topics: Arc<tokio::sync::Mutex<Vec<TopicKey>>> =
                    Arc::new(tokio::sync::Mutex::new(Vec::new()));

                // V2: Phase 1 筛选出的内容有变动的 owner (Agent/Group) 列表
                let changed_owners: Arc<tokio::sync::Mutex<HashSet<OwnerKey>>> =
                    Arc::new(tokio::sync::Mutex::new(HashSet::new()));

                // 用于跟踪 manifest diff 结果是否全部收到，防止 total_ops=0 时 Phase 1 卡住
                let expected_manifest_count = Arc::new(AtomicU32::new(0));
                let manifest_responses_received = Arc::new(AtomicU32::new(0));
                let expected_manifest_types = Arc::new(Mutex::new(HashSet::<ManifestType>::new()));
                // 1: 基础 Metadata (agent, group, avatar), 2: Topic Metadata
                let manifest_phase = Arc::new(AtomicU8::new(1));
                let mut fatal_error = false;
                let mut sync_success = false;
                let mut heartbeat_interval = tokio::time::interval(Duration::from_secs(15));

                'attempt: loop {
                    tokio::select! {
                        biased;
                        _ = cancel_token.cancelled() => {
                            let _ = close_ws_with_deadline(&mut ws_stream).await;
                            break;
                        }
                        _ = heartbeat_interval.tick() => {
                            if let Err(error) = send_ws_with_deadline(&mut ws_stream, Message::Ping(vec![].into())).await {
                                terminate_after_protocol_send_failure(
                                    &handle_clone,
                                    &mut ws_stream,
                                    "WebSocket heartbeat",
                                    &error,
                                ).await;
                                break 'attempt;
                            }
                        }
                        Some(cmd) = pipeline_rx.recv() => {
                            match cmd {
                                crate::vcp_modules::sync_pipeline::pipeline::PipelineCommand::StartTopicMetadata => {
                                    // Phase 2: 拉取缺失的 Topic Configs
                                    emit_sync_phase_activity(&handle_clone, session_id, "topic_metadata");
                                    let db = handle_clone.state::<DbState>();
                                    let owners = {
                                        let guard = changed_owners.lock().await;
                                        guard.iter().cloned().collect::<Vec<OwnerKey>>()
                                    };

                                    if owners.is_empty() {
                                        expected_manifest_count.store(0, Ordering::SeqCst);
                                        manifest_responses_received.store(0, Ordering::SeqCst);
                                        if let Ok(mut expected) = expected_manifest_types.lock() {
                                            expected.clear();
                                        }
                                        let _ = tx_internal.send(SyncCommand::StartTopicValidation { attempt_id });
                                    } else {
                                        match Phase1Metadata::build_targeted_topic_manifest(&db.pool, &owners).await {
                                            Ok(manifest) => {
                                            if manifest.manifest_type() != ManifestType::Topic {
                                                let _ = tx_internal.send(SyncCommand::FailAttemptDetailed {
                                                    attempt_id,
                                                    code: "TOPIC_MANIFEST_INVALID".to_string(),
                                                    message: "Targeted topic manifest returned an unexpected data type".to_string(),
                                                    failed_topic_ids: Vec::new(),
                                                });
                                                continue 'attempt;
                                            }
                                            manifest_phase.store(3, Ordering::SeqCst);
                                            expected_manifest_count.store(1, Ordering::SeqCst);
                                            manifest_responses_received.store(0, Ordering::SeqCst);
                                            match expected_manifest_types.lock() {
                                                Ok(mut expected) => {
                                                    *expected = HashSet::from([ManifestType::Topic]);
                                                }
                                                Err(_) => {
                                                    let _ = tx_internal.send(SyncCommand::FailAttemptDetailed {
                                                        attempt_id,
                                                        code: "SYNC_STATE_POISONED".to_string(),
                                                        message: "Expected manifest type state is poisoned".to_string(),
                                                        failed_topic_ids: Vec::new(),
                                                    });
                                                    continue 'attempt;
                                                }
                                            }
                                            pending_tasks_task.store(0, Ordering::SeqCst);
                                            total_tasks_task.store(0, Ordering::SeqCst);

                                            if let Ok(mut logger) = sync_logger_task.lock() {
                                                logger.log(LogLevel::Info, "topic_metadata", "=== Phase 2: Pulling Topic Metadata ===");
                                            }
                                            if let Err(error) = send_ws_frame(
                                                &mut ws_stream,
                                                &PhaseFrame {
                                                    frame_type: "PHASE_START",
                                                    phase: SyncPhase::TopicMetadata,
                                                },
                                            ).await {
                                                terminate_after_protocol_send_failure(
                                                    &handle_clone,
                                                    &mut ws_stream,
                                                    "topic metadata phase start",
                                                    &error,
                                                ).await;
                                                break 'attempt;
                                            }

                                            let frame = ManifestRequestFrame::new(manifest);
                                            if let Err(error) = send_ws_frame(&mut ws_stream, &frame).await {
                                                terminate_after_protocol_send_failure(
                                                    &handle_clone,
                                                    &mut ws_stream,
                                                    "topic metadata manifest",
                                                    &error,
                                                ).await;
                                                break 'attempt;
                                            }
                                            task_tracker
                                                .spawn(enforce_manifest_response_deadline(
                                                    expected_manifest_types.clone(),
                                                    manifest_phase.clone(),
                                                    3,
                                                    tx_internal.clone(),
                                                    attempt_id,
                                                    PHASE_RESPONSE_TIMEOUT,
                                                ))
                                                .await;
                                            }
                                            Err(error) => {
                                                let _ = tx_internal.send(SyncCommand::FailAttemptDetailed {
                                                    attempt_id,
                                                    code: "TOPIC_MANIFEST_DB_FAILED".to_string(),
                                                    message: format!("Failed to build targeted topic manifest: {error}"),
                                                    failed_topic_ids: Vec::new(),
                                                });
                                            }
                                        }
                                    }
                                },
                                crate::vcp_modules::sync_pipeline::pipeline::PipelineCommand::StartTopicValidation => {
                                    // Phase 2.5: 双哈希批量比对
                                    emit_sync_phase_activity(&handle_clone, session_id, "topic_validation");
                                    if let Ok(mut logger) = sync_logger_task.lock() {
                                        logger.log(LogLevel::Info, "topic_metadata", "=== Phase 2.5: Validating Topic Hashes ===");
                                    }

                                    let db = handle_clone.state::<DbState>();
                                    let owners = {
                                        let guard = changed_owners.lock().await;
                                        guard.iter().cloned().collect::<Vec<OwnerKey>>()
                                    };

                                    match Phase3Message::get_targeted_topic_hashes(&db.pool, &owners).await {
                                        Ok(topic_hashes) => {
                                            if topic_hashes.len() > MAX_SYNC_TOPICS {
                                                let _ = tx_internal.send(SyncCommand::FailAttemptDetailed {
                                                    attempt_id,
                                                    code: "TOPIC_HASH_BUDGET_EXCEEDED".to_string(),
                                                    message: format!("Topic hash batch exceeds {MAX_SYNC_TOPICS} topic budget"),
                                                    failed_topic_ids: Vec::new(),
                                                });
                                                continue 'attempt;
                                            }
                                            let mut topic_states = Vec::new();
                                            let mut expected_topics = HashSet::new();
                                            for (key, state) in topic_hashes {
                                                expected_topics.insert(key.clone());
                                                let owner_type = match OwnerType::try_from(key.owner_type.as_str()) {
                                                    Ok(owner_type) => owner_type,
                                                    Err(_) => {
                                                        let _ = tx_internal.send(SyncCommand::FailAttemptDetailed {
                                                            attempt_id,
                                                            code: "TOPIC_HASH_STATE_INVALID".to_string(),
                                                            message: format!("Topic {} has invalid ownerType", key.topic_id),
                                                            failed_topic_ids: vec![key.topic_id],
                                                        });
                                                        continue 'attempt;
                                                    }
                                                };
                                                topic_states.push(TopicDiffState {
                                                    owner_type,
                                                    owner_id: key.owner_id,
                                                    topic_id: key.topic_id,
                                                    config_hash: state.config_hash,
                                                    content_hash: state.content_hash,
                                                });
                                            }
                                            {
                                                let mut expected = expected_topic_hash_results.lock().await;
                                                if expected.is_some() {
                                                    let _ = tx_internal.send(SyncCommand::FailAttemptDetailed {
                                                        attempt_id,
                                                        code: "TOPIC_HASH_RESPONSE_OVERLAP".to_string(),
                                                        message: "A topic hash response is already pending".to_string(),
                                                        failed_topic_ids: Vec::new(),
                                                    });
                                                    continue 'attempt;
                                                }
                                                *expected = Some(expected_topics);
                                            }
                                            let frame = TopicDiffRequestFrame::new(topic_states);
                                            if let Err(error) = send_ws_frame(&mut ws_stream, &frame).await {
                                                terminate_after_protocol_send_failure(
                                                    &handle_clone,
                                                    &mut ws_stream,
                                                    "topic hash batch",
                                                    &error,
                                                ).await;
                                                break 'attempt;
                                            }
                                            task_tracker
                                                .spawn(enforce_topic_hash_response_deadline(
                                                    expected_topic_hash_results.clone(),
                                                    manifest_phase.clone(),
                                                    3,
                                                    tx_internal.clone(),
                                                    attempt_id,
                                                    PHASE_RESPONSE_TIMEOUT,
                                                ))
                                                .await;
                                        }
                                        Err(e) => {
                                            log::error!("[SyncService] Failed to get targeted topic hashes: {}", e);
                                            let _ = tx_internal.send(SyncCommand::FailAttemptDetailed {
                                                attempt_id,
                                                code: "TOPIC_HASH_DB_FAILED".to_string(),
                                                message: format!("Failed to get targeted topic hashes: {e}"),
                                                failed_topic_ids: Vec::new(),
                                            });
                                        }
                                    }
                                },
                                crate::vcp_modules::sync_pipeline::pipeline::PipelineCommand::StartMessages => {
                                    emit_sync_phase_activity(&handle_clone, session_id, "messages");
                                    if let Ok(mut logger) = sync_logger_task.lock() {
                                        logger.log(LogLevel::Info, "messages", "=== Phase 3: Messages ===");
                                    }
                                    if let Err(error) = send_ws_frame(
                                        &mut ws_stream,
                                        &PhaseFrame {
                                            frame_type: "PHASE_START",
                                            phase: SyncPhase::Messages,
                                        },
                                    ).await {
                                        terminate_after_protocol_send_failure(
                                            &handle_clone,
                                            &mut ws_stream,
                                            "messages phase start",
                                            &error,
                                        ).await;
                                        break 'attempt;
                                    }

                                    let db = handle_clone.state::<DbState>();
                                    let changed_ids = {
                                        let guard = changed_topics.lock().await;
                                        guard.clone()
                                    };

                                    if changed_ids.is_empty() {
                                        if let Ok(mut logger) = sync_logger_task.lock() {
                                            logger.log(LogLevel::Info, "messages", "Phase 3 skipped: no changed topics");
                                        }
                                        emit_sync_log(&handle_clone, "success", "Message phase skipped (no changed topics), proceeding to hash alignment");
                                        let _ = tx_internal.send(SyncCommand::Finalize { attempt_id });
                                    } else {
                                        match Phase3Message::get_topic_message_hashes(&db.pool, &changed_ids).await {
                                            Ok(topic_states) => {
                                                let topic_count = topic_states.len();
                                                pending_msg_topics_task.total.store(topic_count, Ordering::SeqCst);
                                                {
                                                    let mut completed = pending_msg_topics_task.completed.lock().await;
                                                    completed.clear();
                                                }
                                                {
                                                    let mut modified = pending_msg_topics_task.modified.lock().await;
                                                    modified.clear();
                                                }
                                                {
                                                    let mut failed = pending_msg_topics_task.failed.lock().await;
                                                    failed.clear();
                                                }
                                                pending_msg_topics_task
                                                    .legacy_attachment_warnings
                                                    .store(0, Ordering::SeqCst);
                                                let _ = handle_clone.emit(
                                                    "vcp-sync-progress",
                                                    json!({
                                                        "sessionId": session_id,
                                                        "phase": "messages",
                                                        "total": topic_count,
                                                        "completed": 0,
                                                        "successfulTopics": 0,
                                                        "totalTopics": topic_count,
                                                        "failedTopics": 0,
                                                        "legacyAttachmentWarnings": 0,
                                                    }),
                                                );

                                                // 清空可能残留的旧批次，防止断线重连后发送过时数据
                                                {
                                                    let mut pending = pending_diff_batches.lock().await;
                                                    pending.clear();
                                                }
                                                // 按消息数量分批，每批最多 10000 条消息，避免超大 WS payload
                                                let mut batches = match build_diff_batches(topic_states) {
                                                    Ok(batches) => batches,
                                                    Err(error) => {
                                                        let _ = tx_internal.send(SyncCommand::FailAttemptDetailed {
                                                            attempt_id,
                                                            code: "PHASE3_DIFF_BUDGET_EXCEEDED".to_string(),
                                                            message: error,
                                                            failed_topic_ids: changed_ids.iter().take(8).map(|key| key.topic_id.clone()).collect(),
                                                        });
                                                        continue 'attempt;
                                                    }
                                                };
                                                let batch_count = batches.len();
                                                log::info!("[SyncService] Phase3 diff split into {} batches (max {} msgs/batch)", batch_count, MAX_MESSAGES_PER_BATCH);

                                                let first_batch = batches.pop_front();
                                                {
                                                    let mut pending = pending_diff_batches.lock().await;
                                                    *pending = batches;
                                                }

                                                if let Some(batch) = first_batch {
                                                    {
                                                        let mut expected = expected_phase3_batch.lock().await;
                                                        *expected = batch.keys.clone();
                                                    }
                                                    let frame = MessageDiffRequestFrame::new(batch.topics);
                                                    if let Err(error) = send_ws_frame(
                                                        &mut ws_stream,
                                                        &frame,
                                                    ).await {
                                                        terminate_after_protocol_send_failure(
                                                            &handle_clone,
                                                            &mut ws_stream,
                                                            "message diff batch",
                                                            &error,
                                                        ).await;
                                                        break 'attempt;
                                                    }

                                                } else {
                                                    let _ = tx_internal.send(SyncCommand::FailAttempt {
                                                        attempt_id,
                                                        code: "PHASE3_DIFF_MISSING",
                                                        message: "Phase 3 produced no request batch for changed topics".to_string(),
                                                    });
                                                }
                                            }
                                            Err(e) => {
                                                log::error!("[SyncService] Failed to get topic message hashes: {}", e);
                                                let _ = tx_internal.send(SyncCommand::FailAttempt {
                                                    attempt_id,
                                                    code: "PHASE3_HASH_PREP_FAILED",
                                                    message: format!("Phase 3 hash preparation failed: {}", e),
                                                });
                                            }
                                        }
                                    }
                                },
                            }
                        },
                        Some(cmd) = rx.recv() => {
                            match cmd {
                                SyncCommand::Cancel => {
                                    let _ = close_ws_with_deadline(&mut ws_stream).await;
                                    break;
                                },
                                SyncCommand::StartAvatarMetadata { attempt_id: command_attempt } => {
                                    if command_attempt != attempt_id { continue; }
                                    let should_start = {
                                        if let Ok(mut gate) = phase_gate.lock() {
                                            gate.insert("avatar_metadata".to_string())
                                        } else {
                                            false
                                        }
                                    };
                                    if should_start {
                                        if let Err(error) = write_queue_task.flush().await {
                                            let message = format!(
                                                "Owner metadata write drain before avatars failed: {error}"
                                            );
                                            fatal_error = true;
                                            publish_sync_error(
                                                &handle_clone,
                                                session_id,
                                                &connection_status_for_task,
                                                "OWNER_METADATA_DRAIN_FAILED",
                                                &message,
                                                Vec::new(),
                                            ).await;
                                            let _ = close_ws_with_deadline(&mut ws_stream).await;
                                            break;
                                        }
                                        crate::vcp_modules::sync::sync_finalize::invalidate_sync_entity_caches(
                                            &handle_clone,
                                        );

                                        let db = handle_clone.state::<DbState>();
                                        let manifest = match Phase1Metadata::build_avatar_manifest(&db.pool).await {
                                            Ok(manifest) => manifest,
                                            Err(error) => {
                                                let _ = tx_internal.send(SyncCommand::FailAttemptDetailed {
                                                    attempt_id,
                                                    code: "AVATAR_MANIFEST_DB_FAILED".to_string(),
                                                    message: format!("Failed to build avatar manifest: {error}"),
                                                    failed_topic_ids: Vec::new(),
                                                });
                                                continue;
                                            }
                                        };
                                        manifest_phase.store(2, Ordering::SeqCst);
                                        expected_manifest_count.store(1, Ordering::SeqCst);
                                        manifest_responses_received.store(0, Ordering::SeqCst);
                                        match expected_manifest_types.lock() {
                                            Ok(mut expected) => {
                                                *expected = HashSet::from([ManifestType::Avatar]);
                                            }
                                            Err(_) => {
                                                let _ = tx_internal.send(SyncCommand::FailAttemptDetailed {
                                                    attempt_id,
                                                    code: "SYNC_STATE_POISONED".to_string(),
                                                    message: "Expected avatar manifest state is poisoned".to_string(),
                                                    failed_topic_ids: Vec::new(),
                                                });
                                                continue;
                                            }
                                        }
                                        pending_tasks_task.store(0, Ordering::SeqCst);
                                        total_tasks_task.store(0, Ordering::SeqCst);
                                        let frame = ManifestRequestFrame::new(manifest);
                                        if let Err(error) = send_ws_frame(
                                            &mut ws_stream,
                                            &frame,
                                        ).await {
                                            terminate_after_protocol_send_failure(
                                                &handle_clone,
                                                &mut ws_stream,
                                                "avatar metadata manifest",
                                                &error,
                                            ).await;
                                            break 'attempt;
                                        }
                                        task_tracker
                                            .spawn(enforce_manifest_response_deadline(
                                                expected_manifest_types.clone(),
                                                manifest_phase.clone(),
                                                2,
                                                tx_internal.clone(),
                                                attempt_id,
                                                PHASE_RESPONSE_TIMEOUT,
                                            ))
                                            .await;
                                    }
                                },
                                SyncCommand::StartTopicMetadata { attempt_id: command_attempt } => {
                                    if command_attempt != attempt_id { continue; }
                                    let should_flush = {
                                        if let Ok(mut gate) = phase_gate.lock() {
                                            gate.insert("topic_metadata".to_string())
                                        } else {
                                            false
                                        }
                                    };
                                    if should_flush {
                                        if let Err(error) = write_queue_task.flush().await {
                                            let message = format!("Owner metadata write drain failed: {}", error);
                                            fatal_error = true;
                                            emit_sync_log(&handle_clone, "error", &message);
                                            publish_sync_error(
                                                &handle_clone,
                                                session_id,
                                                &connection_status_for_task,
                                                "OWNER_METADATA_DRAIN_FAILED",
                                                &message,
                                                Vec::new(),
                                            ).await;
                                            let _ = close_ws_with_deadline(&mut ws_stream).await;
                                            break;
                                        }
                                        crate::vcp_modules::sync::sync_finalize::invalidate_sync_entity_caches(
                                            &handle_clone,
                                        );
                                        if let Err(error) = send_ws_frame(
                                            &mut ws_stream,
                                            &PhaseFrame {
                                                frame_type: "PHASE_COMPLETED",
                                                phase: SyncPhase::OwnerMetadata,
                                            },
                                        ).await {
                                            terminate_after_protocol_send_failure(
                                                &handle_clone,
                                                &mut ws_stream,
                                                "owner metadata phase completion",
                                                &error,
                                            ).await;
                                            break 'attempt;
                                        }
                                        let _ = pipeline_task.on_owner_metadata_done();
                                    }
                                },
                                SyncCommand::StartTopicValidation { attempt_id: command_attempt } => {
                                    if command_attempt != attempt_id { continue; }
                                    let should_flush = {
                                        if let Ok(mut gate) = phase_gate.lock() {
                                            gate.insert("topic_validation".to_string())
                                        } else {
                                            false
                                        }
                                    };
                                    if should_flush {
                                        if let Err(error) = write_queue_task.flush().await {
                                            let message = format!("Topic metadata write drain failed: {}", error);
                                            fatal_error = true;
                                            emit_sync_log(&handle_clone, "error", &message);
                                            publish_sync_error(
                                                &handle_clone,
                                                session_id,
                                                &connection_status_for_task,
                                                "TOPIC_METADATA_DRAIN_FAILED",
                                                &message,
                                                Vec::new(),
                                            ).await;
                                            let _ = close_ws_with_deadline(&mut ws_stream).await;
                                            break;
                                        }
                                        crate::vcp_modules::sync::sync_finalize::invalidate_sync_entity_caches(
                                            &handle_clone,
                                        );
                                        if let Err(error) = send_ws_frame(
                                            &mut ws_stream,
                                            &PhaseFrame {
                                                frame_type: "PHASE_COMPLETED",
                                                phase: SyncPhase::TopicMetadata,
                                            },
                                        ).await {
                                            terminate_after_protocol_send_failure(
                                                &handle_clone,
                                                &mut ws_stream,
                                                "topic metadata phase completion",
                                                &error,
                                            ).await;
                                            break 'attempt;
                                        }
                                        let _ = pipeline_task.on_topic_metadata_pull_done();
                                    }
                                },
                                SyncCommand::StartMessages { attempt_id: command_attempt } => {
                                    if command_attempt != attempt_id { continue; }
                                    let should_flush = {
                                        if let Ok(mut gate) = phase_gate.lock() {
                                            gate.insert("messages".to_string())
                                        } else {
                                            false
                                        }
                                    };
                                    if should_flush {
                                        if let Err(error) = write_queue_task.flush().await {
                                            let message = format!("Topic validation write drain failed: {}", error);
                                            fatal_error = true;
                                            emit_sync_log(&handle_clone, "error", &message);
                                            publish_sync_error(
                                                &handle_clone,
                                                session_id,
                                                &connection_status_for_task,
                                                "TOPIC_VALIDATION_DRAIN_FAILED",
                                                &message,
                                                Vec::new(),
                                            ).await;
                                            let _ = close_ws_with_deadline(&mut ws_stream).await;
                                            break;
                                        }
                                        let _ = pipeline_task.on_topic_validation_done();
                                    }
                                },
                                SyncCommand::Finalize { attempt_id: command_attempt } => {
                                    if command_attempt != attempt_id { continue; }
                                    let should_flush = {
                                        if let Ok(mut gate) = phase_gate.lock() {
                                            gate.insert("finalize".to_string())
                                        } else {
                                            false
                                        }
                                    };
                                    if should_flush {
                                        emit_sync_phase_activity(&handle_clone, session_id, "finalize");
                                        // 先完成落盘与哈希收尾，成功后才能对桌面端确认相位完成。
                                        let db = handle_clone.state::<DbState>();
                                        let modified_topics = {
                                            let guard = pending_msg_topics_task.modified.lock().await;
                                            guard.clone()
                                        };
                                        if let Err(e) = crate::vcp_modules::sync::sync_finalize::SyncFinalizer::execute(
                                            &handle_clone,
                                            &db,
                                            &write_queue_task,
                                            modified_topics,
                                        ).await {
                                            let message = format!("Sync finalization failed: {}", e);
                                            fatal_error = true;
                                            emit_sync_log(&handle_clone, "error", &message);
                                            publish_sync_error(
                                                &handle_clone,
                                                session_id,
                                                &connection_status_for_task,
                                                "SYNC_FINALIZATION_FAILED",
                                                &message,
                                                Vec::new(),
                                            ).await;
                                            let _ = close_ws_with_deadline(&mut ws_stream).await;
                                            break;
                                        }
                                        let final_ack = FinalAckKey::new(session_id, attempt_id);
                                        match send_ws_frame(&mut ws_stream, &final_ack.message()).await
                                        {
                                            Ok(()) => {
                                                if let Ok(mut pending) = awaiting_final_ack.lock() {
                                                    *pending = Some(final_ack.clone());
                                                } else {
                                                    let _ = tx_internal.send(SyncCommand::FailAttempt {
                                                        attempt_id,
                                                        code: "SYNC_STATE_POISONED",
                                                        message: "Final acknowledgement state lock is poisoned".to_string(),
                                                    });
                                                    continue;
                                                }
                                                let pending = awaiting_final_ack.clone();
                                                let tx_watchdog = tx_internal.clone();
                                                task_tracker
                                                    .spawn(enforce_final_ack_deadline(
                                                        pending,
                                                        final_ack,
                                                        tx_watchdog,
                                                        FINAL_ACK_TIMEOUT,
                                                    ))
                                                    .await;
                                            }
                                            Err(error) => {
                                                let _ = tx_internal.send(SyncCommand::FailAttempt {
                                                    attempt_id,
                                                    code: "WS_SEND_FAILED",
                                                    message: format!(
                                                        "Failed to send final messages phase completion: {}",
                                                        error
                                                    ),
                                                });
                                            }
                                        }
                                    }
                                },
                                SyncCommand::NotifyDelete { target, deleted_at } => {
                                    let frame = DeleteNotificationFrame::new(target, deleted_at);
                                    if let Err(error) = send_ws_frame(&mut ws_stream, &frame).await {
                                        terminate_after_protocol_send_failure(
                                            &handle_clone,
                                            &mut ws_stream,
                                            "local message deletion",
                                            &error,
                                        ).await;
                                        break 'attempt;
                                    }
                                },
                                SyncCommand::StartManualSync => {
                                    let db = handle_clone.state::<DbState>();
                                    manifest_phase.store(1, Ordering::SeqCst);
                                    match Phase1Metadata::build_owner_manifest(&db.pool).await {
                                        Ok(manifest) => {
                                            expected_manifest_count.store(1, Ordering::SeqCst);
                                            manifest_responses_received.store(0, Ordering::SeqCst);
                                            match expected_manifest_types.lock() {
                                                Ok(mut expected) => {
                                                    *expected = HashSet::from([ManifestType::Owner]);
                                                }
                                                Err(_) => {
                                                    let _ = tx_internal.send(SyncCommand::FailAttemptDetailed {
                                                        attempt_id,
                                                        code: "SYNC_STATE_POISONED".to_string(),
                                                        message: "Expected manifest type state is poisoned".to_string(),
                                                        failed_topic_ids: Vec::new(),
                                                    });
                                                    continue 'attempt;
                                                }
                                            }

                                            let frame = ManifestRequestFrame::new(manifest);
                                            if let Err(error) = send_ws_frame(&mut ws_stream, &frame).await {
                                                terminate_after_protocol_send_failure(
                                                    &handle_clone,
                                                    &mut ws_stream,
                                                    "owner metadata manifest",
                                                    &error,
                                                ).await;
                                                break 'attempt;
                                            }
                                            task_tracker
                                                .spawn(enforce_manifest_response_deadline(
                                                    expected_manifest_types.clone(),
                                                    manifest_phase.clone(),
                                                    1,
                                                    tx_internal.clone(),
                                                    attempt_id,
                                                    PHASE_RESPONSE_TIMEOUT,
                                                ))
                                                .await;
                                        }
                                        Err(error) => {
                                            let _ = tx_internal.send(SyncCommand::FailAttemptDetailed {
                                                attempt_id,
                                                code: "OWNER_MANIFEST_DB_FAILED".to_string(),
                                                message: format!("Failed to build Owner manifest: {error}"),
                                                failed_topic_ids: Vec::new(),
                                            });
                                        }
                                    }
                                },
                                SyncCommand::SendMessageDiff { attempt_id: command_attempt, topics } => {
                                    if command_attempt != attempt_id { continue; }
                                    let frame = MessageDiffRequestFrame::new(topics);
                                    if let Err(error) = send_ws_frame(&mut ws_stream, &frame).await {
                                        terminate_after_protocol_send_failure(
                                            &handle_clone,
                                            &mut ws_stream,
                                            "queued sync protocol message",
                                            &error,
                                        ).await;
                                        break 'attempt;
                                    }
                                },
                                SyncCommand::Phase3BatchFinished {
                                    attempt_id: command_attempt,
                                    result,
                                } => {
                                    if command_attempt != attempt_id { continue; }
                                    phase3_batch_inflight.store(false, Ordering::SeqCst);
                                    if let Err(error) = result {
                                        log::error!("[SyncService] BatchDiffHandler failed: {}", error);
                                        fatal_error = true;
                                        let failure_summary = pending_msg_topics_task
                                            .completion_summary()
                                            .await;
                                        let _ = handle_clone.emit(
                                            "vcp-sync-progress",
                                            json!({
                                                "sessionId": session_id,
                                                "phase": "messages",
                                                "total": failure_summary.total_topics,
                                                "completed": failure_summary.successful_topics,
                                                "successfulTopics": failure_summary.successful_topics,
                                                "totalTopics": failure_summary.total_topics,
                                                "failedTopics": failure_summary.failed_topics,
                                                "legacyAttachmentWarnings": failure_summary.legacy_attachment_warnings,
                                            }),
                                        );
                                        emit_sync_log(&handle_clone, "error", &error.message);
                                        publish_sync_error(
                                            &handle_clone,
                                            session_id,
                                            &connection_status_for_task,
                                            &error.code,
                                            &error.message,
                                            error.failed_topic_ids,
                                        ).await;
                                        let _ = close_ws_with_deadline(&mut ws_stream).await;
                                        break 'attempt;
                                    }
                                },
                                SyncCommand::FailAttempt { attempt_id: command_attempt, code, message } => {
                                    if command_attempt != attempt_id { continue; }
                                    fatal_error = true;
                                    emit_sync_log(&handle_clone, "error", &message);
                                    publish_sync_error(
                                        &handle_clone,
                                        session_id,
                                        &connection_status_for_task,
                                        code,
                                        &message,
                                        Vec::new(),
                                    ).await;
                                    let _ = close_ws_with_deadline(&mut ws_stream).await;
                                    break;
                                },
                                SyncCommand::FailAttemptDetailed {
                                    attempt_id: command_attempt,
                                    code,
                                    message,
                                    failed_topic_ids,
                                } => {
                                    if command_attempt != attempt_id { continue; }
                                    fatal_error = true;
                                    emit_sync_log(&handle_clone, "error", &message);
                                    publish_sync_error(
                                        &handle_clone,
                                        session_id,
                                        &connection_status_for_task,
                                        &code,
                                        &message,
                                        failed_topic_ids,
                                    ).await;
                                    let _ = close_ws_with_deadline(&mut ws_stream).await;
                                    break;
                                },
                            }
                        },
                        res = ws_stream.next() => {
                            match res {
                                Some(Ok(msg)) => {
                                    match msg {
                                        Message::Text(text) => {
                                let header = match serde_json::from_str::<FrameHeader>(&text) {
                                    Ok(header) if !header.frame_type.is_empty() => header,
                                    Ok(_) => {
                                        let message = "Sync protocol frame requires a non-empty string type";
                                        fatal_error = true;
                                        publish_sync_error(
                                            &handle_clone,
                                            session_id,
                                            &connection_status_for_task,
                                            "PROTOCOL_FRAME_INVALID",
                                            message,
                                            Vec::new(),
                                        ).await;
                                        let _ = close_ws_with_deadline(&mut ws_stream).await;
                                        break;
                                    }
                                    Err(error) => {
                                        let message = format!("Malformed sync protocol frame: {error}");
                                        fatal_error = true;
                                        publish_sync_error(
                                            &handle_clone,
                                            session_id,
                                            &connection_status_for_task,
                                            "PROTOCOL_FRAME_INVALID",
                                            &message,
                                            Vec::new(),
                                        ).await;
                                        let _ = close_ws_with_deadline(&mut ws_stream).await;
                                        break;
                                    }
                                };
                                let h = handle_clone.clone();
                                let c = http_client.clone();
                                let base = http_url.clone();
                                let wq = write_queue_task.clone();

                                match header.frame_type.as_str() {
                                    "SYNC_ERROR" => {
                                        let encoded = serde_json::from_str::<RemoteSyncErrorFrame>(&text)
                                            .map_err(|error| format!("Invalid SYNC_ERROR: {error}"))
                                            .and_then(|frame| encode_wire_sync_error(&frame.error));
                                        let encoded = match encoded {
                                            Ok(encoded) => encoded,
                                            Err(message) => {
                                                fatal_error = true;
                                                publish_sync_error(
                                                    &handle_clone,
                                                    session_id,
                                                    &connection_status_for_task,
                                                    "PROTOCOL_FRAME_INVALID",
                                                    &message,
                                                    Vec::new(),
                                                ).await;
                                                let _ = close_ws_with_deadline(&mut ws_stream).await;
                                                break;
                                            }
                                        };
                                        publish_sync_error(
                                            &handle_clone,
                                            session_id,
                                            &connection_status_for_task,
                                            "REMOTE_SYNC_FAILED",
                                            &encoded,
                                            Vec::new(),
                                        ).await;
                                        fatal_error = true;
                                        let _ = close_ws_with_deadline(&mut ws_stream).await;
                                        break;
                                    },
                                    "SYNC_MANIFEST_RESULT" => {
                                        let frame = match serde_json::from_str::<ManifestResultFrame>(&text) {
                                            Ok(frame) => frame,
                                            Err(error) => {
                                                let _ = tx_internal.send(SyncCommand::FailAttempt {
                                                    attempt_id,
                                                    code: "PROTOCOL_FRAME_INVALID",
                                                    message: format!("Invalid SYNC_MANIFEST_RESULT: {error}"),
                                                });
                                                continue;
                                            }
                                        };
                                        if let Err(e) = crate::vcp_modules::sync_executor::diff_handler::DiffHandler::handle_diff(
                                            &h,
                                            frame,
                                            &c,
                                            &base,
                                            &sync_token,
                                            &wq,
                                            &pending_tasks_task,
                                            &total_tasks_task,
                                            &manifest_responses_received,
                                            &expected_manifest_count,
                                            &expected_manifest_types,
                                            &manifest_phase,
                                            &tx_internal,
                                            &changed_owners,
                                            &task_tracker,
                                            session_id,
                                            attempt_id,
                                        ).await {
                                            log::error!("[SyncService] DiffHandler failed: {}", e);
                                            let _ = tx_internal.send(SyncCommand::FailAttempt {
                                                attempt_id,
                                                code: "SYNC_DIFF_HANDLER_FAILED",
                                                message: format!("DiffHandler failed: {e}"),
                                            });
                                        }
                                    },
                                    "SYNC_MESSAGE_DIFF_RESULT" => {
                                        let frame = match crate::vcp_modules::sync_executor::batch_diff_handler::parse_message_diff_result_frame(&text) {
                                            Ok(frame) => frame,
                                            Err(error) => {
                                                fatal_error = true;
                                                emit_sync_log(&handle_clone, "error", &error.message);
                                                publish_sync_error(
                                                    &handle_clone,
                                                    session_id,
                                                    &connection_status_for_task,
                                                    &error.code,
                                                    &error.message,
                                                    error.failed_topic_ids,
                                                ).await;
                                                let _ = close_ws_with_deadline(&mut ws_stream).await;
                                                break;
                                            }
                                        };
                                        if phase3_batch_inflight.swap(true, Ordering::SeqCst) {
                                            fatal_error = true;
                                            let message = "Received a Phase 3 batch while another batch is still in flight";
                                            emit_sync_log(&handle_clone, "error", message);
                                            publish_sync_error(
                                                &handle_clone,
                                                session_id,
                                                &connection_status_for_task,
                                                "PHASE3_BATCH_OVERLAP",
                                                message,
                                                Vec::new(),
                                            ).await;
                                            let _ = close_ws_with_deadline(&mut ws_stream).await;
                                            break;
                                        }
                                        let tracker = pending_msg_topics_task.clone();
                                        let batch_tx = tx_internal.clone();
                                        let handler_tx = tx_internal.clone();
                                        let logger = sync_logger_task.clone();
                                        let write_queue = wq.clone();
                                        let pending_batches = pending_diff_batches.clone();
                                        let expected_topics = expected_phase3_batch.clone();
                                        let token = sync_token.clone();
                                        let prerender_enabled = sync_prerender_enabled;
                                        task_tracker.spawn(async move {
                                            let result = crate::vcp_modules::sync_executor::batch_diff_handler::BatchDiffHandler::handle_diff_batch(
                                                &h,
                                                frame,
                                                &c,
                                                &base,
                                                &token,
                                                &tracker,
                                                &handler_tx,
                                                &logger,
                                                &write_queue,
                                                &pending_batches,
                                                prerender_enabled,
                                                &expected_topics,
                                                attempt_id,
                                            ).await;
                                            let _ = batch_tx.send(SyncCommand::Phase3BatchFinished {
                                                attempt_id,
                                                result,
                                            });
                                        }).await;
                                    },
                                    "SYNC_TOPIC_DIFF_RESULT" => {
                                        manifest_phase.store(4, Ordering::SeqCst); // 进入 Phase 2.5+，结束 manifest 阶段
                                        let expected = expected_topic_hash_results.lock().await.take();
                                        let parsed = match expected {
                                            Some(expected) => serde_json::from_str::<TopicDiffResultFrame>(&text)
                                            .map_err(|error| format!("Invalid SYNC_TOPIC_DIFF_RESULT: {error}"))
                                            .and_then(|frame| validate_unique_topic_keys(
                                                frame.changed_topics,
                                                "SYNC_TOPIC_DIFF_RESULT.changedTopics",
                                                MAX_SYNC_TOPICS,
                                            ))
                                            .and_then(|changed_topics| {
                                                if let Some(unexpected) = changed_topics
                                                    .iter()
                                                    .find(|topic| !expected.contains(*topic))
                                                {
                                                    return Err(format!(
                                                        "SYNC_TOPIC_DIFF_RESULT.changedTopics contains unexpected topic {}/{}/{}",
                                                        unexpected.owner_type,
                                                        unexpected.owner_id,
                                                        unexpected.topic_id,
                                                    ));
                                                }
                                                Ok(changed_topics)
                                            }),
                                            None => Err(
                                                "Received an unexpected or duplicate SYNC_TOPIC_DIFF_RESULT frame"
                                                    .to_string(),
                                            ),
                                        };
                                        match parsed {
                                            Ok(changed_topic_keys) => {
                                                log::info!("[SyncService] Phase 2.5 results: {} topics need message sync", changed_topic_keys.len());
                                                {
                                                    let mut guard = changed_topics.lock().await;
                                                    *guard = changed_topic_keys;
                                                }
                                                let _ = tx_internal.send(SyncCommand::StartMessages { attempt_id });
                                            }
                                            Err(message) => {
                                                let _ = tx_internal.send(SyncCommand::FailAttempt {
                                                    attempt_id,
                                                    code: "TOPIC_HASH_RESULTS_INVALID",
                                                    message,
                                                });
                                            }
                                        }
                                    },
                                    "PHASE_ACK" => {
                                        let ack = match serde_json::from_str::<PhaseAckFrame>(&text) {
                                            Ok(ack) if ack.frame_type == "PHASE_ACK" => ack,
                                            Ok(_) => unreachable!("frame header already matched PHASE_ACK"),
                                            Err(error) => {
                                                let _ = tx_internal.send(SyncCommand::FailAttempt {
                                                    attempt_id,
                                                    code: "PROTOCOL_FRAME_INVALID",
                                                    message: format!("Invalid PHASE_ACK: {error}"),
                                                });
                                                continue;
                                            }
                                        };
                                        if !consume_final_ack(&awaiting_final_ack, &ack) {
                                            log::debug!("[SyncService] Ignoring mismatched, stale, or replayed final acknowledgement");
                                            continue;
                                        }
                                        manifest_phase.store(0, Ordering::SeqCst); // 同步完成，重置 manifest 阶段
                                        if let Err(error) = write_queue_task.flush().await {
                                            let message = format!("Final sync write drain failed: {}", error);
                                            fatal_error = true;
                                            emit_sync_log(&handle_clone, "error", &message);
                                            publish_sync_error(
                                                &handle_clone,
                                                session_id,
                                                &connection_status_for_task,
                                                "FINAL_WRITE_DRAIN_FAILED",
                                                &message,
                                                Vec::new(),
                                            ).await;
                                            let _ = close_ws_with_deadline(&mut ws_stream).await;
                                            break;
                                        }

                                        sync_success = publish_sync_completed(
                                            &handle_clone,
                                            session_id,
                                            &connection_status_for_task,
                                            pending_msg_topics_task.completion_summary().await,
                                        ).await;
                                        if sync_success {
                                            if let Ok(mut logger) = sync_logger_task.lock() {
                                                (*logger).end_session();
                                            }
                                            emit_sync_log(&handle_clone, "success", "同步已完成，所有数据已对齐");

                                        }
                                        let _ = close_ws_with_deadline(&mut ws_stream).await;
                                        break;
                                    },
                                    "SYNC_LOG_EVENT" => {
                                        let event = match serde_json::from_str::<SyncLogEventFrame>(&text) {
                                            Ok(event) => event,
                                            Err(error) => {
                                                let _ = tx_internal.send(SyncCommand::FailAttempt {
                                                    attempt_id,
                                                    code: "PROTOCOL_FRAME_INVALID",
                                                    message: format!("Invalid SYNC_LOG_EVENT: {error}"),
                                                });
                                                continue;
                                            }
                                        };
                                        emit_sync_log(&handle_clone, &event.level, &format!("[Desktop] {}", event.message));
                                    },
                                    "DESKTOP_PHASE_START" | "DESKTOP_PHASE_COMPLETE" => {
                                        let event = match serde_json::from_str::<DesktopPhaseEventFrame>(&text) {
                                            Ok(event) => event,
                                            Err(error) => {
                                                let _ = tx_internal.send(SyncCommand::FailAttempt {
                                                    attempt_id,
                                                    code: "PROTOCOL_FRAME_INVALID",
                                                    message: format!("Invalid desktop phase event: {error}"),
                                                });
                                                continue;
                                            }
                                        };
                                        let msg = match event.frame_type.as_str() {
                                            "DESKTOP_PHASE_START" => format!("[Desktop] Phase {} started", event.phase),
                                            _ => format!("[Desktop] Phase {} completed", event.phase),
                                        };
                                        emit_sync_log(&handle_clone, "info", &msg);
                                    },
                                    _ => {
                                        let _ = tx_internal.send(SyncCommand::FailAttempt {
                                            attempt_id,
                                            code: "PROTOCOL_FRAME_INVALID",
                                            message: format!("Unsupported sync response frame {}", header.frame_type),
                                        });
                                    }
                                    }
                                }
                                Message::Close(close_frame) => {
                                    let mut err_msg = "WebSocket 连接被关闭".to_string();
                                    let mut error_code = "WS_CLOSED".to_string();
                                    let mut solution = "连接已被服务器关闭，请在桌面端控制台查看详细日志。".to_string();
                                    if let Some(frame) = &close_frame {
                                        let code: u16 = frame.code.into();
                                        let reason = &frame.reason;
                                        err_msg = format!("WebSocket 连接被服务器关闭 (code: {}, reason: {})", code, reason);
                                        if code == 4001 {
                                            error_code = "TOKEN_MISMATCH".to_string();
                                            err_msg = "身份认证失败（Token 错误）".to_string();
                                            solution = "移动端设置的同步令牌与桌面端不匹配。请检查移动端设置中的『同步令牌』是否与电脑端 VCPMobileSync 插件的 config.env 中的 SYNC_TOKEN 完全一致。".to_string();
                                            fatal_error = true;
                                        }
                                    }
                                    let level = if fatal_error { "error" } else { "warning" };
                                    emit_sync_log(&handle_clone, level, &format!("同步连接关闭 [{}]: {}", error_code, err_msg));
                                    emit_sync_log(&handle_clone, level, &format!("排查建议: {}", solution));
                                    if fatal_error {
                                        publish_sync_error(
                                            &handle_clone,
                                            session_id,
                                            &connection_status_for_task,
                                            &error_code,
                                            &err_msg,
                                            Vec::new(),
                                        ).await;
                                    }
                                    break;
                                }
                                _ => {}
                            }
                        }
                                Some(Err(e)) => {
                                    let err_msg = format!("WebSocket 接收发生错误: {}", e);
                                    emit_sync_log(&handle_clone, "error", &err_msg);
                                    break;
                                }
                                None => {
                                    let err_msg = "WebSocket 连接意外断开 (服务器关闭连接)";
                                    emit_sync_log(&handle_clone, "error", err_msg);
                                    break;
                                }
                            }
                        }
                        else => break,
                    }
                }
                attempt_cancel.cancel();
                task_tracker.close_and_wait().await;
                if !sync_success {
                    if let Err(error) = write_queue_task.flush().await {
                        let message =
                            format!("Failed to drain database writes before reconnect: {error}");
                        fatal_error = true;
                        emit_sync_log(&handle_clone, "error", &message);
                        publish_sync_error(
                            &handle_clone,
                            session_id,
                            &connection_status_for_task,
                            "RETRY_WRITE_DRAIN_FAILED",
                            &message,
                            Vec::new(),
                        )
                        .await;
                    }
                    let modified_topics = {
                        let guard = pending_msg_topics_task.modified.lock().await;
                        guard.clone()
                    };
                    let db = handle_clone.state::<DbState>();
                    if let Err(error) = crate::vcp_modules::sync::sync_finalize::SyncFinalizer::reconcile_after_interruption(
                        &db,
                        &modified_topics,
                    )
                    .await
                    {
                        let message = format!(
                            "Failed to reconcile database state after interrupted sync: {error}"
                        );
                        fatal_error = true;
                        emit_sync_log(&handle_clone, "error", &message);
                        publish_sync_error(
                            &handle_clone,
                            session_id,
                            &connection_status_for_task,
                            "SYNC_FINALIZATION_FAILED",
                            &message,
                            Vec::new(),
                        )
                        .await;
                    }
                    crate::vcp_modules::sync::sync_finalize::invalidate_sync_entity_caches(
                        &handle_clone,
                    );
                }
                if sync_success {
                    break; // 同步完成，退出外层 loop
                } else {
                    if fatal_error {
                        break;
                    }
                    if schedule_sync_retry(
                        &handle_clone,
                        session_id,
                        &connection_status_for_task,
                        &cancel_token,
                        &mut retry_count,
                        &mut retry_delay,
                        "WS_DISCONNECTED",
                        "同步中途异常断开",
                    )
                    .await
                    {
                        continue;
                    }
                    break;
                }
            }
            Err(_) => {
                let retry_message = format!(
                    "WebSocket connection timed out after {} seconds",
                    WS_CONNECT_TIMEOUT.as_secs()
                );
                if !schedule_sync_retry(
                    &handle_clone,
                    session_id,
                    &connection_status_for_task,
                    &cancel_token,
                    &mut retry_count,
                    &mut retry_delay,
                    "WS_CONNECT_TIMEOUT",
                    &retry_message,
                )
                .await
                {
                    break;
                }
            }
            Ok(Err(e)) => {
                let error_code = classify_connection_failure(&e);
                let error_detail = e.to_string();
                let is_fatal = error_code == "TOKEN_MISMATCH" || error_code == "WS_PATH_INVALID";

                if is_fatal {
                    emit_sync_log(
                        &handle_clone,
                        "error",
                        &format!("❌ 同步连接失败 [{error_code}]: {error_detail}"),
                    );

                    publish_sync_error(
                        &handle_clone,
                        session_id,
                        &connection_status_for_task,
                        error_code,
                        &error_detail,
                        Vec::new(),
                    )
                    .await;
                    break;
                }
                let retry_message = format!("同步端口连接失败: {error_detail}");
                if !schedule_sync_retry(
                    &handle_clone,
                    session_id,
                    &connection_status_for_task,
                    &cancel_token,
                    &mut retry_count,
                    &mut retry_delay,
                    error_code,
                    &retry_message,
                )
                .await
                {
                    break;
                }
            }
        }
    }

    // No session is considered stopped until its children can no longer enqueue
    // writes and the session-local queue has drained everything already accepted.
    cancel_token.cancel();
    let shutdown_result = match write_queue.flush().await {
        Ok(()) => Ok(()),
        Err(error) => {
            let message = format!("Sync session shutdown write drain failed: {error}");
            emit_sync_log(&app_handle, "error", &message);
            publish_sync_error(
                &app_handle,
                session_id,
                &connection_status,
                "SYNC_DB_DRAIN_FAILED",
                &message,
                Vec::new(),
            )
            .await;
            Err(message)
        }
    };
    // 失败 attempt 也可能已有部分实体写入；离开 session 前必须丢弃旧 Facade cache。
    crate::vcp_modules::sync::sync_finalize::invalidate_sync_entity_caches(&app_handle);

    let sync_state = app_handle.state::<SyncState>();
    let _owner_commit = sync_state.owner_commit.lock().await;
    if sync_state.current_session_id.load(Ordering::SeqCst) == session_id {
        sync_state.ws_sender.clear_if_owner(session_id);
        #[cfg(target_os = "android")]
        if sync_guardian_acquired {
            release_sync_guardian_with_diagnostics(&app_handle);
        }
        {
            let mut logger_guard = sync_state.current_logger.write().unwrap();
            *logger_guard = None;
        }
        *sync_state.current_log_path.write().await = None;
    }
    shutdown_result
}

/// Phase 3 diff 同时受消息数和真实 JSON 字节预算约束。
const MAX_MESSAGES_PER_BATCH: usize = 10000;
const MAX_WS_DIFF_BATCH_BYTES: usize = 8 * 1024 * 1024;

struct JsonSizeCounter {
    bytes: usize,
    limit: usize,
}

impl JsonSizeCounter {
    fn new(limit: usize) -> Self {
        Self { bytes: 0, limit }
    }
}

impl Write for JsonSizeCounter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let next = self.bytes.saturating_add(bytes.len());
        if next > self.limit {
            return Err(std::io::Error::other("JSON value exceeds its byte budget"));
        }
        self.bytes = next;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub(crate) struct Phase3DiffBatch {
    pub topics: Vec<MessageDiffTopicState>,
    pub keys: HashSet<TopicKey>,
}

fn build_diff_batches(
    topic_states: std::collections::HashMap<
        TopicKey,
        crate::vcp_modules::sync_pipeline::phase3_message::TopicLocalState,
    >,
) -> Result<std::collections::VecDeque<Phase3DiffBatch>, String> {
    let mut batches = std::collections::VecDeque::new();
    let mut current_topics = Vec::new();
    let mut current_keys = HashSet::new();
    let mut current_msg_count = 0usize;
    let envelope_bytes = br#"{"type":"SYNC_MESSAGE_DIFF_REQUEST","topics":[]}"#.len();
    let mut current_bytes = envelope_bytes;
    let mut topic_states = topic_states.into_iter().collect::<Vec<_>>();
    topic_states.sort_by(|left, right| left.0.cmp(&right.0));

    for (key, state) in topic_states {
        let msg_count = state.messages.len();
        if msg_count > MAX_MESSAGES_PER_BATCH {
            return Err(format!(
                "Phase 3 diff topic {} exceeds the {MAX_MESSAGES_PER_BATCH}-message batch limit",
                key.topic_id
            ));
        }
        let owner_type = OwnerType::try_from(key.owner_type.as_str())
            .map_err(|_| format!("Phase 3 topic {} has invalid ownerType", key.topic_id))?;
        let topic_obj = MessageDiffTopicState {
            owner_type,
            owner_id: key.owner_id.clone(),
            topic_id: key.topic_id.clone(),
            content_hash: state.content_hash,
            messages: state.messages,
        };
        let mut counter = JsonSizeCounter::new(MAX_WS_DIFF_BATCH_BYTES);
        serde_json::to_writer(&mut counter, &topic_obj)
            .map_err(|error| format!("Failed to size Phase 3 topic {}: {error}", key.topic_id))?;
        let entry_bytes = counter.bytes;
        if envelope_bytes.saturating_add(entry_bytes) > MAX_WS_DIFF_BATCH_BYTES {
            return Err(format!(
                "Phase 3 diff topic {} exceeds the 8 MiB WebSocket frame limit",
                key.topic_id
            ));
        }

        let separator_bytes = usize::from(!current_topics.is_empty());
        if !current_topics.is_empty()
            && (current_msg_count.saturating_add(msg_count) > MAX_MESSAGES_PER_BATCH
                || current_bytes
                    .saturating_add(separator_bytes)
                    .saturating_add(entry_bytes)
                    > MAX_WS_DIFF_BATCH_BYTES)
        {
            batches.push_back(Phase3DiffBatch {
                topics: current_topics,
                keys: current_keys,
            });
            current_topics = Vec::new();
            current_keys = HashSet::new();
            current_msg_count = 0;
            current_bytes = envelope_bytes;
        }

        current_bytes = current_bytes
            .saturating_add(usize::from(!current_topics.is_empty()))
            .saturating_add(entry_bytes);
        current_keys.insert(key);
        current_topics.push(topic_obj);
        current_msg_count = current_msg_count.saturating_add(msg_count);
    }

    if !current_topics.is_empty() {
        batches.push_back(Phase3DiffBatch {
            topics: current_topics,
            keys: current_keys,
        });
    }

    Ok(batches)
}

pub(crate) fn emit_sync_log<R: Runtime>(app_handle: &AppHandle<R>, level: &str, message: &str) {
    let sync_state = app_handle.state::<SyncState>();
    if let Some(logger_arc) = sync_state
        .current_logger
        .read()
        .ok()
        .and_then(|guard| guard.clone())
    {
        if let Ok(mut logger) = logger_arc.lock() {
            let log_level = match level {
                "trace" => LogLevel::Trace,
                "debug" => LogLevel::Debug,
                "error" => LogLevel::Error,
                "warn" | "warning" => LogLevel::Warning,
                _ => LogLevel::Info,
            };
            logger.log(log_level, "sync", message);
        }
    } else {
        let safe_message = redact_sync_diagnostic(message);
        let rust_log_level = match level {
            "trace" => log::Level::Trace,
            "debug" => log::Level::Debug,
            "error" => log::Level::Error,
            "warn" | "warning" => log::Level::Warn,
            _ => log::Level::Info,
        };
        log::log!(rust_log_level, "[Sync] [{}] {}", level, safe_message);
    }
}

fn emit_sync_phase_activity<R: Runtime>(app_handle: &AppHandle<R>, session_id: u64, phase: &str) {
    let _ = app_handle.emit(
        "vcp-sync-progress",
        json!({
            "sessionId": session_id,
            "phase": phase,
            "total": 0,
            "completed": 0,
        }),
    );
}

fn emit_operator_sync_log<R: Runtime>(
    app_handle: &AppHandle<R>,
    session_id: u64,
    level: &str,
    message: &str,
) {
    let _ = app_handle.emit(
        "vcp-log",
        serde_json::json!({
            "id": format!("{}_{}", level, chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)),
            "level": level,
            "category": "sync",
            "audience": "operator",
            "sessionId": session_id,
            "message": message,
        }),
    );
}

#[cfg(target_os = "android")]
fn release_sync_guardian_with_diagnostics(app_handle: &AppHandle) {
    if let Err(error) =
        tauri_plugin_vcp_mobile::stream::stop_stream_service_inner(app_handle, SYNC_GUARDIAN_LABEL)
    {
        emit_sync_log(
            app_handle,
            "warning",
            &format!(
                "Failed to release Android sync foreground lease: {}",
                redact_sync_diagnostic(&error)
            ),
        );
    }
}

#[tauri::command]
pub async fn stop_sync(
    #[allow(unused_variables)] handle: AppHandle,
    state: State<'_, SyncState>,
) -> Result<(), String> {
    let _lifecycle_guard = state.lifecycle.lock().await;

    // Invalidate the old owner before signalling it. Every late status/finally
    // path now observes a different generation and loses commit authority.
    {
        let _owner_commit = state.owner_commit.lock().await;
        let generation = state.next_session_id.fetch_add(1, Ordering::SeqCst) + 1;
        state.current_session_id.store(generation, Ordering::SeqCst);
        state.ws_sender.clear();
    }

    let session = state.session.lock().await.take();
    let join_result = if let Some(session) = session {
        cancel_and_join_session(session).await
    } else {
        Ok(())
    };

    {
        let mut guard = state.connection_status.write().await;
        *guard = "disconnected".to_string();
    }
    #[cfg(target_os = "android")]
    release_sync_guardian_with_diagnostics(&handle);
    let logger_clear_result = state
        .current_logger
        .write()
        .map(|mut logger_guard| *logger_guard = None)
        .map_err(|_| {
            encode_sync_command_error(
                "SYNC_STATE_POISONED",
                "Sync logger state lock is poisoned while stopping",
            )
        });
    *state.current_log_path.write().await = None;

    join_result.map_err(|detail| encode_sync_command_error("SYNC_STOP_FAILED", &detail))?;
    logger_clear_result?;
    Ok(())
}

async fn cancel_and_join_session(session: SyncSessionHandle) -> Result<(), String> {
    log::info!("[SyncService] Cancelling session {}", session.session_id);
    session.cancel_token.cancel();
    let _ = session.command_tx.send(SyncCommand::Cancel);
    session
        .join_handle
        .await
        .map_err(|error| format!("同步会话退出失败: {error}"))?
}

#[tauri::command]
pub async fn get_sync_status(state: State<'_, SyncState>) -> Result<String, String> {
    Ok(state.connection_status.read().await.clone())
}

#[tauri::command]
pub async fn start_manual_sync(
    handle: AppHandle,
    state: State<'_, SyncState>,
) -> Result<u64, String> {
    let _lifecycle_guard = state.lifecycle.lock().await;

    let finished_session = {
        let mut session = state.session.lock().await;
        if let Some(active) = session.as_ref() {
            if !active.join_handle.is_finished() {
                return Err(encode_sync_command_error(
                    "SYNC_ALREADY_RUNNING",
                    "A sync session is already running",
                ));
            }
        }
        session.take()
    };
    if let Some(finished_session) = finished_session {
        match finished_session.join_handle.await {
            Ok(Ok(())) => {}
            Ok(Err(detail)) => {
                return Err(encode_sync_command_error(
                    "SYNC_PREVIOUS_SESSION_EXIT_FAILED",
                    &detail,
                ));
            }
            Err(error) => {
                return Err(encode_sync_command_error(
                    "SYNC_PREVIOUS_SESSION_EXIT_FAILED",
                    &error.to_string(),
                ));
            }
        }
    }

    let db = handle.state::<DbState>();
    let has_active_generation =
        sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM active_generations)")
            .fetch_one(&db.pool)
            .await
            .map_err(|error| {
                encode_sync_command_error(
                    "SYNC_ATTEMPT_FAILED",
                    &format!("Active generation preflight failed: {error}"),
                )
            })?;
    if has_active_generation {
        return Err(encode_sync_command_error(
            "SYNC_ACTIVE_GENERATION",
            "A message generation is still active",
        ));
    }

    let settings_state = handle.state::<SettingsState>();
    let settings = read_settings(handle.clone(), settings_state)
        .await
        .map_err(|error| {
            encode_sync_command_error(
                "SYNC_SETTINGS_READ_FAILED",
                &format!("Failed to read sync settings: {error}"),
            )
        })?;
    let session_config = build_sync_session_config(&settings, cfg!(target_os = "android"))
        .map_err(|error| encode_sync_command_error(error.code, &error.detail))?;

    let (tx, rx) = mpsc::unbounded_channel::<SyncCommand>();
    tx.send(SyncCommand::StartManualSync).map_err(|error| {
        encode_sync_command_error("SYNC_START_CHANNEL_FAILED", &error.to_string())
    })?;
    let session_id = state.next_session_id.fetch_add(1, Ordering::SeqCst) + 1;
    let command_tx = tx.clone();
    {
        let _owner_commit = state.owner_commit.lock().await;
        state.current_session_id.store(session_id, Ordering::SeqCst);
        state.ws_sender.install(session_id, command_tx.clone());
        *state.connection_status.write().await = "disconnected".to_string();
        *state.current_log_path.write().await = None;
    }
    let cancel_token = CancellationToken::new();

    let app_handle = handle.clone();
    let connection_status = state.connection_status.clone();
    let run_cancel_token = cancel_token.clone();
    let join_handle = tokio::spawn(async move {
        run_sync_session(
            app_handle,
            session_id,
            session_config,
            run_cancel_token,
            tx,
            rx,
            connection_status,
        )
        .await
    });

    *state.session.lock().await = Some(SyncSessionHandle {
        session_id,
        cancel_token,
        command_tx,
        join_handle,
    });
    Ok(session_id)
}

#[derive(Debug, serde::Serialize)]
pub struct SyncLogFileInfo {
    pub filename: String,
    pub created_at: u64,
    pub size_bytes: u64,
}

#[tauri::command]
pub async fn list_sync_log_files(app: AppHandle) -> Result<Vec<SyncLogFileInfo>, String> {
    let log_dir = app
        .path()
        .app_log_dir()
        .map_err(|error| encode_sync_command_error("SYNC_LOG_LIST_FAILED", &error.to_string()))?
        .join("sync_logs");
    if !log_dir.exists() {
        return Ok(Vec::new());
    }

    let mut entries = Vec::new();
    let mut read_dir = tokio::fs::read_dir(&log_dir)
        .await
        .map_err(|error| encode_sync_command_error("SYNC_LOG_LIST_FAILED", &error.to_string()))?;
    while let Some(entry) = read_dir
        .next_entry()
        .await
        .map_err(|error| encode_sync_command_error("SYNC_LOG_LIST_FAILED", &error.to_string()))?
    {
        let metadata = entry.metadata().await.map_err(|error| {
            encode_sync_command_error("SYNC_LOG_LIST_FAILED", &error.to_string())
        })?;
        if metadata.is_file() {
            let filename = entry.file_name().to_string_lossy().to_string();
            let created_at = metadata
                .created()
                .or_else(|_| metadata.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            entries.push(SyncLogFileInfo {
                filename,
                created_at,
                size_bytes: metadata.len(),
            });
        }
    }

    entries.sort_by_key(|b| std::cmp::Reverse(b.created_at));
    Ok(entries)
}

#[tauri::command]
pub async fn get_sync_session_log_path(
    state: State<'_, SyncState>,
) -> Result<Option<String>, String> {
    let guard = state.current_log_path.read().await;
    Ok(guard.clone())
}

#[tauri::command]
pub async fn read_sync_log_file(app: AppHandle, filename: String) -> Result<String, String> {
    let log_dir = app
        .path()
        .app_log_dir()
        .map_err(|error| encode_sync_command_error("SYNC_LOG_READ_FAILED", &error.to_string()))?
        .join("sync_logs");
    let file_path = log_dir.join(&filename);

    // 安全检查：确保文件在 sync_logs 目录内
    let canonical_dir = log_dir
        .canonicalize()
        .map_err(|error| encode_sync_command_error("SYNC_LOG_READ_FAILED", &error.to_string()))?;
    let canonical_file = file_path
        .canonicalize()
        .map_err(|error| encode_sync_command_error("SYNC_LOG_READ_FAILED", &error.to_string()))?;
    if !canonical_file.starts_with(&canonical_dir) {
        return Err(encode_sync_command_error(
            "SYNC_LOG_PATH_INVALID",
            "Requested sync log is outside the sync log directory",
        ));
    }

    let content = tokio::fs::read_to_string(&canonical_file)
        .await
        .map_err(|error| encode_sync_command_error("SYNC_LOG_READ_FAILED", &error.to_string()))?;
    Ok(content)
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncLogCleanupResult {
    pub removed: u32,
    pub failed: u32,
}

fn count_log_removal(
    result: std::io::Result<()>,
    removed: &mut u32,
    failed: &mut u32,
) -> Option<std::io::Error> {
    match result {
        Ok(()) => {
            *removed += 1;
            None
        }
        Err(error) => {
            *failed += 1;
            Some(error)
        }
    }
}

#[tauri::command]
pub async fn clear_old_sync_logs(
    app: AppHandle,
    keep_days: u32,
) -> Result<SyncLogCleanupResult, String> {
    let log_dir = app
        .path()
        .app_log_dir()
        .map_err(|error| encode_sync_command_error("SYNC_LOG_CLEAR_FAILED", &error.to_string()))?
        .join("sync_logs");
    if !log_dir.exists() {
        return Ok(SyncLogCleanupResult {
            removed: 0,
            failed: 0,
        });
    }

    let cutoff = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(keep_days as u64 * 86400))
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
    let mut removed = 0u32;
    let mut failed = 0u32;

    let mut read_dir = tokio::fs::read_dir(&log_dir)
        .await
        .map_err(|error| encode_sync_command_error("SYNC_LOG_CLEAR_FAILED", &error.to_string()))?;
    while let Some(entry) = read_dir
        .next_entry()
        .await
        .map_err(|error| encode_sync_command_error("SYNC_LOG_CLEAR_FAILED", &error.to_string()))?
    {
        let metadata = entry.metadata().await.map_err(|error| {
            encode_sync_command_error("SYNC_LOG_CLEAR_FAILED", &error.to_string())
        })?;
        if metadata.is_file() {
            let modified = metadata
                .modified()
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            if modified < cutoff {
                if let Some(error) = count_log_removal(
                    tokio::fs::remove_file(entry.path()).await,
                    &mut removed,
                    &mut failed,
                ) {
                    log::warn!(
                        "[SyncLog] Failed to remove old log: {}",
                        redact_sync_diagnostic(&error.to_string())
                    );
                }
            }
        }
    }

    Ok(SyncLogCleanupResult { removed, failed })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcp_modules::sync_error::SyncErrorCategory;
    use serde_json::Value;
    use std::sync::atomic::AtomicBool;

    #[test]
    fn sync_error_contract_keeps_raw_detail_out_of_the_user_payload() {
        let payload = build_sync_error_payload(
            "TOKEN_MISMATCH",
            vec!["topic-a".to_string()],
            Some("20260813_120000_000_7_sync.log".to_string()),
        );
        let json = serde_json::to_value(payload).expect("serialize sync error");

        assert_eq!(json["category"], "configuration");
        assert_eq!(json["origin"], "mobile_sync");
        assert_eq!(json["stage"], "connect");
        assert_eq!(json["retryAction"], "after_user_action");
        assert_eq!(json["message"], "手机端与电脑端的同步令牌不一致");
        assert_eq!(json["guidance"], "重新核对两端令牌后再试。");
        assert!(json.get("detail").is_none());
        assert!(json.get("solution").is_none());

        let fallback = build_sync_error_payload("desktop raw code", Vec::new(), None);
        assert_eq!(fallback.code, "SYNC_ATTEMPT_FAILED");
        assert_eq!(fallback.category, SyncErrorCategory::Internal);
    }

    #[test]
    fn command_errors_use_the_structured_transport_prefix() {
        let encoded = encode_sync_command_error(
            "SYNC_ACTIVE_GENERATION",
            "Bearer raw-secret should stay in native logs only",
        );
        let json = encoded
            .strip_prefix("SYNC_ERROR:")
            .expect("structured sync command prefix");
        let payload: Value = serde_json::from_str(json).expect("structured sync error JSON");

        assert_eq!(payload["code"], "SYNC_ACTIVE_GENERATION");
        assert_eq!(payload["category"], "data");
        assert!(!encoded.contains("raw-secret"));
    }

    fn valid_sync_settings() -> Settings {
        Settings {
            sync_server_url: "wss://192.168.1.10:5975/ws-sync".to_string(),
            sync_http_url: "https://192.168.1.10:5974".to_string(),
            sync_token: "sync-token".to_string(),
            sync_log_level: "DEBUG".to_string(),
            sync_prerender_enabled: true,
            ..Settings::default()
        }
    }

    fn sync_config_error(settings: &Settings, is_android: bool) -> SyncConfigValidationError {
        match build_sync_session_config(settings, is_android) {
            Ok(_) => panic!("expected invalid sync configuration"),
            Err(error) => error,
        }
    }

    #[test]
    fn session_config_snapshot_is_validated_normalized_and_frozen() {
        let mut settings = valid_sync_settings();
        settings.sync_server_url =
            "wss://192.168.1.10:5975/ws-sync?token=stale&mode=legacy".to_string();
        settings.sync_http_url = "https://192.168.1.10:5974/base/".to_string();
        settings.sync_token = "token +/?".to_string();

        let config = build_sync_session_config(&settings, true)
            .unwrap_or_else(|error| panic!("valid sync configuration rejected: {}", error.detail));
        settings.sync_server_url = "ws://changed.invalid".to_string();
        settings.sync_token = "changed".to_string();

        let ws_url = url::Url::parse(&config.ws_url).expect("validated WebSocket URL");
        let query = ws_url.query_pairs().collect::<Vec<_>>();
        assert_eq!(query.len(), 1);
        assert_eq!(query[0].0, "token");
        assert_eq!(query[0].1, "token +/?");
        assert_eq!(config.http_url, "https://192.168.1.10:5974/base");
        assert_eq!(config.sync_token, "token +/?");
        assert_eq!(config.sync_log_level, "DEBUG");
        assert!(config.sync_prerender_enabled);
    }

    #[test]
    fn session_config_rejects_missing_urls_and_token_before_session_creation() {
        for (field, settings, expected_code) in [
            (
                "WebSocket URL",
                Settings {
                    sync_server_url: "   ".to_string(),
                    ..valid_sync_settings()
                },
                "SYNC_CONFIG_MISSING",
            ),
            (
                "HTTP URL",
                Settings {
                    sync_http_url: String::new(),
                    ..valid_sync_settings()
                },
                "SYNC_CONFIG_MISSING",
            ),
            (
                "token",
                Settings {
                    sync_token: " \t ".to_string(),
                    ..valid_sync_settings()
                },
                "SYNC_TOKEN_MISSING",
            ),
        ] {
            let error = sync_config_error(&settings, true);
            assert_eq!(error.code, expected_code, "code for {field}");
        }
    }

    #[test]
    fn session_config_rejects_invalid_endpoint_shapes() {
        for (field, settings) in [
            (
                "WebSocket scheme",
                Settings {
                    sync_server_url: "https://192.168.1.10:5975/ws-sync".to_string(),
                    ..valid_sync_settings()
                },
            ),
            (
                "HTTP scheme",
                Settings {
                    sync_http_url: "ftp://192.168.1.10:5974".to_string(),
                    ..valid_sync_settings()
                },
            ),
            (
                "WebSocket host",
                Settings {
                    sync_server_url: "ws://".to_string(),
                    ..valid_sync_settings()
                },
            ),
            (
                "HTTP query",
                Settings {
                    sync_http_url: "https://192.168.1.10:5974?mode=legacy".to_string(),
                    ..valid_sync_settings()
                },
            ),
            (
                "WebSocket fragment",
                Settings {
                    sync_server_url: "wss://192.168.1.10:5975/ws-sync#fragment".to_string(),
                    ..valid_sync_settings()
                },
            ),
        ] {
            let error = sync_config_error(&settings, true);
            assert_eq!(error.code, "SYNC_CONFIG_INVALID", "code for {field}");
        }
    }

    #[test]
    fn android_session_config_rejects_both_endpoint_loopbacks() {
        for (field, settings) in [
            (
                "WebSocket localhost",
                Settings {
                    sync_server_url: "ws://localhost:5975".to_string(),
                    ..valid_sync_settings()
                },
            ),
            (
                "WebSocket IPv4 loopback range",
                Settings {
                    sync_server_url: "ws://127.0.0.2:5975".to_string(),
                    ..valid_sync_settings()
                },
            ),
            (
                "HTTP IPv6 loopback",
                Settings {
                    sync_http_url: "http://[::1]:5974".to_string(),
                    ..valid_sync_settings()
                },
            ),
        ] {
            let error = sync_config_error(&settings, true);
            assert_eq!(error.code, "CONFIG_LOOPBACK_ON_MOBILE", "code for {field}");
        }

        let settings = Settings {
            sync_server_url: "ws://localhost:5975".to_string(),
            sync_http_url: "http://127.0.0.2:5974".to_string(),
            ..valid_sync_settings()
        };
        assert!(build_sync_session_config(&settings, false).is_ok());
    }

    #[tokio::test]
    async fn missing_manifest_frame_fails_the_current_attempt() {
        let expected = Arc::new(Mutex::new(HashSet::from([
            ManifestType::Owner,
            ManifestType::Avatar,
        ])));
        let phase = Arc::new(AtomicU8::new(1));
        let (tx, mut rx) = mpsc::unbounded_channel();
        enforce_manifest_response_deadline(expected, phase, 1, tx, 7, Duration::from_millis(1))
            .await;
        match rx.recv().await.expect("deadline command") {
            SyncCommand::FailAttemptDetailed {
                attempt_id,
                code,
                message,
                ..
            } => {
                assert_eq!(attempt_id, 7);
                assert_eq!(code, "MANIFEST_RESPONSE_TIMEOUT");
                assert!(message.contains("owner"));
                assert!(message.contains("avatar"));
            }
            _ => panic!("unexpected deadline command"),
        }
    }

    #[tokio::test]
    async fn missing_topic_hash_frame_fails_the_current_attempt() {
        let expected = Arc::new(AsyncMutex::new(Some(HashSet::from([TopicKey::new(
            "agent", "agent-a", "topic-a",
        )]))));
        let phase = Arc::new(AtomicU8::new(3));
        let (tx, mut rx) = mpsc::unbounded_channel();
        enforce_topic_hash_response_deadline(expected, phase, 3, tx, 8, Duration::from_millis(1))
            .await;
        match rx.recv().await.expect("deadline command") {
            SyncCommand::FailAttemptDetailed {
                attempt_id, code, ..
            } => {
                assert_eq!(attempt_id, 8);
                assert_eq!(code, "TOPIC_HASH_RESPONSE_TIMEOUT");
            }
            _ => panic!("unexpected deadline command"),
        }
    }

    #[test]
    fn version_ack_is_strict_but_compatibility_is_owned_by_the_wire_version() {
        let ack = parse_version_ack(
            &json!({
                "type": "VERSION_ACK",
                "pluginVersion": "1.4.0",
                "protocolVersion": "1.4",
            })
            .to_string(),
        )
        .expect("strict 1.4 acknowledgement");
        assert_eq!(ack.plugin_version, "1.4.0");
        assert_eq!(ack.protocol_version, "1.4");
        assert!(is_wire_compatible(&ack));
        assert!(is_wire_compatible(&VersionAck {
            frame_type: "VERSION_ACK".to_string(),
            plugin_version: "1.4.9".to_string(),
            protocol_version: "1.4".to_string(),
        }));
        assert!(!is_wire_compatible(&VersionAck {
            frame_type: "VERSION_ACK".to_string(),
            plugin_version: "1.4.0".to_string(),
            protocol_version: "1.3".to_string(),
        }));

        assert!(parse_version_ack(
            &json!({
                "type": "VERSION_ACK",
                "version": "1.4.0",
            })
            .to_string()
        )
        .is_err());
        assert!(parse_version_ack(
            &json!({
                "type": "VERSION_ACK",
                "pluginVersion": "1.4.0",
                "protocolVersion": 1.4,
            })
            .to_string()
        )
        .is_err());
    }

    #[test]
    fn handshake_preserves_a_structured_desktop_error_before_version_ack() {
        let result = parse_version_handshake_text(
            &json!({
                "type": "SYNC_ERROR",
                "error": {
                    "code": "PLUGIN_VERSION_MISMATCH",
                    "origin": "desktop_plugin",
                    "stage": "handshake",
                    "kind": "compatibility",
                    "retry": "after_user_action",
                    "message": "plugin package mismatch",
                    "failedTopicIds": []
                }
            })
            .to_string(),
        );
        let VersionHandshakeError::Remote(encoded) = result.expect_err("remote error") else {
            panic!("expected structured remote error");
        };
        assert_eq!(
            decode_wire_sync_error(&encoded)
                .expect("encoded error")
                .code,
            "PLUGIN_VERSION_MISMATCH"
        );
        assert!(parse_version_handshake_text(
            &json!({
                "type": "SYNC_LOG_EVENT",
                "level": "info",
                "phase": "startup",
                "message": "ready",
                "ts": 1
            })
            .to_string()
        )
        .expect("log frame")
        .is_none());
    }

    #[test]
    fn changed_topic_list_requires_unique_compound_identities() {
        let topic_a = TopicKey::new("agent", "agent-a", "topic-a");
        let topic_b = TopicKey::new("group", "group-a", "topic-b");
        assert_eq!(
            validate_unique_topic_keys(
                vec![topic_a.clone(), topic_b.clone()],
                "changedTopics",
                MAX_SYNC_TOPICS,
            )
            .expect("valid topic list"),
            vec![topic_a.clone(), topic_b.clone()]
        );
        assert!(validate_unique_topic_keys(
            vec![TopicKey::new("agent", "agent-a", "")],
            "changedTopics",
            MAX_SYNC_TOPICS,
        )
        .is_err());
        assert!(validate_unique_topic_keys(
            vec![topic_a.clone(), topic_a],
            "changedTopics",
            MAX_SYNC_TOPICS,
        )
        .is_err());
        assert!(validate_unique_topic_keys(
            vec![topic_b, TopicKey::new("agent", "agent-c", "topic-c")],
            "changedTopics",
            1
        )
        .is_err());
    }

    #[test]
    fn phase3_diff_batches_enforce_serialized_byte_budget() {
        use crate::vcp_modules::sync_pipeline::phase3_message::TopicLocalState;
        use crate::vcp_modules::sync_types::{MessageLiveState, MessageVersionState};
        use std::collections::{BTreeMap, HashMap};
        let version = || {
            MessageVersionState::Live(MessageLiveState {
                message_hash: "m".repeat(64),
                updated_at: 1,
            })
        };

        let mut states = HashMap::new();
        for index in 0..3 {
            let key = TopicKey::new("agent", "agent-a", format!("topic-{index}"));
            states.insert(
                key,
                TopicLocalState {
                    content_hash: "h".repeat(64),
                    messages: BTreeMap::from([(
                        format!("message-{index}-{}", "x".repeat(3 * 1024 * 1024)),
                        version(),
                    )]),
                },
            );
        }
        let batches = build_diff_batches(states).expect("bounded batches");
        assert!(batches.len() >= 2);
        for batch in batches {
            let bytes = serde_json::to_vec(&json!({
                "type": "SYNC_MESSAGE_DIFF_REQUEST",
                "topics": batch.topics,
            }))
            .expect("serialize batch");
            assert!(bytes.len() <= MAX_WS_DIFF_BATCH_BYTES);
        }
    }

    #[tokio::test]
    async fn session_task_tracker_cancels_and_joins_children() {
        let cancel_token = CancellationToken::new();
        let tracker = Arc::new(SyncTaskTracker::new(cancel_token.clone()));
        let started = Arc::new(tokio::sync::Notify::new());
        let late_side_effect = Arc::new(AtomicBool::new(false));
        let child_started = started.clone();
        let child_side_effect = late_side_effect.clone();

        tracker
            .spawn(async move {
                child_started.notify_one();
                tokio::time::sleep(Duration::from_secs(30)).await;
                child_side_effect.store(true, Ordering::SeqCst);
            })
            .await;
        started.notified().await;

        cancel_token.cancel();
        tracker.close_and_wait().await;

        assert!(!late_side_effect.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn cancel_session_waits_for_main_task_exit() {
        let cancel_token = CancellationToken::new();
        let task_token = cancel_token.clone();
        let exited = Arc::new(AtomicBool::new(false));
        let task_exited = exited.clone();
        let (command_tx, _command_rx) = mpsc::unbounded_channel();
        let join_handle = tokio::spawn(async move {
            task_token.cancelled().await;
            tokio::time::sleep(Duration::from_millis(10)).await;
            task_exited.store(true, Ordering::SeqCst);
            Ok(())
        });

        cancel_and_join_session(SyncSessionHandle {
            session_id: 1,
            cancel_token,
            command_tx,
            join_handle,
        })
        .await
        .expect("session should exit cleanly");

        assert!(exited.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn final_ack_deadline_fails_only_the_current_pending_attempt() {
        let expected = FinalAckKey {
            session_id: 3,
            attempt_id: 7,
            phase: "messages".into(),
            nonce: "nonce-7".into(),
        };
        let pending = Arc::new(Mutex::new(Some(expected.clone())));
        let (tx, mut rx) = mpsc::unbounded_channel();

        enforce_final_ack_deadline(pending.clone(), expected, tx, Duration::from_millis(1)).await;

        match rx.try_recv() {
            Ok(SyncCommand::FailAttempt {
                attempt_id,
                code,
                message,
            }) => {
                assert_eq!(attempt_id, 7);
                assert_eq!(code, "FINAL_ACK_TIMEOUT");
                assert!(message.contains("acknowledgement timed out"));
            }
            _ => panic!("pending final acknowledgement must fail its attempt"),
        }
        assert!(pending.lock().expect("pending lock").is_none());
    }

    #[tokio::test]
    async fn stale_watchdog_cannot_clear_a_newer_attempt() {
        let stale = FinalAckKey {
            session_id: 3,
            attempt_id: 7,
            phase: "messages".into(),
            nonce: "nonce-7".into(),
        };
        let current = FinalAckKey {
            session_id: 3,
            attempt_id: 8,
            phase: "messages".into(),
            nonce: "nonce-8".into(),
        };
        let pending = Arc::new(Mutex::new(Some(current.clone())));
        let (tx, mut rx) = mpsc::unbounded_channel();

        enforce_final_ack_deadline(pending.clone(), stale, tx, Duration::from_millis(1)).await;

        assert!(rx.try_recv().is_err());
        assert_eq!(
            pending.lock().expect("pending lock").as_ref(),
            Some(&current)
        );
    }

    #[test]
    fn final_ack_requires_exact_identity_and_is_consumed_once() {
        let expected = FinalAckKey {
            session_id: 11,
            attempt_id: 4,
            phase: "messages".into(),
            nonce: "exact-nonce".into(),
        };
        let pending = Arc::new(Mutex::new(Some(expected.clone())));
        let mismatches = [
            PhaseAckFrame {
                frame_type: "PHASE_COMPLETED".into(),
                phase: SyncPhase::Messages,
                session_id: Some(11),
                attempt_id: Some(4),
                nonce: Some("exact-nonce".into()),
            },
            PhaseAckFrame {
                frame_type: "PHASE_ACK".into(),
                phase: SyncPhase::OwnerMetadata,
                session_id: Some(11),
                attempt_id: Some(4),
                nonce: Some("exact-nonce".into()),
            },
            PhaseAckFrame {
                frame_type: "PHASE_ACK".into(),
                phase: SyncPhase::Messages,
                session_id: Some(10),
                attempt_id: Some(4),
                nonce: Some("exact-nonce".into()),
            },
            PhaseAckFrame {
                frame_type: "PHASE_ACK".into(),
                phase: SyncPhase::Messages,
                session_id: Some(11),
                attempt_id: Some(3),
                nonce: Some("exact-nonce".into()),
            },
            PhaseAckFrame {
                frame_type: "PHASE_ACK".into(),
                phase: SyncPhase::Messages,
                session_id: Some(11),
                attempt_id: Some(4),
                nonce: Some("stale-nonce".into()),
            },
            PhaseAckFrame {
                frame_type: "PHASE_ACK".into(),
                phase: SyncPhase::Messages,
                session_id: Some(11),
                attempt_id: Some(4),
                nonce: None,
            },
        ];
        for payload in mismatches {
            assert!(!consume_final_ack(&pending, &payload));
            assert_eq!(
                pending.lock().expect("pending lock").as_ref(),
                Some(&expected)
            );
        }

        let exact = PhaseAckFrame {
            frame_type: "PHASE_ACK".into(),
            phase: SyncPhase::Messages,
            session_id: Some(11),
            attempt_id: Some(4),
            nonce: Some("exact-nonce".into()),
        };
        assert!(consume_final_ack(&pending, &exact));
        assert!(!consume_final_ack(&pending, &exact));
        assert!(pending.lock().expect("pending lock").is_none());
    }

    #[test]
    fn command_router_tracks_the_current_session_owner() {
        let router = SyncCommandRouter::default();
        assert!(router.send(SyncCommand::Cancel).is_err());

        let (first_tx, _first_rx) = mpsc::unbounded_channel();
        router.install(1, first_tx);
        let (second_tx, mut second_rx) = mpsc::unbounded_channel();
        router.install(2, second_tx);

        router.clear_if_owner(1);
        router
            .send(SyncCommand::Cancel)
            .expect("stale cleanup must preserve the current session sender");
        assert!(matches!(second_rx.try_recv(), Ok(SyncCommand::Cancel)));

        router.clear_if_owner(2);
        assert!(router.send(SyncCommand::Cancel).is_err());
    }
}

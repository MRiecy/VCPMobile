use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::db_write_queue::DbWriteQueue;
use crate::vcp_modules::sync_error::{
    build_local_error_payload, build_wire_error_payload, decode_wire_sync_error,
    encode_wire_sync_error, parse_wire_sync_error, SyncErrorPayload,
};
use crate::vcp_modules::sync_executor::PullExecutor;
use crate::vcp_modules::sync_hash::HashInitializer;
use crate::vcp_modules::sync_logger::{redact_sync_diagnostic, LogLevel, SyncLogger};
use crate::vcp_modules::sync_pipeline::{Phase1Metadata, Phase3Message, SyncPipeline};
use crate::vcp_modules::sync_types::SyncDataType;
use crate::vcp_modules::vcp_log_service::get_vcp_log_status_internal;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::future::Future;
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, Runtime, State};
use tokio::sync::{mpsc, Mutex as AsyncMutex, RwLock, Semaphore};
use tokio::task::{JoinHandle, JoinSet};
use tokio_tungstenite::{
    connect_async, tungstenite::protocol::Message, MaybeTlsStream, WebSocketStream,
};
use tokio_util::sync::CancellationToken;

const EXPECTED_PLUGIN_VERSION: &str = "1.2.0";
const WIRE_PROTOCOL_VERSION: &str = "1.2";
const VERSION_CHECK_TIMEOUT: Duration = Duration::from_secs(5);
const WS_OPERATION_TIMEOUT: Duration = Duration::from_secs(5);
const SYNC_HTTP_REQUEST_TIMEOUT: Duration = Duration::from_secs(270);
const PHASE_RESPONSE_TIMEOUT: Duration = Duration::from_secs(60);
const FINAL_ACK_TIMEOUT: Duration = Duration::from_secs(30);
const ENTITY_OPERATION_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_SYNC_RETRIES: u32 = 3;
const MAX_SYNC_TOPICS: usize = 10_000;
type RoutedSyncCommand = (u64, mpsc::UnboundedSender<SyncCommand>);
type SyncWebSocket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

#[derive(Debug, PartialEq, Eq)]
struct VersionAck {
    plugin_version: String,
    protocol_version: String,
}

#[derive(Debug, PartialEq, Eq)]
enum VersionHandshakeError {
    Protocol(String),
    Remote(String),
    Closed { code: Option<u16>, reason: String },
    Transport(String),
}

fn parse_version_ack(payload: &Value) -> Result<VersionAck, String> {
    if payload.get("type").and_then(Value::as_str) != Some("VERSION_ACK") {
        return Err("expected VERSION_ACK".to_string());
    }
    let plugin_version = payload
        .get("pluginVersion")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "VERSION_ACK.pluginVersion must be a non-empty string".to_string())?;
    let protocol_version = payload
        .get("protocolVersion")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "VERSION_ACK.protocolVersion must be a non-empty string".to_string())?;
    Ok(VersionAck {
        plugin_version: plugin_version.to_string(),
        protocol_version: protocol_version.to_string(),
    })
}

fn parse_version_handshake_payload(
    payload: &Value,
) -> Result<Option<VersionAck>, VersionHandshakeError> {
    match payload.get("type").and_then(Value::as_str) {
        Some("SYNC_ERROR") => {
            let wire = payload
                .get("error")
                .ok_or_else(|| {
                    VersionHandshakeError::Protocol("SYNC_ERROR.error is missing".to_string())
                })
                .and_then(|value| {
                    parse_wire_sync_error(value).map_err(VersionHandshakeError::Protocol)
                })?;
            let encoded = encode_wire_sync_error(&wire).map_err(VersionHandshakeError::Protocol)?;
            Err(VersionHandshakeError::Remote(encoded))
        }
        Some("SYNC_LOG_EVENT") => Ok(None),
        _ => parse_version_ack(payload)
            .map(Some)
            .map_err(VersionHandshakeError::Protocol),
    }
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
    session_id: u64,
    status: &Arc<RwLock<String>>,
    ws_stream: &mut SyncWebSocket,
    context: &str,
    error: &str,
) {
    let message = protocol_send_failure_message(context, error);
    log::warn!("[SyncService] {message}");
    emit_sync_log(app_handle, "warning", &message);
    let _ = (session_id, status);
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
    let Some(backoff) = take_retry_slot(retry_count, retry_delay) else {
        let final_message =
            format!("{message}; retry budget exhausted after {MAX_SYNC_RETRIES} attempts");
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
    !cancel_token.is_cancelled() && !cancelled_during(cancel_token, backoff).await
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FinalAckKey {
    session_id: u64,
    attempt_id: u64,
    phase: String,
    nonce: String,
}

impl FinalAckKey {
    fn new(session_id: u64, attempt_id: u64) -> Self {
        Self {
            session_id,
            attempt_id,
            phase: "messages".to_string(),
            nonce: uuid::Uuid::new_v4().to_string(),
        }
    }

    fn message(&self) -> Value {
        json!({
            "type": "PHASE_COMPLETED",
            "phase": self.phase,
            "sessionId": self.session_id,
            "attemptId": self.attempt_id,
            "nonce": self.nonce,
        })
    }

    fn matches_payload(&self, payload: &Value) -> bool {
        payload.get("type").and_then(Value::as_str) == Some("PHASE_ACK")
            && payload.get("phase").and_then(Value::as_str) == Some(self.phase.as_str())
            && payload.get("sessionId").and_then(Value::as_u64) == Some(self.session_id)
            && payload.get("attemptId").and_then(Value::as_u64) == Some(self.attempt_id)
            && payload.get("nonce").and_then(Value::as_str) == Some(self.nonce.as_str())
    }
}

type PendingFinalAck = Arc<Mutex<Option<FinalAckKey>>>;

fn consume_final_ack(pending: &PendingFinalAck, payload: &Value) -> bool {
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
    expected_types: Arc<Mutex<HashSet<String>>>,
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
            let mut missing = expected.iter().cloned().collect::<Vec<_>>();
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
    expected_results: Arc<AsyncMutex<Option<HashSet<String>>>>,
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
    pub completed: tokio::sync::Mutex<HashSet<String>>,
    pub modified: tokio::sync::Mutex<HashSet<String>>,
    pub failed: tokio::sync::Mutex<HashSet<String>>,
    pub legacy_attachment_warnings: std::sync::atomic::AtomicUsize,
    pub total: std::sync::atomic::AtomicUsize,
}

impl Phase3Tracker {
    /// 标记某个 topic 为数据已修改（实际发生了 pull/push）
    pub async fn mark_modified(&self, topic_id: &str) {
        let mut modified = self.modified.lock().await;
        modified.insert(topic_id.to_string());
    }

    pub async fn mark_failed(&self, topic_id: &str) {
        self.failed.lock().await.insert(topic_id.to_string());
    }

    pub fn add_legacy_attachment_warnings(&self, count: usize) {
        self.legacy_attachment_warnings
            .fetch_add(count, Ordering::SeqCst);
    }

    async fn completion_summary(&self) -> SyncCompletionSummary {
        let successful_topics = self.completed.lock().await.len();
        let failed = self.failed.lock().await;
        let mut failed_topic_ids = failed.iter().cloned().collect::<Vec<_>>();
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
    /// 当所有 topic 都完成时，触发 complete_phase 和 Phase3 命令。
    pub async fn mark_completed(
        &self,
        topic_id: &str,
        logger: &Arc<Mutex<SyncLogger>>,
        tx: &mpsc::UnboundedSender<SyncCommand>,
        app_handle: &AppHandle,
        quiet: bool,
    ) -> bool {
        let mut completed = self.completed.lock().await;
        let is_new = completed.insert(topic_id.to_string());
        if is_new {
            let done = completed.len();
            let total = self.total.load(Ordering::SeqCst);

            if !quiet {
                if let Ok(mut logger) = logger.lock() {
                    logger.log_operation("messages", "topic", topic_id, true, None);
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
                    "message": format!("Syncing Messages: {}/{}", done, total),
                    "successfulTopics": done,
                    "totalTopics": total,
                    "failedTopics": self.failed.lock().await.len(),
                    "legacyAttachmentWarnings": self.legacy_attachment_warnings.load(Ordering::SeqCst)
                }),
            );

            if done == total {
                if let Ok(mut logger) = logger.lock() {
                    logger.complete_phase("messages");
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

pub struct NetworkAwareSemaphore {
    semaphore: Arc<Semaphore>,
}

impl NetworkAwareSemaphore {
    pub fn new() -> Self {
        // [Evolution] 动态并发控制：根据核心数动态调整
        // 核心数 * 1.5 是 IO 密集型任务的平衡点，但在移动端需严格限制上限以保护 UI 响应
        let cores = std::thread::available_parallelism()
            .map(|p| p.get())
            .unwrap_or(4);

        let concurrency = ((cores as f32) * 1.5).clamp(6.0, 12.0) as usize;
        log::info!(
            "[Sync] Auto-optimized concurrency set to {} (cores: {})",
            concurrency,
            cores
        );

        Self {
            semaphore: Arc::new(Semaphore::new(concurrency)),
        }
    }

    pub async fn acquire(&self) -> tokio::sync::SemaphorePermit<'_> {
        self.semaphore.acquire().await.unwrap()
    }
}

pub enum SyncCommand {
    NotifyLocalChange {
        id: String,
        data_type: SyncDataType,
        hash: String,
        ts: i64,
    },
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
        data_type: SyncDataType,
        id: String,
        deleted_at: i64,
        owner_type: Option<String>,
        owner_id: Option<String>,
    },
    NotifyMessageDelete {
        topic_id: String,
        message_id: String,
        deleted_at: i64,
    },
    StartManualSync,
    SendWsMessage {
        attempt_id: u64,
        value: serde_json::Value,
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

pub fn parse_sync_data_type(value: &Value) -> Option<SyncDataType> {
    serde_json::from_value::<SyncDataType>(value.clone()).ok()
}

fn parse_unique_nonempty_strings(
    value: &Value,
    field: &str,
    max_items: usize,
) -> Result<Vec<String>, String> {
    let values = value
        .as_array()
        .ok_or_else(|| format!("{field} must be an array"))?;
    if values.len() > max_items {
        return Err(format!("{field} exceeds {max_items} item budget"));
    }
    let mut seen = HashSet::new();
    let mut result = Vec::with_capacity(values.len());
    for value in values {
        let item = value
            .as_str()
            .filter(|item| !item.is_empty())
            .ok_or_else(|| format!("{field} must contain only non-empty strings"))?;
        if !seen.insert(item) {
            return Err(format!("{field} contains duplicate value {item}"));
        }
        result.push(item.to_string());
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
    message: &str,
) {
    let sync_state = app_handle.state::<SyncState>();
    let _owner_commit = sync_state.owner_commit.lock().await;
    if sync_state.current_session_id.load(Ordering::SeqCst) != session_id {
        return;
    }
    publish_sync_status_inner(app_handle, session_id, status, next_status, message, None).await;
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
    let user_message = error.message.clone();
    publish_sync_status_inner(
        app_handle,
        session_id,
        status,
        "error",
        &user_message,
        Some(error),
    )
    .await;
}

async fn publish_sync_status_inner<R: Runtime>(
    app_handle: &AppHandle<R>,
    session_id: u64,
    status: &Arc<RwLock<String>>,
    next_status: &str,
    message: &str,
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
        "message": message,
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
            "agentsChanged": true,
            "groupsChanged": true,
            "topicsChanged": true,
            "messagesChanged": true,
        }),
    );
    let _ = app_handle.emit(
        "vcp-sync-status",
        json!({
            "status": terminal_status,
            "message": if terminal_status == "completed" { "同步完成" } else { "同步完成，但存在旧附件警告" },
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

fn check_loopback_on_mobile(ws_url: &str, is_android: bool) -> bool {
    if is_android {
        if let Ok(u) = url::Url::parse(ws_url) {
            if let Some(host) = u.host_str() {
                if host == "127.0.0.1" || host == "localhost" {
                    return true;
                }
            }
        }
    }
    false
}

fn classify_connection_failure(
    ws_url: &str,
    err: &tokio_tungstenite::tungstenite::error::Error,
) -> &'static str {
    let is_android = cfg!(target_os = "android");
    if check_loopback_on_mobile(ws_url, is_android) {
        return "CONFIG_LOOPBACK_ON_MOBILE";
    }

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
    cancel_token: CancellationToken,
    tx: mpsc::UnboundedSender<SyncCommand>,
    mut rx: mpsc::UnboundedReceiver<SyncCommand>,
    connection_status: Arc<RwLock<String>>,
) -> Result<(), String> {
    let handle_clone = app_handle.clone();
    let tx_internal = tx.clone();
    let connection_status_for_task = connection_status.clone();

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
    let configured_log_level = {
        let settings_state =
            app_handle.state::<crate::vcp_modules::settings_manager::SettingsState>();
        crate::vcp_modules::settings_manager::read_settings(app_handle.clone(), settings_state)
            .await
            .ok()
            .map(|settings| settings.sync_log_level)
            .unwrap_or_else(|| "INFO".to_string())
    };
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
    let _ = tauri_plugin_vcp_mobile::stream::start_stream_service_inner(
        &app_handle,
        "[数据同步] VCP Mobile",
    );

    let network_semaphore = Arc::new(NetworkAwareSemaphore::new());
    let semaphore_task = network_semaphore.clone();
    let write_queue_task = write_queue.clone();
    let sync_logger_task = sync_logger.clone();

    'session: loop {
        if cancel_token.is_cancelled() {
            break;
        }
        let (ws_url, http_url) = {
            let settings_state =
                handle_clone.state::<crate::vcp_modules::settings_manager::SettingsState>();
            match crate::vcp_modules::settings_manager::read_settings(
                handle_clone.clone(),
                settings_state,
            )
            .await
            {
                Ok(s) => {
                    if s.sync_server_url.is_empty() || s.sync_http_url.is_empty() {
                        emit_sync_log(&handle_clone, "error", "同步服务 URL 未配置，请检查设置");
                        publish_sync_error(
                            &handle_clone,
                            session_id,
                            &connection_status_for_task,
                            "SYNC_CONFIG_MISSING",
                            "同步服务 URL 未配置",
                            Vec::new(),
                        )
                        .await;
                        break;
                    }
                    let ws_addr = match url::Url::parse(&s.sync_server_url) {
                        Ok(mut u) => {
                            u.set_query(Some(&format!("token={}", s.sync_token)));
                            u.to_string()
                        }
                        Err(e) => {
                            emit_sync_log(
                                &handle_clone,
                                "error",
                                &format!("同步服务 URL 格式非法: {}", e),
                            );
                            publish_sync_error(
                                &handle_clone,
                                session_id,
                                &connection_status_for_task,
                                "SYNC_CONFIG_INVALID",
                                "同步服务 URL 格式非法",
                                Vec::new(),
                            )
                            .await;
                            break;
                        }
                    };
                    (ws_addr, s.sync_http_url.clone())
                }
                Err(error) => {
                    let message = format!("无法读取同步配置: {error}");
                    emit_sync_log(&handle_clone, "error", &message);
                    publish_sync_error(
                        &handle_clone,
                        session_id,
                        &connection_status_for_task,
                        "SYNC_SETTINGS_READ_FAILED",
                        &message,
                        Vec::new(),
                    )
                    .await;
                    break;
                }
            }
        };

        publish_sync_nonterminal_status(
            &handle_clone,
            session_id,
            &connection_status_for_task,
            "connecting",
            "同步服务连接中...",
        )
        .await;

        let phase_gate: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));

        let connect_result = tokio::select! {
            biased;
            _ = cancel_token.cancelled() => break,
            result = connect_async(&ws_url) => result,
        };

        match connect_result {
            Ok((mut ws_stream, _)) => {
                // ── 版本验证握手 ──
                {
                    let version_req = json!({
                        "type": "VERSION_CHECK",
                        "mobileVersion": env!("CARGO_PKG_VERSION"),
                        "protocolVersion": WIRE_PROTOCOL_VERSION,
                    });
                    if let Err(error) = send_ws_with_deadline(
                        &mut ws_stream,
                        Message::Text(version_req.to_string().into()),
                    )
                    .await
                    {
                        terminate_after_protocol_send_failure(
                            &handle_clone,
                            session_id,
                            &connection_status_for_task,
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
                                        let payload = serde_json::from_str::<Value>(&text)
                                            .map_err(|error| VersionHandshakeError::Protocol(
                                                format!("Malformed VERSION_ACK JSON: {error}")
                                            ))?;
                                        match parse_version_handshake_payload(&payload)? {
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
                            if version_ack.plugin_version == EXPECTED_PLUGIN_VERSION
                                && version_ack.protocol_version == WIRE_PROTOCOL_VERSION
                            {
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
                                        "桌面端插件 v{} / 协议 {} 与期望 v{} / 协议 {} 不兼容",
                                        version_ack.plugin_version,
                                        version_ack.protocol_version,
                                        EXPECTED_PLUGIN_VERSION,
                                        WIRE_PROTOCOL_VERSION,
                                    ),
                                    Vec::new(),
                                )
                                .await;
                                emit_sync_log(
                                    &handle_clone,
                                    "error",
                                    &format!(
                                        "❌ 同步协议不匹配: 桌面端插件 v{} / 协议 {}，期望 v{} / 协议 {}",
                                        version_ack.plugin_version,
                                        version_ack.protocol_version,
                                        EXPECTED_PLUGIN_VERSION,
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
                    logger.start_phase("owner_metadata", 0);
                    logger.log(LogLevel::Info, "sync", "=== Phase 1: Owner Metadata ===");
                }
                if let Err(error) = send_ws_with_deadline(
                    &mut ws_stream,
                    Message::Text(
                        json!({ "type": "PHASE_START", "phase": "owner_metadata" })
                            .to_string()
                            .into(),
                    ),
                )
                .await
                {
                    terminate_after_protocol_send_failure(
                        &handle_clone,
                        session_id,
                        &connection_status_for_task,
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
                    "同步服务已连接",
                )
                .await;

                let db = handle_clone.state::<DbState>();
                if let Err(e) = HashInitializer::ensure_all_agent_hashes(&db.pool).await {
                    if let Ok(mut logger) = sync_logger_task.lock() {
                        logger.log(
                            LogLevel::Error,
                            "owner_metadata",
                            &format!("Failed to initialize agent hashes: {}", e),
                        );
                    }
                    publish_sync_error(
                        &handle_clone,
                        session_id,
                        &connection_status_for_task,
                        "AGENT_HASH_INIT_DB_FAILED",
                        &format!("Failed to initialize agent hashes: {e}"),
                        Vec::new(),
                    )
                    .await;
                    let _ = close_ws_with_deadline(&mut ws_stream).await;
                    break 'session;
                }
                if let Err(e) = HashInitializer::ensure_all_group_hashes(&db.pool).await {
                    if let Ok(mut logger) = sync_logger_task.lock() {
                        logger.log(
                            LogLevel::Error,
                            "owner_metadata",
                            &format!("Hash init error: {}", e),
                        );
                    }
                    publish_sync_error(
                        &handle_clone,
                        session_id,
                        &connection_status_for_task,
                        "GROUP_HASH_INIT_DB_FAILED",
                        &format!("Failed to initialize group hashes: {e}"),
                        Vec::new(),
                    )
                    .await;
                    let _ = close_ws_with_deadline(&mut ws_stream).await;
                    break 'session;
                }

                // Every reconnect gets a fresh owner set. Cancelling and joining this tracker
                // before retry prevents late phase commands and writes from crossing attempts.
                next_attempt_id = next_attempt_id.wrapping_add(1);
                let attempt_id = next_attempt_id;
                let attempt_cancel = cancel_token.child_token();
                let task_tracker = Arc::new(SyncTaskTracker::new(attempt_cancel.clone()));
                let uploaded_hashes = Arc::new(RwLock::new(HashSet::new()));
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
                    Arc::new(tokio::sync::Mutex::new(HashSet::<String>::new()));
                let expected_topic_hash_results =
                    Arc::new(tokio::sync::Mutex::new(None::<HashSet<String>>));
                let phase3_batch_inflight = Arc::new(AtomicBool::new(false));
                let awaiting_final_ack: PendingFinalAck = Arc::new(Mutex::new(None));

                // Phase3 分批 diff 的待发送批次队列
                let pending_diff_batches: Arc<
                    tokio::sync::Mutex<
                        std::collections::VecDeque<serde_json::Map<String, serde_json::Value>>,
                    >,
                > = Arc::new(tokio::sync::Mutex::new(std::collections::VecDeque::new()));

                // Phase 2 筛选出的需要消息同步的 topic 列表
                let changed_topics: Arc<tokio::sync::Mutex<Vec<String>>> =
                    Arc::new(tokio::sync::Mutex::new(Vec::new()));

                // V2: Phase 1 筛选出的内容有变动的 owner (Agent/Group) 列表
                let changed_owners: Arc<tokio::sync::Mutex<HashSet<String>>> =
                    Arc::new(tokio::sync::Mutex::new(HashSet::new()));

                // 用于跟踪 manifest diff 结果是否全部收到，防止 total_ops=0 时 Phase 1 卡住
                let expected_manifest_count = Arc::new(AtomicU32::new(0));
                let manifest_responses_received = Arc::new(AtomicU32::new(0));
                let expected_manifest_types = Arc::new(Mutex::new(HashSet::<String>::new()));
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
                                    session_id,
                                    &connection_status_for_task,
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
                                    let db = handle_clone.state::<DbState>();
                                    let owners = {
                                        let guard = changed_owners.lock().await;
                                        guard.iter().cloned().collect::<Vec<String>>()
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
                                            if manifest.data_type != SyncDataType::Topic {
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
                                                    *expected = HashSet::from(["topic".to_string()]);
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
                                                logger.start_phase("topic_metadata", 1);
                                                logger.log(LogLevel::Info, "topic_metadata", "=== Phase 2: Pulling Topic Metadata ===");
                                            }
                                            if let Err(error) = send_ws_with_deadline(&mut ws_stream, Message::Text(json!({ "type": "PHASE_START", "phase": "topic_metadata" }).to_string().into())).await {
                                                terminate_after_protocol_send_failure(
                                                    &handle_clone,
                                                    session_id,
                                                    &connection_status_for_task,
                                                    &mut ws_stream,
                                                    "topic metadata phase start",
                                                    &error,
                                                ).await;
                                                break 'attempt;
                                            }

                                            let msg = json!({
                                                "type": "SYNC_MANIFEST",
                                                "data": manifest.items,
                                                "dataType": manifest.data_type,
                                                "phase": 2, // Use explicit Phase ID 2
                                                "targetedOwners": owners
                                            });
                                            if let Err(error) = send_ws_with_deadline(&mut ws_stream, Message::Text(msg.to_string().into())).await {
                                                terminate_after_protocol_send_failure(
                                                    &handle_clone,
                                                    session_id,
                                                    &connection_status_for_task,
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
                                    if let Ok(mut logger) = sync_logger_task.lock() {
                                        logger.log(LogLevel::Info, "topic_metadata", "=== Phase 2.5: Validating Topic Hashes ===");
                                    }

                                    let db = handle_clone.state::<DbState>();
                                    let owners = {
                                        let guard = changed_owners.lock().await;
                                        guard.iter().cloned().collect::<Vec<String>>()
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
                                            let mut hash_map = serde_json::Map::new();
                                            let mut topic_states = Vec::new();
                                            for (topic_id, state) in topic_hashes {
                                                hash_map.insert(topic_id.clone(), json!({
                                                    "configHash": state.config_hash.clone(),
                                                    "contentHash": state.content_hash.clone()
                                                }));
                                                topic_states.push(json!({
                                                    "topicId": topic_id,
                                                    "ownerType": state.owner_type,
                                                    "ownerId": state.owner_id,
                                                    "configHash": state.config_hash,
                                                    "contentHash": state.content_hash,
                                                }));
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
                                                *expected = Some(hash_map.keys().cloned().collect());
                                            }
                                            let msg = json!({
                                                "type": "SYNC_TOPIC_HASH_BATCH_V2",
                                                "hashes": hash_map,
                                                "topics": topic_states,
                                            });
                                            if let Err(error) = send_ws_with_deadline(&mut ws_stream, Message::Text(msg.to_string().into())).await {
                                                terminate_after_protocol_send_failure(
                                                    &handle_clone,
                                                    session_id,
                                                    &connection_status_for_task,
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
                                    if let Ok(mut logger) = sync_logger_task.lock() {
                                        logger.start_phase("messages", 0);
                                        logger.log(LogLevel::Info, "messages", "=== Phase 3: Messages ===");
                                    }
                                    if let Err(error) = send_ws_with_deadline(&mut ws_stream, Message::Text(json!({ "type": "PHASE_START", "phase": "messages" }).to_string().into())).await {
                                        terminate_after_protocol_send_failure(
                                            &handle_clone,
                                            session_id,
                                            &connection_status_for_task,
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
                                            logger.complete_phase("messages");
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
                                                        "message": format!("Syncing Messages: 0/{topic_count}"),
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
                                                let batches = match build_diff_batches(topic_states) {
                                                    Ok(batches) => batches,
                                                    Err(error) => {
                                                        let _ = tx_internal.send(SyncCommand::FailAttemptDetailed {
                                                            attempt_id,
                                                            code: "PHASE3_DIFF_BUDGET_EXCEEDED".to_string(),
                                                            message: error,
                                                            failed_topic_ids: changed_ids.iter().take(8).cloned().collect(),
                                                        });
                                                        continue 'attempt;
                                                    }
                                                };
                                                let batch_count = batches.len();
                                                log::info!("[SyncService] Phase3 diff split into {} batches (max {} msgs/batch)", batch_count, MAX_MESSAGES_PER_BATCH);

                                                let mut first_batch = None;
                                                {
                                                    let mut pending = pending_diff_batches.lock().await;
                                                    if !batches.is_empty() {
                                                        first_batch = Some(batches[0].clone());
                                                        *pending = batches.into_iter().skip(1).collect();
                                                    }
                                                }

                                                if let Some(batch) = first_batch {
                                                    {
                                                        let mut expected = expected_phase3_batch.lock().await;
                                                        *expected = batch.keys().cloned().collect();
                                                    }
                                                    let msg = json!({
                                                        "type": "SYNC_MESSAGE_DIFF_BATCH",
                                                        "topics": batch,
                                                    });
                                                    if let Err(error) = send_ws_with_deadline(
                                                        &mut ws_stream,
                                                        Message::Text(msg.to_string().into()),
                                                    ).await {
                                                        terminate_after_protocol_send_failure(
                                                            &handle_clone,
                                                            session_id,
                                                            &connection_status_for_task,
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
                                crate::vcp_modules::sync_pipeline::pipeline::PipelineCommand::Finalize => {
                                    // 本地 pipeline 已落盘，但不能越过桌面端最终 ACK 发布完成态。
                                    emit_sync_log(&handle_clone, "info", "Local finalization complete; waiting for desktop acknowledgement");
                                },
                            }
                        },
                        Some(cmd) = rx.recv() => {
                            match cmd {
                                SyncCommand::Cancel => {
                                    let _ = close_ws_with_deadline(&mut ws_stream).await;
                                    break;
                                },
                                SyncCommand::NotifyLocalChange { id, data_type, hash, ts } => {
                                    let msg = json!({ "type": "SYNC_ENTITY_UPDATE", "id": id, "dataType": data_type, "hash": hash, "ts": ts });
                                    if let Err(error) = send_ws_with_deadline(&mut ws_stream, Message::Text(msg.to_string().into())).await {
                                        terminate_after_protocol_send_failure(
                                            &handle_clone,
                                            session_id,
                                            &connection_status_for_task,
                                            &mut ws_stream,
                                            "local entity update",
                                            &error,
                                        ).await;
                                        break 'attempt;
                                    }
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
                                                "Agent/group metadata write drain before avatars failed: {error}"
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
                                                *expected = HashSet::from(["avatar".to_string()]);
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
                                        let msg = json!({
                                            "type": "SYNC_MANIFEST",
                                            "data": manifest.items,
                                            "dataType": manifest.data_type,
                                            "phase": 1
                                        });
                                        if let Err(error) = send_ws_with_deadline(
                                            &mut ws_stream,
                                            Message::Text(msg.to_string().into()),
                                        ).await {
                                            terminate_after_protocol_send_failure(
                                                &handle_clone,
                                                session_id,
                                                &connection_status_for_task,
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
                                        if let Err(error) = send_ws_with_deadline(&mut ws_stream, Message::Text(json!({ "type": "PHASE_COMPLETED", "phase": "owner_metadata" }).to_string().into())).await {
                                            terminate_after_protocol_send_failure(
                                                &handle_clone,
                                                session_id,
                                                &connection_status_for_task,
                                                &mut ws_stream,
                                                "owner metadata phase completion",
                                                &error,
                                            ).await;
                                            break 'attempt;
                                        }
                                        let _ = pipeline_task.on_owner_metadata_done().await;
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
                                        if let Err(error) = send_ws_with_deadline(&mut ws_stream, Message::Text(json!({ "type": "PHASE_COMPLETED", "phase": "topic_metadata" }).to_string().into())).await {
                                            terminate_after_protocol_send_failure(
                                                &handle_clone,
                                                session_id,
                                                &connection_status_for_task,
                                                &mut ws_stream,
                                                "topic metadata phase completion",
                                                &error,
                                            ).await;
                                            break 'attempt;
                                        }
                                        let _ = pipeline_task.on_topic_metadata_pull_done().await;
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
                                        let _ = pipeline_task.on_topic_validation_done().await;
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
                                            &pipeline_task,
                                            &sync_logger_task,
                                            modified_topics,
                                        ).await {
                                            let message = format!("Sync finalization failed: {}", e);
                                            fatal_error = true;
                                            log::error!("[SyncService] {}", message);
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
                                        match send_ws_with_deadline(
                                            &mut ws_stream,
                                            Message::Text(final_ack.message().to_string().into()),
                                        )
                                        .await
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
                                SyncCommand::NotifyDelete {
                                    data_type,
                                    id,
                                    deleted_at,
                                    owner_type,
                                    owner_id,
                                } => {
                                    let mut msg = json!({
                                        "type": "SYNC_ENTITY_DELETE",
                                        "id": id,
                                        "dataType": data_type,
                                        "deletedAt": deleted_at,
                                    });
                                    if let Some(owner_type) = owner_type {
                                        msg["ownerType"] = Value::String(owner_type);
                                    }
                                    if let Some(owner_id) = owner_id {
                                        msg["ownerId"] = Value::String(owner_id);
                                    }
                                    if let Err(error) = send_ws_with_deadline(&mut ws_stream, Message::Text(msg.to_string().into())).await {
                                        terminate_after_protocol_send_failure(
                                            &handle_clone,
                                            session_id,
                                            &connection_status_for_task,
                                            &mut ws_stream,
                                            "local entity deletion",
                                            &error,
                                        ).await;
                                        break 'attempt;
                                    }
                                },
                                SyncCommand::NotifyMessageDelete { topic_id, message_id, deleted_at } => {
                                    let msg = json!({
                                        "type": "SYNC_ENTITY_DELETE",
                                        "id": message_id,
                                        "topicId": topic_id,
                                        "dataType": SyncDataType::Message,
                                        "deletedAt": deleted_at,
                                    });
                                    if let Err(error) = send_ws_with_deadline(
                                        &mut ws_stream,
                                        Message::Text(msg.to_string().into()),
                                    ).await {
                                        terminate_after_protocol_send_failure(
                                            &handle_clone,
                                            session_id,
                                            &connection_status_for_task,
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
                                    let manifests = async {
                                        Ok::<_, String>(vec![
                                            Phase1Metadata::build_agent_manifest(&db.pool).await?,
                                            Phase1Metadata::build_group_manifest(&db.pool).await?,
                                        ])
                                    }.await;
                                    match manifests {
                                        Ok(manifests) => {
                                        let manifest_types = manifests
                                            .iter()
                                            .map(|manifest| manifest.data_type.to_string())
                                            .collect::<HashSet<_>>();
                                        if manifest_types.len() != manifests.len() {
                                            let _ = tx_internal.send(SyncCommand::FailAttemptDetailed {
                                                attempt_id,
                                                code: "OWNER_MANIFEST_INVALID".to_string(),
                                                message: "Phase 1 manifests contain duplicate data types".to_string(),
                                                failed_topic_ids: Vec::new(),
                                            });
                                            continue 'attempt;
                                        }
                                        let count = match u32::try_from(manifests.len()) {
                                            Ok(count) => count,
                                            Err(_) => {
                                                let _ = tx_internal.send(SyncCommand::FailAttemptDetailed {
                                                    attempt_id,
                                                    code: "OWNER_MANIFEST_INVALID".to_string(),
                                                    message: "Phase 1 manifest count exceeds the supported range".to_string(),
                                                    failed_topic_ids: Vec::new(),
                                                });
                                                continue 'attempt;
                                            }
                                        };
                                        expected_manifest_count.store(count, Ordering::SeqCst);
                                        manifest_responses_received.store(0, Ordering::SeqCst);
                                        match expected_manifest_types.lock() {
                                            Ok(mut expected) => *expected = manifest_types,
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

                                        if let Ok(mut logger) = sync_logger_task.lock() {
                                            logger.set_phase_expected("owner_metadata", count);
                                        }
                                        if manifests.is_empty() {
                                            let _ = tx_internal.send(SyncCommand::StartTopicMetadata { attempt_id });
                                            continue 'attempt;
                                        }
                                        for manifest in manifests {
                                            let msg = json!({
                                                "type": "SYNC_MANIFEST",
                                                "data": manifest.items,
                                                "dataType": manifest.data_type,
                                                "phase": 1 // Explicit Phase ID
                                            });
                                            if let Err(error) = send_ws_with_deadline(&mut ws_stream, Message::Text(msg.to_string().into())).await {
                                                terminate_after_protocol_send_failure(
                                                    &handle_clone,
                                                    session_id,
                                                    &connection_status_for_task,
                                                    &mut ws_stream,
                                                    "owner metadata manifest",
                                                    &error,
                                                ).await;
                                                break 'attempt;
                                            }
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
                                                message: format!("Failed to build Phase 1 manifests: {error}"),
                                                failed_topic_ids: Vec::new(),
                                            });
                                        }
                                    }
                                },
                                SyncCommand::SendWsMessage { attempt_id: command_attempt, value } => {
                                    if command_attempt != attempt_id { continue; }
                                    if let Err(error) = send_ws_with_deadline(&mut ws_stream, Message::Text(value.to_string().into())).await {
                                        terminate_after_protocol_send_failure(
                                            &handle_clone,
                                            session_id,
                                            &connection_status_for_task,
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
                                                "message": "Message synchronization failed",
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
                                let payload: Value = match serde_json::from_str::<Value>(&text) {
                                    Ok(payload) if payload.is_object() => payload,
                                    Ok(_) => {
                                        let message = "Sync protocol frame must be a JSON object";
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
                                if payload
                                    .get("type")
                                    .and_then(Value::as_str)
                                    .filter(|value| !value.is_empty())
                                    .is_none()
                                {
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

                                let h = handle_clone.clone();
                                let c = http_client.clone();
                                let base = http_url.clone();
                                let sem = semaphore_task.clone();
                                let wq = write_queue_task.clone();
                                let settings = match crate::vcp_modules::settings_manager::read_settings(h.clone(), h.state()).await {
                                    Ok(settings) => settings,
                                    Err(error) => {
                                        let _ = tx_internal.send(SyncCommand::FailAttempt {
                                            attempt_id,
                                            code: "SYNC_SETTINGS_READ_FAILED",
                                            message: format!("Failed to read sync settings: {error}"),
                                        });
                                        continue;
                                    }
                                };

                                match payload["type"].as_str() {
                                    Some("SYNC_ENTITY_UPDATE") => {
                                        let id = match payload.get("id").and_then(Value::as_str).filter(|id| !id.is_empty()) {
                                            Some(id) => id.to_string(),
                                            None => {
                                                fatal_error = true;
                                                publish_sync_error(
                                                    &handle_clone,
                                                    session_id,
                                                    &connection_status_for_task,
                                                    "PROTOCOL_FRAME_INVALID",
                                                    "SYNC_ENTITY_UPDATE.id must be a non-empty string",
                                                    Vec::new(),
                                                ).await;
                                                let _ = close_ws_with_deadline(&mut ws_stream).await;
                                                break;
                                            }
                                        };
                                        let data_type = match parse_sync_data_type(&payload["dataType"]) {
                                            Some(data_type @ (SyncDataType::Agent | SyncDataType::Group | SyncDataType::Topic)) => data_type,
                                            _ => {
                                                fatal_error = true;
                                                publish_sync_error(
                                                    &handle_clone,
                                                    session_id,
                                                    &connection_status_for_task,
                                                    "PROTOCOL_FRAME_INVALID",
                                                    "SYNC_ENTITY_UPDATE.dataType must be agent, group, or topic",
                                                    vec![id.clone()],
                                                ).await;
                                                let _ = close_ws_with_deadline(&mut ws_stream).await;
                                                break;
                                            }
                                        };
                                        let owner_type = if data_type == SyncDataType::Topic {
                                            match payload.get("ownerType").and_then(Value::as_str).filter(|owner_type| matches!(*owner_type, "agent" | "group")) {
                                                Some(owner_type) => owner_type,
                                                None => {
                                                    fatal_error = true;
                                                    publish_sync_error(
                                                        &handle_clone,
                                                        session_id,
                                                        &connection_status_for_task,
                                                        "PROTOCOL_FRAME_INVALID",
                                                        "SYNC_ENTITY_UPDATE topic requires ownerType agent or group",
                                                        vec![id.clone()],
                                                    ).await;
                                                    let _ = close_ws_with_deadline(&mut ws_stream).await;
                                                    break;
                                                }
                                            }
                                        } else {
                                            ""
                                        };
                                        let operation = tokio::time::timeout(ENTITY_OPERATION_TIMEOUT, async {
                                            let _permit = sem.acquire().await;
                                            let pull_result = match data_type {
                                                SyncDataType::Agent => PullExecutor::pull_agent(&h, &c, &base, &settings.sync_token, &id, &wq).await,
                                                SyncDataType::Group => PullExecutor::pull_group(&h, &c, &base, &settings.sync_token, &id, &wq).await,
                                                SyncDataType::Topic if owner_type == "group" => PullExecutor::pull_group_topic(&h, &c, &base, &settings.sync_token, &id, &wq).await,
                                                SyncDataType::Topic => PullExecutor::pull_agent_topic(&h, &c, &base, &settings.sync_token, &id, &wq).await,
                                                other => Err(format!("unsupported entity update type: {other:?}")),
                                            };
                                            pull_result?;
                                            wq.flush().await.map_err(|error| {
                                                format!("entity update write drain failed: {error}")
                                            })
                                        });
                                        tokio::pin!(operation);
                                        let result = loop {
                                            tokio::select! {
                                                biased;
                                                _ = cancel_token.cancelled() => {
                                                    let _ = close_ws_with_deadline(&mut ws_stream).await;
                                                    break 'attempt;
                                                }
                                                _ = heartbeat_interval.tick() => {
                                                    if let Err(error) = send_ws_with_deadline(
                                                        &mut ws_stream,
                                                        Message::Ping(Vec::new().into()),
                                                    ).await {
                                                        emit_sync_log(
                                                            &handle_clone,
                                                            "warning",
                                                            &format!("Entity update heartbeat failed: {error}"),
                                                        );
                                                        let _ = close_ws_with_deadline(&mut ws_stream).await;
                                                        break 'attempt;
                                                    }
                                                }
                                                result = &mut operation => break result,
                                            }
                                        }
                                            .map_err(|_| format!("operation timed out after {} seconds", ENTITY_OPERATION_TIMEOUT.as_secs()))
                                            .and_then(|result| result);
                                        if let Err(error) = result {
                                            fatal_error = true;
                                            let message = format!("SYNC_ENTITY_UPDATE failed for {id}: {error}");
                                            publish_sync_error(
                                                &handle_clone,
                                                session_id,
                                                &connection_status_for_task,
                                                "ENTITY_UPDATE_FAILED",
                                                &message,
                                                vec![id.clone()],
                                            ).await;
                                            let _ = close_ws_with_deadline(&mut ws_stream).await;
                                            break;
                                        }
                                    },
                                    Some("SYNC_DELETE_NOTIFY") => {
                                        use crate::vcp_modules::sync_executor::delete_executor::DeleteExecutor;
                                        let id = match payload.get("id").and_then(Value::as_str).filter(|id| !id.is_empty()) {
                                            Some(id) => id.to_string(),
                                            None => {
                                                fatal_error = true;
                                                publish_sync_error(
                                                    &handle_clone,
                                                    session_id,
                                                    &connection_status_for_task,
                                                    "PROTOCOL_FRAME_INVALID",
                                                    "SYNC_DELETE_NOTIFY.id must be a non-empty string",
                                                    Vec::new(),
                                                ).await;
                                                let _ = close_ws_with_deadline(&mut ws_stream).await;
                                                break;
                                            }
                                        };
                                        let data_type = match parse_sync_data_type(&payload["dataType"]) {
                                            Some(data_type @ (SyncDataType::Agent | SyncDataType::Group | SyncDataType::Topic | SyncDataType::Avatar | SyncDataType::Message)) => data_type,
                                            _ => {
                                                fatal_error = true;
                                                publish_sync_error(
                                                    &handle_clone,
                                                    session_id,
                                                    &connection_status_for_task,
                                                    "PROTOCOL_FRAME_INVALID",
                                                    "SYNC_DELETE_NOTIFY.dataType is missing or invalid",
                                                    vec![id.clone()],
                                                ).await;
                                                let _ = close_ws_with_deadline(&mut ws_stream).await;
                                                break;
                                            }
                                        };
                                        let deleted_at = match payload
                                            .get("deletedAt")
                                            .and_then(Value::as_i64)
                                            .filter(|deleted_at| *deleted_at >= 0)
                                        {
                                            Some(deleted_at) => deleted_at,
                                            None => {
                                                fatal_error = true;
                                                publish_sync_error(
                                                    &handle_clone,
                                                    session_id,
                                                    &connection_status_for_task,
                                                    "PROTOCOL_FRAME_INVALID",
                                                    "SYNC_DELETE_NOTIFY requires a non-negative integer deletedAt",
                                                    vec![id.clone()],
                                                ).await;
                                                let _ = close_ws_with_deadline(&mut ws_stream).await;
                                                break;
                                            }
                                        };
                                        let message_topic_id = if data_type == SyncDataType::Message {
                                            let topic_id = match payload
                                                .get("topicId")
                                                .and_then(Value::as_str)
                                                .filter(|topic_id| !topic_id.is_empty())
                                            {
                                                Some(topic_id) => topic_id.to_string(),
                                                None => {
                                                    fatal_error = true;
                                                    publish_sync_error(
                                                        &handle_clone,
                                                        session_id,
                                                        &connection_status_for_task,
                                                        "PROTOCOL_FRAME_INVALID",
                                                        "Message delete requires a non-empty topicId",
                                                        vec![id.clone()],
                                                    ).await;
                                                    let _ = close_ws_with_deadline(&mut ws_stream).await;
                                                    break;
                                                }
                                            };
                                            Some(topic_id)
                                        } else {
                                            None
                                        };
                                        let operation = tokio::time::timeout(ENTITY_OPERATION_TIMEOUT, async {
                                            match data_type {
                                                SyncDataType::Agent => DeleteExecutor::soft_delete_agent(&h, &id, deleted_at).await,
                                                SyncDataType::Group => DeleteExecutor::soft_delete_group(&h, &id, deleted_at).await,
                                                SyncDataType::Topic => DeleteExecutor::soft_delete_topic(&h, &id, deleted_at).await,
                                                SyncDataType::Avatar => match id.split_once(':') {
                                                    Some((owner_type, owner_id))
                                                        if crate::vcp_modules::sync_types::is_valid_avatar_owner(owner_type, owner_id) =>
                                                    {
                                                        DeleteExecutor::soft_delete_avatar(&h, owner_type, owner_id, deleted_at).await
                                                    }
                                                    _ => Err(format!("invalid avatar id: {id}")),
                                                },
                                                SyncDataType::Message => {
                                                    let topic_id = message_topic_id
                                                        .as_ref()
                                                        .ok_or_else(|| "message delete metadata is missing".to_string())?;
                                                    DeleteExecutor::soft_delete_message(
                                                        &h,
                                                        topic_id,
                                                        &id,
                                                        deleted_at,
                                                    ).await
                                                },
                                            }
                                        });
                                        tokio::pin!(operation);
                                        let result = loop {
                                            tokio::select! {
                                                biased;
                                                _ = cancel_token.cancelled() => {
                                                    let _ = close_ws_with_deadline(&mut ws_stream).await;
                                                    break 'attempt;
                                                }
                                                _ = heartbeat_interval.tick() => {
                                                    if let Err(error) = send_ws_with_deadline(
                                                        &mut ws_stream,
                                                        Message::Ping(Vec::new().into()),
                                                    ).await {
                                                        emit_sync_log(
                                                            &handle_clone,
                                                            "warning",
                                                            &format!("Entity delete heartbeat failed: {error}"),
                                                        );
                                                        let _ = close_ws_with_deadline(&mut ws_stream).await;
                                                        break 'attempt;
                                                    }
                                                }
                                                result = &mut operation => break result,
                                            }
                                        }
                                            .map_err(|_| format!("operation timed out after {} seconds", ENTITY_OPERATION_TIMEOUT.as_secs()))
                                            .and_then(|result| result);
                                        if let Err(error) = result {
                                            fatal_error = true;
                                            let message = format!("SYNC_DELETE_NOTIFY failed for {id}: {error}");
                                            publish_sync_error(
                                                &handle_clone,
                                                session_id,
                                                &connection_status_for_task,
                                                "ENTITY_DELETE_FAILED",
                                                &message,
                                                vec![id.clone()],
                                            ).await;
                                            let _ = close_ws_with_deadline(&mut ws_stream).await;
                                            break;
                                        }
                                    },
                                    Some("SYNC_ERROR") => {
                                        let encoded = payload
                                            .get("error")
                                            .ok_or_else(|| "SYNC_ERROR.error is missing".to_string())
                                            .and_then(parse_wire_sync_error)
                                            .and_then(|wire| encode_wire_sync_error(&wire));
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
                                    Some("SYNC_DIFF_RESULTS") => {
                                        let Some(data_type) = parse_sync_data_type(&payload["dataType"]) else {
                                            let _ = tx_internal.send(SyncCommand::FailAttempt {
                                                attempt_id,
                                                code: "PROTOCOL_FRAME_INVALID",
                                                message: "SYNC_DIFF_RESULTS.dataType is missing or invalid".to_string(),
                                            });
                                            continue;
                                        };
                                        if let Err(e) = crate::vcp_modules::sync_executor::diff_handler::DiffHandler::handle_diff(
                                            &h,
                                            &payload,
                                            data_type,
                                            &c,
                                            &base,
                                            &settings.sync_token,
                                            &wq,
                                            &pending_tasks_task,
                                            &total_tasks_task,
                                            &manifest_responses_received,
                                            &expected_manifest_count,
                                            &expected_manifest_types,
                                            &manifest_phase,
                                            &tx_internal,
                                            &changed_owners,
                                            &sync_logger_task,
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
                                    Some("SYNC_DIFF_RESULTS_BATCH") => {
                                        let payload = match crate::vcp_modules::sync_executor::batch_diff_handler::parse_phase3_batch_frame(&text) {
                                            Ok(payload) => payload,
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
                                        let upload_tracker = uploaded_hashes.clone();
                                        let expected_topics = expected_phase3_batch.clone();
                                        let token = settings.sync_token.clone();
                                        let prerender_enabled = settings.sync_prerender_enabled;
                                        task_tracker.spawn(async move {
                                            let result = crate::vcp_modules::sync_executor::batch_diff_handler::BatchDiffHandler::handle_diff_batch(
                                                &h,
                                                &payload,
                                                &c,
                                                &base,
                                                &token,
                                                &tracker,
                                                &handler_tx,
                                                &logger,
                                                &write_queue,
                                                &pending_batches,
                                                prerender_enabled,
                                                &upload_tracker,
                                                &expected_topics,
                                                attempt_id,
                                            ).await;
                                            let _ = batch_tx.send(SyncCommand::Phase3BatchFinished {
                                                attempt_id,
                                                result,
                                            });
                                        }).await;
                                    },
                                    Some("SYNC_TOPIC_HASH_RESULTS") => {
                                        manifest_phase.store(4, Ordering::SeqCst); // 进入 Phase 2.5+，旧 manifest 看门狗失效
                                        let expected = expected_topic_hash_results.lock().await.take();
                                        let parsed = match expected {
                                            Some(expected) => parse_unique_nonempty_strings(
                                                &payload["changedTopics"],
                                                "SYNC_TOPIC_HASH_RESULTS.changedTopics",
                                                MAX_SYNC_TOPICS,
                                            )
                                            .and_then(|changed_ids| {
                                                if let Some(unexpected) = changed_ids
                                                    .iter()
                                                    .find(|topic_id| !expected.contains(*topic_id))
                                                {
                                                    return Err(format!(
                                                        "SYNC_TOPIC_HASH_RESULTS.changedTopics contains unexpected topic {unexpected}"
                                                    ));
                                                }
                                                Ok(changed_ids)
                                            }),
                                            None => Err(
                                                "Received an unexpected or duplicate SYNC_TOPIC_HASH_RESULTS frame"
                                                    .to_string(),
                                            ),
                                        };
                                        match parsed {
                                            Ok(changed_ids) => {
                                                log::info!("[SyncService] Phase 2.5 results: {} topics need message sync", changed_ids.len());
                                                {
                                                    let mut guard = changed_topics.lock().await;
                                                    *guard = changed_ids;
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
                                    Some("PHASE_MANIFESTS") => {
                                        // Topic 元数据已在 Phase 1 的 SYNC_DIFF_RESULTS 中处理完毕。
                                        // 桌面端在 PHASE_START metadata/topic 时仍可能返回 PHASE_MANIFESTS，此处安全忽略。
                                    },
                                    Some("PHASE_ACK") => {
                                        if !consume_final_ack(&awaiting_final_ack, &payload) {
                                            log::debug!("[SyncService] Ignoring mismatched, stale, or replayed final acknowledgement");
                                            continue;
                                        }
                                        manifest_phase.store(0, Ordering::SeqCst); // 同步完成，所有看门狗失效
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
                                                logger.complete_phase("sync");
                                                (*logger).end_session();
                                            }
                                            emit_sync_log(&handle_clone, "success", "同步已完成，所有数据已对齐");

                                        }
                                        let _ = close_ws_with_deadline(&mut ws_stream).await;
                                        break;
                                    },
                                    Some("SYNC_LOG_EVENT") => {
                                        let level = payload["level"].as_str().unwrap_or("info");
                                        let message = payload["message"].as_str().unwrap_or("");
                                        emit_sync_log(&handle_clone, level, &format!("[Desktop] {}", message));
                                    },
                                    Some("DESKTOP_PHASE_START") | Some("DESKTOP_PHASE_PROGRESS") | Some("DESKTOP_PHASE_COMPLETE") => {
                                        let phase = payload["phase"].as_str().unwrap_or("unknown");
                                        let msg = match payload["type"].as_str() {
                                            Some("DESKTOP_PHASE_START") => format!("[Desktop] Phase {} started", phase),
                                            Some("DESKTOP_PHASE_COMPLETE") => format!("[Desktop] Phase {} completed", phase),
                                            _ => format!("[Desktop] Phase {} in progress", phase),
                                        };
                                        emit_sync_log(&handle_clone, "info", &msg);
                                    },
                                        _ => {}
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
                                    if let Ok(mut logger) = sync_logger_task.lock() {
                                        logger.log(LogLevel::Error, "network", &err_msg);
                                    }
                                    emit_sync_log(&handle_clone, "error", &err_msg);
                                    break;
                                }
                                None => {
                                    let err_msg = "WebSocket 连接意外断开 (服务器关闭连接)";
                                    if let Ok(mut logger) = sync_logger_task.lock() {
                                        logger.log(LogLevel::Error, "network", err_msg);
                                    }
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
            Err(e) => {
                let error_code = classify_connection_failure(&ws_url, &e);
                let error_detail = e.to_string();
                let is_fatal = error_code == "CONFIG_LOOPBACK_ON_MOBILE"
                    || error_code == "TOKEN_MISMATCH"
                    || error_code == "WS_PATH_INVALID";

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
            log::error!("[SyncService] {message}");
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
        {
            let mut logger_guard = sync_state.current_logger.write().unwrap();
            *logger_guard = None;
        }
        *sync_state.current_log_path.write().await = None;

        #[cfg(target_os = "android")]
        let _ = tauri_plugin_vcp_mobile::stream::stop_stream_service_inner(
            &app_handle,
            "[数据同步] VCP Mobile",
        );
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

fn build_diff_batches(
    topic_states: std::collections::HashMap<
        String,
        crate::vcp_modules::sync_pipeline::phase3_message::TopicLocalState,
    >,
) -> Result<std::collections::VecDeque<serde_json::Map<String, serde_json::Value>>, String> {
    let mut batches = std::collections::VecDeque::new();
    let mut current_batch = serde_json::Map::new();
    let mut current_msg_count = 0usize;
    let envelope_bytes = br#"{"type":"SYNC_MESSAGE_DIFF_BATCH","topics":{}}"#.len();
    let mut current_bytes = envelope_bytes;
    let mut topic_states = topic_states.into_iter().collect::<Vec<_>>();
    topic_states.sort_by(|left, right| left.0.cmp(&right.0));

    for (topic_id, state) in topic_states {
        let msg_count = state.messages.len();
        if msg_count > MAX_MESSAGES_PER_BATCH {
            return Err(format!(
                "Phase 3 diff topic {topic_id} exceeds the {MAX_MESSAGES_PER_BATCH}-message batch limit"
            ));
        }
        let mut msg_map = serde_json::Map::new();
        let mut messages = state.messages.into_iter().collect::<Vec<_>>();
        messages.sort_by(|left, right| left.0.cmp(&right.0));
        for (msg_id, hash) in messages {
            msg_map.insert(msg_id, serde_json::Value::String(hash));
        }
        let topic_obj = serde_json::json!({
            "ownerType": state.owner_type,
            "ownerId": state.owner_id,
            "topicHash": state.topic_hash,
            "messages": msg_map,
        });
        let mut counter = JsonSizeCounter::new(MAX_WS_DIFF_BATCH_BYTES);
        serde_json::to_writer(&mut counter, &topic_id)
            .and_then(|_| counter.write_all(b":").map_err(serde_json::Error::io))
            .and_then(|_| serde_json::to_writer(&mut counter, &topic_obj))
            .map_err(|error| format!("Failed to size Phase 3 topic {topic_id}: {error}"))?;
        let entry_bytes = counter.bytes;
        if envelope_bytes.saturating_add(entry_bytes) > MAX_WS_DIFF_BATCH_BYTES {
            return Err(format!(
                "Phase 3 diff topic {topic_id} exceeds the 8 MiB WebSocket frame limit"
            ));
        }

        let separator_bytes = usize::from(!current_batch.is_empty());
        if !current_batch.is_empty()
            && (current_msg_count.saturating_add(msg_count) > MAX_MESSAGES_PER_BATCH
                || current_bytes
                    .saturating_add(separator_bytes)
                    .saturating_add(entry_bytes)
                    > MAX_WS_DIFF_BATCH_BYTES)
        {
            batches.push_back(current_batch);
            current_batch = serde_json::Map::new();
            current_msg_count = 0;
            current_bytes = envelope_bytes;
        }

        current_bytes = current_bytes
            .saturating_add(usize::from(!current_batch.is_empty()))
            .saturating_add(entry_bytes);
        current_batch.insert(topic_id, topic_obj);
        current_msg_count = current_msg_count.saturating_add(msg_count);
    }

    if !current_batch.is_empty() {
        batches.push_back(current_batch);
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

    #[cfg(target_os = "android")]
    let _ = tauri_plugin_vcp_mobile::stream::stop_stream_service_inner(
        &handle,
        "[数据同步] VCP Mobile",
    );

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

    // VCPLog 是全局重要通道，未连接时直接拦截同步，避免进入同步主循环后长时间挂起
    let log_status = get_vcp_log_status_internal().await;
    if log_status != "connected" {
        return Err(encode_sync_command_error(
            "VCP_LOG_DISCONNECTED",
            &format!("VCPLog status is {log_status}"),
        ));
    }

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
    use std::sync::atomic::AtomicBool;
    use tokio_tungstenite::tungstenite::error::Error as WsError;
    use tokio_tungstenite::tungstenite::http::{Response, StatusCode};

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
    fn sync_error_classification_covers_connection_protocol_and_data_failures() {
        assert_eq!(
            build_sync_error_payload("CONNECTION_REFUSED", Vec::new(), None).category,
            SyncErrorCategory::Connection
        );
        assert_eq!(
            build_sync_error_payload("PROTOCOL_FRAME_INVALID", Vec::new(), None).category,
            SyncErrorCategory::Protocol
        );
        assert_eq!(
            build_sync_error_payload("SYNC_DB_DRAIN_FAILED", Vec::new(), None).category,
            SyncErrorCategory::Storage
        );
        assert_eq!(
            build_sync_error_payload("SYNC_VERSION_INCOMPATIBLE", Vec::new(), None).category,
            SyncErrorCategory::Compatibility
        );
    }

    #[test]
    fn command_errors_use_the_structured_transport_prefix() {
        let encoded = encode_sync_command_error(
            "VCP_LOG_DISCONNECTED",
            "Bearer raw-secret should stay in native logs only",
        );
        let json = encoded
            .strip_prefix("SYNC_ERROR:")
            .expect("structured sync command prefix");
        let payload: Value = serde_json::from_str(json).expect("structured sync error JSON");

        assert_eq!(payload["code"], "VCP_LOG_DISCONNECTED");
        assert_eq!(payload["category"], "connection");
        assert!(!encoded.contains("raw-secret"));
    }

    #[test]
    fn log_cleanup_never_counts_failed_removals_as_removed() {
        let mut removed = 0;
        let mut failed = 0;
        assert!(count_log_removal(Ok(()), &mut removed, &mut failed).is_none());
        assert!(count_log_removal(
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "denied"
            )),
            &mut removed,
            &mut failed,
        )
        .is_some());
        assert_eq!(removed, 1);
        assert_eq!(failed, 1);
    }

    #[test]
    fn protocol_send_failure_names_frame_and_transport_error() {
        assert_eq!(
            protocol_send_failure_message("owner metadata manifest", "socket closed"),
            "Failed to send owner metadata manifest: socket closed"
        );
    }

    #[test]
    fn retry_budget_is_shared_across_connection_stages() {
        let mut retry_count = 0;
        let mut retry_delay = Duration::from_millis(500);
        assert_eq!(
            take_retry_slot(&mut retry_count, &mut retry_delay),
            Some(Duration::from_millis(500))
        );
        assert_eq!(
            take_retry_slot(&mut retry_count, &mut retry_delay),
            Some(Duration::from_secs(1))
        );
        assert_eq!(
            take_retry_slot(&mut retry_count, &mut retry_delay),
            Some(Duration::from_secs(2))
        );
        assert_eq!(take_retry_slot(&mut retry_count, &mut retry_delay), None);
        assert_eq!(retry_count, MAX_SYNC_RETRIES);
    }

    #[tokio::test]
    async fn missing_manifest_frame_fails_the_current_attempt() {
        let expected = Arc::new(Mutex::new(HashSet::from([
            "agent".to_string(),
            "group".to_string(),
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
                assert!(message.contains("agent"));
                assert!(message.contains("group"));
            }
            _ => panic!("unexpected deadline command"),
        }
    }

    #[tokio::test]
    async fn missing_topic_hash_frame_fails_the_current_attempt() {
        let expected = Arc::new(AsyncMutex::new(Some(HashSet::from(
            ["topic-a".to_string()],
        ))));
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
    fn protocol_1_2_version_ack_is_strict_and_uses_public_field_names() {
        let ack = parse_version_ack(&json!({
            "type": "VERSION_ACK",
            "pluginVersion": "1.2.0",
            "protocolVersion": "1.2",
        }))
        .expect("strict 1.2 acknowledgement");
        assert_eq!(ack.plugin_version, "1.2.0");
        assert_eq!(ack.protocol_version, "1.2");

        assert!(parse_version_ack(&json!({
            "type": "VERSION_ACK",
            "version": "1.2.0",
        }))
        .is_err());
        assert!(parse_version_ack(&json!({
            "type": "VERSION_ACK",
            "pluginVersion": "1.2.0",
            "protocolVersion": 1.2,
        }))
        .is_err());
    }

    #[test]
    fn handshake_preserves_a_structured_desktop_error_before_version_ack() {
        let result = parse_version_handshake_payload(&json!({
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
        }));
        let VersionHandshakeError::Remote(encoded) = result.expect_err("remote error") else {
            panic!("expected structured remote error");
        };
        assert_eq!(
            decode_wire_sync_error(&encoded)
                .expect("encoded error")
                .code,
            "PLUGIN_VERSION_MISMATCH"
        );
        assert!(parse_version_handshake_payload(&json!({
            "type": "SYNC_LOG_EVENT",
            "level": "info"
        }))
        .expect("log frame")
        .is_none());
    }

    #[test]
    fn changed_topic_list_rejects_wrong_types_empty_ids_and_duplicates() {
        assert_eq!(
            parse_unique_nonempty_strings(
                &json!(["topic-a", "topic-b"]),
                "changedTopics",
                MAX_SYNC_TOPICS,
            )
            .expect("valid topic list"),
            vec!["topic-a".to_string(), "topic-b".to_string()]
        );
        assert!(
            parse_unique_nonempty_strings(&json!("topic-a"), "changedTopics", MAX_SYNC_TOPICS,)
                .is_err()
        );
        assert!(
            parse_unique_nonempty_strings(&json!([""]), "changedTopics", MAX_SYNC_TOPICS,).is_err()
        );
        assert!(parse_unique_nonempty_strings(
            &json!(["topic-a", "topic-a"]),
            "changedTopics",
            MAX_SYNC_TOPICS,
        )
        .is_err());
        assert!(
            parse_unique_nonempty_strings(&json!(["topic-a", "topic-b"]), "changedTopics", 1,)
                .is_err()
        );
    }

    #[test]
    fn phase3_diff_batches_enforce_serialized_byte_budget() {
        use crate::vcp_modules::sync_pipeline::phase3_message::TopicLocalState;
        use std::collections::HashMap;

        let mut states = HashMap::new();
        for index in 0..3 {
            states.insert(
                format!("topic-{index}"),
                TopicLocalState {
                    owner_type: "agent".to_string(),
                    owner_id: "agent-a".to_string(),
                    topic_hash: "h".repeat(64),
                    messages: HashMap::from([(
                        format!("message-{index}-{}", "x".repeat(3 * 1024 * 1024)),
                        "m".repeat(64),
                    )]),
                },
            );
        }
        let batches = build_diff_batches(states).expect("bounded batches");
        assert!(batches.len() >= 2);
        for batch in batches {
            let bytes = serde_json::to_vec(&json!({
                "type": "SYNC_MESSAGE_DIFF_BATCH",
                "topics": batch,
            }))
            .expect("serialize batch");
            assert!(bytes.len() <= MAX_WS_DIFF_BATCH_BYTES);
        }

        let oversized = HashMap::from([(
            "topic-oversized".to_string(),
            TopicLocalState {
                owner_type: "agent".to_string(),
                owner_id: "agent-a".to_string(),
                topic_hash: "h".repeat(64),
                messages: HashMap::from([("x".repeat(MAX_WS_DIFF_BATCH_BYTES), "m".repeat(64))]),
            },
        )]);
        assert!(build_diff_batches(oversized).is_err());

        let too_many_messages = (0..=MAX_MESSAGES_PER_BATCH)
            .map(|index| (format!("message-{index}"), "m".repeat(64)))
            .collect();
        let oversized_topic = HashMap::from([(
            "topic-too-many".to_string(),
            TopicLocalState {
                owner_type: "agent".to_string(),
                owner_id: "agent-a".to_string(),
                topic_hash: "h".repeat(64),
                messages: too_many_messages,
            },
        )]);
        assert!(build_diff_batches(oversized_topic).is_err());
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
            json!({"type":"PHASE_COMPLETED","phase":"messages","sessionId":11,"attemptId":4,"nonce":"exact-nonce"}),
            json!({"type":"PHASE_ACK","phase":"owner_metadata","sessionId":11,"attemptId":4,"nonce":"exact-nonce"}),
            json!({"type":"PHASE_ACK","phase":"messages","sessionId":10,"attemptId":4,"nonce":"exact-nonce"}),
            json!({"type":"PHASE_ACK","phase":"messages","sessionId":11,"attemptId":3,"nonce":"exact-nonce"}),
            json!({"type":"PHASE_ACK","phase":"messages","sessionId":11,"attemptId":4,"nonce":"stale-nonce"}),
            json!({"type":"PHASE_ACK","phase":"messages","sessionId":11,"attemptId":4}),
        ];
        for payload in mismatches {
            assert!(!consume_final_ack(&pending, &payload));
            assert_eq!(
                pending.lock().expect("pending lock").as_ref(),
                Some(&expected)
            );
        }

        let exact = json!({
            "type": "PHASE_ACK",
            "phase": "messages",
            "sessionId": 11,
            "attemptId": 4,
            "nonce": "exact-nonce"
        });
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

    #[test]
    fn test_check_loopback_on_mobile() {
        // Test Android mode (is_android = true)
        assert!(check_loopback_on_mobile("ws://127.0.0.1:3000", true));
        assert!(check_loopback_on_mobile(
            "ws://localhost:8080/ws-sync",
            true
        ));
        assert!(!check_loopback_on_mobile("ws://192.168.1.100:3000", true));
        assert!(!check_loopback_on_mobile("ws://my-pc.local:3000", true));

        // Test non-Android mode (is_android = false)
        assert!(!check_loopback_on_mobile("ws://127.0.0.1:3000", false));
        assert!(!check_loopback_on_mobile(
            "ws://localhost:8080/ws-sync",
            false
        ));
    }

    #[test]
    fn test_classify_unauthorized_token() {
        let response = Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .body(None)
            .unwrap();
        let err = WsError::Http(response);

        assert_eq!(
            classify_connection_failure("ws://192.168.1.100:3000/ws-sync", &err),
            "TOKEN_MISMATCH"
        );
    }

    #[test]
    fn test_classify_not_found_path() {
        let response = Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(None)
            .unwrap();
        let err = WsError::Http(response);

        assert_eq!(
            classify_connection_failure("ws://192.168.1.100:3000/ws-sync", &err),
            "WS_PATH_INVALID"
        );
    }

    #[test]
    fn test_classify_connection_refused() {
        let io_err =
            std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "connection refused");
        let err = WsError::Io(io_err);

        assert_eq!(
            classify_connection_failure("ws://192.168.1.100:1/ws-sync", &err),
            "CONNECTION_REFUSED"
        );
    }

    #[test]
    fn test_classify_network_unreachable_as_closed_port() {
        let io_err = std::io::Error::new(
            std::io::ErrorKind::AddrNotAvailable,
            "address not available",
        );
        let err = WsError::Io(io_err);

        assert_eq!(
            classify_connection_failure("ws://non-existent-domain-vcp-test.xyz/ws-sync", &err),
            "CONNECTION_REFUSED"
        );
    }
}

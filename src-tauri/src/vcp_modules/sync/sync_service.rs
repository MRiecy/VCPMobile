use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::db_write_queue::DbWriteQueue;
use crate::vcp_modules::sync_executor::PullExecutor;
use crate::vcp_modules::sync_hash::HashInitializer;
use crate::vcp_modules::sync_logger::{LogLevel, SyncLogger};
use crate::vcp_modules::sync_pipeline::{Phase1Metadata, Phase3Message, SyncPipeline};
use crate::vcp_modules::sync_types::SyncDataType;
use crate::vcp_modules::vcp_log_service::get_vcp_log_status_internal;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::future::Future;
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

const EXPECTED_PLUGIN_VERSION: &str = "1.1.0";
const WIRE_PROTOCOL_VERSION: &str = "1.1";
const VERSION_CHECK_TIMEOUT: Duration = Duration::from_secs(5);
const WS_OPERATION_TIMEOUT: Duration = Duration::from_secs(5);
const PHASE3_WATCHDOG_TICK: Duration = Duration::from_secs(10);
const PHASE3_WATCHDOG_STUCK_TICKS: u32 = 6;
const FINAL_ACK_TIMEOUT: Duration = Duration::from_secs(30);
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
    log::error!("[SyncService] {message}");
    emit_sync_log(app_handle, "error", &message);
    publish_sync_status(app_handle, session_id, status, "error", &message).await;
    let _ = close_ws_with_deadline(ws_stream).await;
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
            message: format!(
                "Desktop final sync acknowledgement timed out after {} seconds",
                deadline.as_secs()
            ),
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
    join_handle: JoinHandle<()>,
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
    },
    StartManualSync,
    SendWsMessage {
        attempt_id: u64,
        value: serde_json::Value,
    },
    FailAttempt {
        attempt_id: u64,
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

fn parse_unique_nonempty_strings(value: &Value, field: &str) -> Result<Vec<String>, String> {
    let values = value
        .as_array()
        .ok_or_else(|| format!("{field} must be an array"))?;
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

async fn publish_sync_status<R: Runtime>(
    app_handle: &AppHandle<R>,
    session_id: u64,
    status: &Arc<RwLock<String>>,
    next_status: &str,
    message: &str,
) {
    let error =
        (next_status == "error").then(|| ("SYNC_ATTEMPT_FAILED", message, Vec::<String>::new()));
    publish_sync_status_inner(app_handle, session_id, status, next_status, message, error).await;
}

async fn publish_sync_error<R: Runtime>(
    app_handle: &AppHandle<R>,
    session_id: u64,
    status: &Arc<RwLock<String>>,
    code: &str,
    message: &str,
    failed_topic_ids: Vec<String>,
) {
    publish_sync_status_inner(
        app_handle,
        session_id,
        status,
        "error",
        message,
        Some((code, message, failed_topic_ids)),
    )
    .await;
}

async fn publish_sync_status_inner<R: Runtime>(
    app_handle: &AppHandle<R>,
    session_id: u64,
    status: &Arc<RwLock<String>>,
    next_status: &str,
    message: &str,
    error: Option<(&str, &str, Vec<String>)>,
) {
    let sync_state = app_handle.state::<SyncState>();
    let _owner_commit = sync_state.owner_commit.lock().await;
    if sync_state.current_session_id.load(Ordering::SeqCst) != session_id {
        return;
    }

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
    if let Some((code, original_message, failed_topic_ids)) = error {
        payload["error"] = json!({
            "code": code,
            "message": original_message,
            "failedTopicIds": failed_topic_ids,
        });
    }

    // 统一使用 vcp-system-event 发射，type 为明确的 vcp-sync-status
    let _ = app_handle.emit(
        "vcp-system-event",
        json!({
            "type": "vcp-sync-status",
            "status": next_status,
            "message": message,
            "source": "Sync",
            "sessionId": session_id,
            "error": payload.get("error").cloned().unwrap_or(Value::Null),
        }),
    );

    // 直接发射前端 syncSession 监听的 vcp-sync-status
    let _ = app_handle.emit("vcp-sync-status", payload);

    // 同步发射到 Mini Log Terminal
    let level = match next_status {
        "open" => "success",
        "error" => "error",
        "connecting" => "info",
        _ => "info",
    };
    emit_sync_log(app_handle, level, message);
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

#[derive(Debug, Clone, serde::Serialize)]
pub struct ConnectionErrorDiagnosis {
    pub error_code: String,
    pub error_message: String,
    pub solution: String,
    pub error_detail: String,
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

async fn diagnose_connection_failure(
    _ws_url: &str,
    http_url: &str,
    err: &tokio_tungstenite::tungstenite::error::Error,
) -> ConnectionErrorDiagnosis {
    let err_detail = err.to_string();

    // 如果系统已经明确报告地址/网络不可达，直接返回 NETWORK_UNREACHABLE，
    // 避免依赖外部网络状态的 HTTP 探测导致误判（也消除相关单元测试的脆弱性）。
    if let tokio_tungstenite::tungstenite::error::Error::Io(io_err) = err {
        if matches!(
            io_err.kind(),
            std::io::ErrorKind::AddrNotAvailable
                | std::io::ErrorKind::NetworkUnreachable
                | std::io::ErrorKind::HostUnreachable
        ) {
            return ConnectionErrorDiagnosis {
                error_code: "NETWORK_UNREACHABLE".to_string(),
                error_message: "网络不可达或地址无效".to_string(),
                solution: "无法建立连接。请检查手机网络状态，确保 WiFi 已连接且配置了正确的电脑端局域网 IP 和端口。"
                    .to_string(),
                error_detail: err_detail.clone(),
            };
        }
    }

    let is_android = cfg!(target_os = "android");
    if check_loopback_on_mobile(_ws_url, is_android) {
        return ConnectionErrorDiagnosis {
            error_code: "CONFIG_LOOPBACK_ON_MOBILE".to_string(),
            error_message: "移动端配置了本地回环地址".to_string(),
            solution: "移动设备无法通过 127.0.0.1 或 localhost 访问电脑端的服务。请确认电脑与手机连接在同一个 WiFi 下，并在移动端设置中将同步 IP 改为电脑的局域网 IP（例如：192.168.1.100）。".to_string(),
            error_detail: err_detail.clone(),
        };
    }

    if let tokio_tungstenite::tungstenite::error::Error::Http(response) = err {
        let status = response.status();
        if status == 401 || status == 403 {
            return ConnectionErrorDiagnosis {
                error_code: "TOKEN_MISMATCH".to_string(),
                error_message: "身份认证失败（Token 错误）".to_string(),
                solution: "移动端设置的同步令牌与桌面端不匹配。请检查移动端设置中的『同步令牌』是否与电脑端 VCPMobileSync 插件的 config.env 中的 SYNC_TOKEN 完全一致。".to_string(),
                error_detail: format!("HTTP Status: {}, Details: {}", status, err_detail),
            };
        } else if status == 404 {
            return ConnectionErrorDiagnosis {
                error_code: "WS_PATH_INVALID".to_string(),
                error_message: "同步服务路径不存在 (404)".to_string(),
                solution: "已成功连上服务器，但同步服务路径未被识别。请确保电脑端 VCPToolBox / VCPChat 已经启用且已加载移动端同步插件 (VCPMobileSync)，或检查同步 IP 和端口配置。".to_string(),
                error_detail: err_detail.clone(),
            };
        } else {
            return ConnectionErrorDiagnosis {
                error_code: "HTTP_HANDSHAKE_REJECTED".to_string(),
                error_message: format!("握手被服务器拒绝 (HTTP {})", status.as_u16()),
                solution: "握手请求被服务器拒绝。请检查服务器状态、端口配置或尝试重启电脑端服务。"
                    .to_string(),
                error_detail: err_detail.clone(),
            };
        }
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap_or_default();

    match client.get(http_url).send().await {
        Ok(res) => {
            let status = res.status();
            if status.is_success() || status.as_u16() == 401 || status.as_u16() == 403 {
                ConnectionErrorDiagnosis {
                    error_code: "WS_UPGRADE_FAILED".to_string(),
                    error_message: "HTTP 访问正常，但 WebSocket 升级失败".to_string(),
                    solution: "网络通路正常，但无法建立 WebSocket 通道。可能是桌面端的同步插件（VCPMobileSync）未正常启动，或者代理软件/VPN 拦截了 WebSocket 握手协议，请关闭 VPN 或在电脑端控制台查看 VCPMobileSync 日志。".to_string(),
                    error_detail: format!("HTTP Status: {}, WS Err: {}", status, err_detail),
                }
            } else {
                ConnectionErrorDiagnosis {
                    error_code: "HTTP_PROBE_ERROR".to_string(),
                    error_message: format!("HTTP 探测返回异常 (HTTP {})", status.as_u16()),
                    solution: "已连上服务器 IP 和端口，但 HTTP 请求返回异常。请确认桌面端服务运行正常并加载了正确的同步插件。".to_string(),
                    error_detail: format!("HTTP Status: {}, WS Err: {}", status, err_detail),
                }
            }
        }
        Err(req_err) => {
            let req_err_str = req_err.to_string();
            if req_err.is_timeout() {
                ConnectionErrorDiagnosis {
                    error_code: "NETWORK_TIMEOUT".to_string(),
                    error_message: "连接超时，无法访问服务器".to_string(),
                    solution: "网络请求超时。请确保：1. 电脑和手机连接在同一个 WiFi 下；2. 电脑没有开启可能会拦截局域网访问的防火墙或安全软件；3. 电脑的 IP 没有改变，与设置中的同步 IP 一致。".to_string(),
                    error_detail: format!("WS Err: {} | HTTP Probe Err: {}", err_detail, req_err_str),
                }
            } else if req_err.is_connect() {
                ConnectionErrorDiagnosis {
                    error_code: "CONNECTION_REFUSED".to_string(),
                    error_message: "连接被拒绝 (Connection Refused)".to_string(),
                    solution: "服务器主动拒绝了连接。请确保：1. 电脑上的 VCPToolBox / VCPChat 已经启动；2. 桌面端的移动端同步服务已经启用并且端口配置正确。".to_string(),
                    error_detail: format!("WS Err: {} | HTTP Probe Err: {}", err_detail, req_err_str),
                }
            } else {
                ConnectionErrorDiagnosis {
                    error_code: "NETWORK_UNREACHABLE".to_string(),
                    error_message: "网络不可达或地址无效".to_string(),
                    solution: "无法建立连接。请检查手机网络状态，确保 WiFi 已连接且配置了正确的电脑端局域网 IP 和端口。".to_string(),
                    error_detail: format!("WS Err: {} | HTTP Probe Err: {}", err_detail, req_err_str),
                }
            }
        }
    }
}

async fn run_sync_session(
    app_handle: AppHandle,
    session_id: u64,
    cancel_token: CancellationToken,
    tx: mpsc::UnboundedSender<SyncCommand>,
    mut rx: mpsc::UnboundedReceiver<SyncCommand>,
    connection_status: Arc<RwLock<String>>,
) {
    let handle_clone = app_handle.clone();
    let tx_internal = tx.clone();
    let connection_status_for_task = connection_status.clone();

    let http_client = reqwest::Client::new();
    let mut retry_count = 0u32;
    const MAX_RETRIES: u32 = 3;
    let mut retry_delay = Duration::from_millis(500);
    let mut next_attempt_id = 0u64;

    let db = app_handle.state::<DbState>();
    let mut write_queue = DbWriteQueue::new(db.pool.clone(), db.path.clone());
    let sync_log_level = LogLevel::Info;
    let log_dir = app_handle
        .path()
        .app_log_dir()
        .ok()
        .map(|d| d.join("sync_logs"));
    let sync_logger = Arc::new(std::sync::Mutex::new(SyncLogger::new_session(
        sync_log_level,
        log_dir,
        Some(app_handle.clone()),
    )));
    {
        let sync_state = app_handle.state::<SyncState>();
        let _owner_commit = sync_state.owner_commit.lock().await;
        if sync_state.current_session_id.load(Ordering::SeqCst) != session_id {
            return;
        }
        let log_path = {
            let logger = sync_logger.lock();
            logger.ok().and_then(|l| l.log_path().cloned())
        };
        if let Some(path) = log_path {
            let mut guard = sync_state.current_log_path.write().await;
            *guard = Some(path.to_string_lossy().to_string());
        }
        let mut logger_guard = sync_state.current_logger.write().unwrap();
        *logger_guard = Some(sync_logger.clone());
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
                        publish_sync_status(
                            &handle_clone,
                            session_id,
                            &connection_status_for_task,
                            "error",
                            "同步服务 URL 未配置",
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
                            publish_sync_status(
                                &handle_clone,
                                session_id,
                                &connection_status_for_task,
                                "error",
                                "同步服务 URL 格式非法",
                            )
                            .await;
                            break;
                        }
                    };
                    (ws_addr, s.sync_http_url.clone())
                }
                Err(_) => {
                    emit_sync_log(&handle_clone, "error", "无法读取同步配置");
                    publish_sync_status(
                        &handle_clone,
                        session_id,
                        &connection_status_for_task,
                        "error",
                        "无法读取同步配置",
                    )
                    .await;
                    break;
                }
            }
        };

        publish_sync_status(
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
                retry_count = 0;
                retry_delay = Duration::from_millis(500);

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
                                        return parse_version_ack(&payload)
                                            .map_err(VersionHandshakeError::Protocol);
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
                                publish_sync_status(
                                    &handle_clone,
                                    session_id,
                                    &connection_status_for_task,
                                    "error",
                                    &format!(
                                        "桌面端插件 v{} / 协议 {} 与期望 v{} / 协议 {} 不兼容",
                                        version_ack.plugin_version,
                                        version_ack.protocol_version,
                                        EXPECTED_PLUGIN_VERSION,
                                        WIRE_PROTOCOL_VERSION,
                                    ),
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
                                    "error",
                                    &format!("❌ 同步连接失败 [WS_CLOSED]: {}", err_msg),
                                );
                                emit_sync_log(
                                    &handle_clone,
                                    "error",
                                    "👉 排查建议: 请检查桌面端控制台日志以获取详细关闭原因。",
                                );
                                publish_sync_error(
                                    &handle_clone,
                                    session_id,
                                    &connection_status_for_task,
                                    "WS_CLOSED",
                                    &err_msg,
                                    Vec::new(),
                                )
                                .await;
                            }
                            break;
                        }
                        Ok(Err(VersionHandshakeError::Transport(message))) => {
                            emit_sync_log(
                                &handle_clone,
                                "error",
                                &format!("❌ 同步连接失败 [WS_RECEIVE_FAILED]: {message}"),
                            );
                            emit_sync_log(&handle_clone, "error", "👉 排查建议: 请确认桌面端服务正常运行，且同步 Token 与网络无异常。");
                            publish_sync_error(
                                &handle_clone,
                                session_id,
                                &connection_status_for_task,
                                "WS_RECEIVE_FAILED",
                                &message,
                                Vec::new(),
                            )
                            .await;
                            break;
                        }
                        Err(_) => {
                            emit_sync_log(
                                &handle_clone,
                                "error",
                                "❌ 同步连接失败 [VERSION_CHECK_TIMEOUT]: 版本验证超时",
                            );
                            emit_sync_log(&handle_clone, "error", "👉 排查建议: 桌面端服务响应缓慢，或者当前网络异常。请检查局域网连接，或尝试重启电脑端服务。");
                            publish_sync_error(
                                &handle_clone,
                                session_id,
                                &connection_status_for_task,
                                "VERSION_CHECK_TIMEOUT",
                                "版本验证超时",
                                Vec::new(),
                            )
                            .await;
                            break;
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
                    break 'session;
                }
                publish_sync_status(
                    &handle_clone,
                    session_id,
                    &connection_status_for_task,
                    "open",
                    "同步服务已连接",
                )
                .await;

                // 同步连接成功提示
                let _ = handle_clone.emit(
                    "vcp-system-event",
                    json!({
                        "type": "vcp-log-message",
                        "data": {
                            "id": "vcp_sync_connection_status",
                            "status": "success",
                            "tool_name": "Sync",
                            "content": "已连接桌面端",
                            "source": "Sync"
                        }
                    }),
                );
                let db = handle_clone.state::<DbState>();
                if let Err(e) = HashInitializer::ensure_all_agent_hashes(&db.pool).await {
                    if let Ok(mut logger) = sync_logger_task.lock() {
                        logger.log(
                            LogLevel::Error,
                            "owner_metadata",
                            &format!("Failed to initialize agent hashes: {}", e),
                        );
                    }

                    // 同步初始化失败提示
                    let _ = handle_clone.emit(
                        "vcp-system-event",
                        json!({
                            "type": "vcp-log-message",
                            "data": {
                                "id": "vcp_sync_connection_status",
                                "status": "error",
                                "tool_name": "同步初始化失败",
                                "content": "数据库初始化失败",
                                "source": "Sync"
                            }
                        }),
                    );
                    if cancelled_during(&cancel_token, retry_delay).await {
                        break;
                    }
                    retry_delay = (retry_delay * 2).min(Duration::from_secs(60));
                    continue;
                }
                if let Err(e) = HashInitializer::ensure_all_group_hashes(&db.pool).await {
                    if let Ok(mut logger) = sync_logger_task.lock() {
                        logger.log(
                            LogLevel::Error,
                            "owner_metadata",
                            &format!("Hash init error: {}", e),
                        );
                    }

                    // 同步初始化失败提示
                    let _ = handle_clone.emit(
                        "vcp-system-event",
                        json!({
                            "type": "vcp-log-message",
                            "data": {
                                "id": "vcp_sync_connection_status",
                                "status": "error",
                                "tool_name": "同步初始化失败",
                                "content": "数据库初始化失败",
                                "source": "Sync"
                            }
                        }),
                    );
                    if cancelled_during(&cancel_token, retry_delay).await {
                        break;
                    }
                    retry_delay = (retry_delay * 2).min(Duration::from_secs(60));
                    continue;
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
                            if let Err(e) = send_ws_with_deadline(&mut ws_stream, Message::Ping(vec![].into())).await {
                                log::warn!("[SyncService] Failed to send WebSocket Ping: {}", e);
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
                                        let _ = tx_internal.send(SyncCommand::StartTopicValidation { attempt_id });
                                    } else {
                                        if let Ok(manifest) = Phase1Metadata::build_targeted_topic_manifest(&db.pool, &owners).await {
                                            manifest_phase.store(2, Ordering::SeqCst);
                                            expected_manifest_count.store(1, Ordering::SeqCst);
                                            manifest_responses_received.store(0, Ordering::SeqCst);
                                            pending_tasks_task.store(0, Ordering::SeqCst);
                                            total_tasks_task.store(0, Ordering::SeqCst);

                                            if let Ok(mut logger) = sync_logger_task.lock() {
                                                logger.start_phase("topic_metadata", 1);
                                                logger.log(LogLevel::Info, "topic_metadata", "=== Phase 2: Pulling Topic Metadata ===");
                                            }
                                            if let Err(error) = send_ws_with_deadline(&mut ws_stream, Message::Text(json!({ "type": "PHASE_START", "phase": "topic_metadata" }).to_string().into())).await {
                                                fatal_error = true;
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
                                                fatal_error = true;
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
                                        } else {
                                            let _ = tx_internal.send(SyncCommand::StartTopicValidation { attempt_id });
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
                                            let mut hash_map = serde_json::Map::new();
                                            for (topic_id, (conf_h, cont_h)) in topic_hashes {
                                                hash_map.insert(topic_id, json!({
                                                    "configHash": conf_h,
                                                    "contentHash": cont_h
                                                }));
                                            }
                                            let msg = json!({
                                                "type": "SYNC_TOPIC_HASH_BATCH_V2",
                                                "hashes": hash_map,
                                            });
                                            if let Err(error) = send_ws_with_deadline(&mut ws_stream, Message::Text(msg.to_string().into())).await {
                                                fatal_error = true;
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
                                        }
                                        Err(e) => {
                                            log::error!("[SyncService] Failed to get targeted topic hashes: {}", e);
                                            let _ = tx_internal.send(SyncCommand::StartMessages { attempt_id });
                                        }
                                    }
                                },
                                crate::vcp_modules::sync_pipeline::pipeline::PipelineCommand::StartMessages => {
                                    if let Ok(mut logger) = sync_logger_task.lock() {
                                        logger.start_phase("messages", 0);
                                        logger.log(LogLevel::Info, "messages", "=== Phase 3: Messages ===");
                                    }
                                    if let Err(error) = send_ws_with_deadline(&mut ws_stream, Message::Text(json!({ "type": "PHASE_START", "phase": "messages" }).to_string().into())).await {
                                        fatal_error = true;
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

                                                // 清空可能残留的旧批次，防止断线重连后发送过时数据
                                                {
                                                    let mut pending = pending_diff_batches.lock().await;
                                                    pending.clear();
                                                }
                                                // 按消息数量分批，每批最多 10000 条消息，避免超大 WS payload
                                                let batches = build_diff_batches(topic_states);
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
                                                        fatal_error = true;
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

                                                    let tracker = pending_msg_topics_task.clone();
                                                    let tx_watchdog = tx_internal.clone();
                                                    task_tracker.spawn(async move {
                                                        let mut last_completed = 0usize;
                                                        let mut stuck_ticks = 0u32;
                                                        loop {
                                                            tokio::time::sleep(PHASE3_WATCHDOG_TICK).await;
                                                            let completed = tracker.completed.lock().await.len();
                                                            let total = tracker.total.load(Ordering::SeqCst);
                                                            if completed >= total {
                                                                break;
                                                            }
                                                            if completed == last_completed {
                                                                stuck_ticks += 1;
                                                            } else {
                                                                last_completed = completed;
                                                                stuck_ticks = 0;
                                                            }
                                                            if stuck_ticks >= PHASE3_WATCHDOG_STUCK_TICKS {
                                                                let _ = tx_watchdog.send(SyncCommand::FailAttempt {
                                                                    attempt_id,
                                                                    message: format!(
                                                                        "Phase 3 timed out: completed {}/{} topics",
                                                                        completed, total
                                                                    ),
                                                                });
                                                                break;
                                                            }
                                                        }
                                                    }).await;
                                                } else {
                                                    let _ = tx_internal.send(SyncCommand::FailAttempt {
                                                        attempt_id,
                                                        message: "Phase 3 produced no request batch for changed topics".to_string(),
                                                    });
                                                }
                                            }
                                            Err(e) => {
                                                log::error!("[SyncService] Failed to get topic message hashes: {}", e);
                                                let _ = tx_internal.send(SyncCommand::FailAttempt {
                                                    attempt_id,
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
                                        fatal_error = true;
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
                                            publish_sync_status(
                                                &handle_clone,
                                                session_id,
                                                &connection_status_for_task,
                                                "error",
                                                &message,
                                            ).await;
                                            let _ = close_ws_with_deadline(&mut ws_stream).await;
                                            break;
                                        }
                                        if let Err(error) = send_ws_with_deadline(&mut ws_stream, Message::Text(json!({ "type": "PHASE_COMPLETED", "phase": "owner_metadata" }).to_string().into())).await {
                                            fatal_error = true;
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
                                            publish_sync_status(
                                                &handle_clone,
                                                session_id,
                                                &connection_status_for_task,
                                                "error",
                                                &message,
                                            ).await;
                                            let _ = close_ws_with_deadline(&mut ws_stream).await;
                                            break;
                                        }
                                        if let Err(error) = send_ws_with_deadline(&mut ws_stream, Message::Text(json!({ "type": "PHASE_COMPLETED", "phase": "topic_metadata" }).to_string().into())).await {
                                            fatal_error = true;
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
                                            publish_sync_status(
                                                &handle_clone,
                                                session_id,
                                                &connection_status_for_task,
                                                "error",
                                                &message,
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
                                            publish_sync_status(
                                                &handle_clone,
                                                session_id,
                                                &connection_status_for_task,
                                                "error",
                                                &message,
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
                                                    message: format!(
                                                        "Failed to send final messages phase completion: {}",
                                                        error
                                                    ),
                                                });
                                            }
                                        }
                                    }
                                },
                                SyncCommand::NotifyDelete { data_type, id } => {
                                    let msg = json!({ "type": "SYNC_ENTITY_DELETE", "id": id, "dataType": data_type });
                                    if let Err(error) = send_ws_with_deadline(&mut ws_stream, Message::Text(msg.to_string().into())).await {
                                        fatal_error = true;
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
                                SyncCommand::StartManualSync => {
                                    let db = handle_clone.state::<DbState>();
                                    manifest_phase.store(1, Ordering::SeqCst);
                                    if let Ok(manifests) = Phase1Metadata::build_phase1_manifests(&db.pool).await {
                                        let count = manifests.len() as u32;
                                        expected_manifest_count.store(count, Ordering::SeqCst);
                                        manifest_responses_received.store(0, Ordering::SeqCst);

                                        if let Ok(mut logger) = sync_logger_task.lock() {
                                            logger.set_phase_expected("owner_metadata", count);
                                        }
                                        for manifest in manifests {
                                            let msg = json!({
                                                "type": "SYNC_MANIFEST",
                                                "data": manifest.items,
                                                "dataType": manifest.data_type,
                                                "phase": 1 // Explicit Phase ID
                                            });
                                            if let Err(error) = send_ws_with_deadline(&mut ws_stream, Message::Text(msg.to_string().into())).await {
                                                fatal_error = true;
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
                                    }
                                },
                                SyncCommand::SendWsMessage { attempt_id: command_attempt, value } => {
                                    if command_attempt != attempt_id { continue; }
                                    if let Err(error) = send_ws_with_deadline(&mut ws_stream, Message::Text(value.to_string().into())).await {
                                        fatal_error = true;
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
                                SyncCommand::FailAttempt { attempt_id: command_attempt, message } => {
                                    if command_attempt != attempt_id { continue; }
                                    fatal_error = true;
                                    emit_sync_log(&handle_clone, "error", &message);
                                    publish_sync_status(
                                        &handle_clone,
                                        session_id,
                                        &connection_status_for_task,
                                        "error",
                                        &message,
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
                                        publish_sync_status(
                                            &handle_clone,
                                            session_id,
                                            &connection_status_for_task,
                                            "error",
                                            message,
                                        ).await;
                                        let _ = close_ws_with_deadline(&mut ws_stream).await;
                                        break;
                                    }
                                    Err(error) => {
                                        let message = format!("Malformed sync protocol frame: {error}");
                                        fatal_error = true;
                                        publish_sync_status(
                                            &handle_clone,
                                            session_id,
                                            &connection_status_for_task,
                                            "error",
                                            &message,
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
                                            message: format!("Failed to read sync settings: {error}"),
                                        });
                                        continue;
                                    }
                                };

                                match payload["type"].as_str() {
                                    Some("SYNC_ENTITY_UPDATE") => {
                                        let id = payload["id"].as_str().unwrap_or_default().to_string();
                                        let owner_type = payload["ownerType"].as_str().unwrap_or("agent").to_string();
                                        let Some(data_type) = parse_sync_data_type(&payload["dataType"]) else { continue; };
                                        let failure_tx = tx_internal.clone();
                                        task_tracker.spawn(async move {
                                            let result: Result<(), String> = async {
                                                let _permit = sem.acquire().await;
                                                let settings = crate::vcp_modules::settings_manager::read_settings(h.clone(), h.state()).await?;
                                                match &data_type {
                                                SyncDataType::Agent => PullExecutor::pull_agent(&h, &c, &base, &settings.sync_token, &id, &wq).await,
                                                SyncDataType::Group => PullExecutor::pull_group(&h, &c, &base, &settings.sync_token, &id, &wq).await,
                                                SyncDataType::Topic => {
                                                    if owner_type == "group" {
                                                        PullExecutor::pull_group_topic(&h, &c, &base, &settings.sync_token, &id, &wq).await
                                                    } else {
                                                        PullExecutor::pull_agent_topic(&h, &c, &base, &settings.sync_token, &id, &wq).await
                                                    }
                                                },
                                                _ => Err(format!("unsupported entity update type: {data_type:?}")),
                                                }
                                            }.await;
                                            if let Err(error) = result {
                                                let _ = failure_tx.send(SyncCommand::FailAttempt {
                                                    attempt_id,
                                                    message: format!("SYNC_ENTITY_UPDATE failed for {id}: {error}"),
                                                });
                                            }
                                        }).await;
                                    },
                                    Some("SYNC_DELETE_NOTIFY") => {
                                        use crate::vcp_modules::sync_executor::delete_executor::DeleteExecutor;
                                        let id = payload["id"].as_str().unwrap_or_default().to_string();
                                        let Some(data_type) = parse_sync_data_type(&payload["dataType"]) else { continue; };
                                        let failure_tx = tx_internal.clone();
                                        task_tracker.spawn(async move {
                                            let result = match &data_type {
                                                SyncDataType::Agent => DeleteExecutor::soft_delete_agent(&h, &id).await,
                                                SyncDataType::Group => DeleteExecutor::soft_delete_group(&h, &id).await,
                                                SyncDataType::Topic => DeleteExecutor::soft_delete_topic(&h, &id).await,
                                                SyncDataType::Avatar => {
                                                    let parts: Vec<&str> = id.split(':').collect();
                                                    if parts.len() == 2 {
                                                        DeleteExecutor::soft_delete_avatar(&h, parts[0], parts[1]).await
                                                    } else {
                                                        Err(format!("invalid avatar id: {id}"))
                                                    }
                                                },
                                                _ => Err(format!("unsupported delete notification type: {data_type:?}")),
                                            };
                                            if let Err(error) = result {
                                                let _ = failure_tx.send(SyncCommand::FailAttempt {
                                                    attempt_id,
                                                    message: format!("SYNC_DELETE_NOTIFY failed for {id}: {error}"),
                                                });
                                            }
                                        }).await;
                                    },
                                    Some("SYNC_ERROR") => {
                                        let Some(message) = payload.get("message").and_then(Value::as_str).filter(|value| !value.is_empty()) else {
                                            fatal_error = true;
                                            publish_sync_error(
                                                &handle_clone,
                                                session_id,
                                                &connection_status_for_task,
                                                "PROTOCOL_FRAME_INVALID",
                                                "SYNC_ERROR.message must be a non-empty string",
                                                Vec::new(),
                                            ).await;
                                            let _ = close_ws_with_deadline(&mut ws_stream).await;
                                            break;
                                        };
                                        let code = match payload.get("code") {
                                            Some(Value::String(code)) if !code.is_empty() => code.clone(),
                                            Some(Value::Number(code)) if code.is_u64() => code.to_string(),
                                            _ => {
                                                fatal_error = true;
                                                publish_sync_error(
                                                    &handle_clone,
                                                    session_id,
                                                    &connection_status_for_task,
                                                    "PROTOCOL_FRAME_INVALID",
                                                    "SYNC_ERROR.code must be a non-empty string or unsigned integer",
                                                    Vec::new(),
                                                ).await;
                                                let _ = close_ws_with_deadline(&mut ws_stream).await;
                                                break;
                                            }
                                        };
                                        let err_msg = format!("Desktop Error ({code}): {message}");
                                        log::error!("[SyncService] {}", err_msg);
                                        emit_sync_log(&handle_clone, "error", &err_msg);
                                        publish_sync_error(
                                            &handle_clone,
                                            session_id,
                                            &connection_status_for_task,
                                            &code,
                                            &err_msg,
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
                                        if let Err(e) = crate::vcp_modules::sync_executor::batch_diff_handler::BatchDiffHandler::handle_diff_batch(
                                            &h,
                                            &payload,
                                            &c,
                                            &base,
                                            &settings.sync_token,
                                            &pending_msg_topics_task,
                                            &tx_internal,
                                            &sync_logger_task,
                                            &wq,
                                            &pending_diff_batches,
                                            settings.sync_prerender_enabled,
                                            &uploaded_hashes,
                                            &task_tracker,
                                            &expected_phase3_batch,
                                            attempt_id,
                                        ).await {
                                            log::error!("[SyncService] BatchDiffHandler failed: {}", e);
                                            fatal_error = true;
                                            emit_sync_log(
                                                &handle_clone,
                                                "error",
                                                &e.message,
                                            );
                                            publish_sync_error(
                                                &handle_clone,
                                                session_id,
                                                &connection_status_for_task,
                                                &e.code,
                                                &e.message,
                                                e.failed_topic_ids,
                                            ).await;
                                            let _ = close_ws_with_deadline(&mut ws_stream).await;
                                            break;
                                        }
                                    },
                                    Some("SYNC_TOPIC_HASH_RESULTS") => {
                                        manifest_phase.store(3, Ordering::SeqCst); // 进入 Phase 2.5+，旧 Phase 2 看门狗失效
                                        match parse_unique_nonempty_strings(
                                            &payload["changedTopics"],
                                            "SYNC_TOPIC_HASH_RESULTS.changedTopics",
                                        ) {
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
                                            publish_sync_status(
                                                &handle_clone,
                                                session_id,
                                                &connection_status_for_task,
                                                "error",
                                                &message,
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

                                            // 发送同步完成提示
                                            let _ = handle_clone.emit(
                                                "vcp-system-event",
                                                json!({
                                                    "type": "vcp-log-message",
                                                    "data": {
                                                        "id": "vcp_sync_connection_status",
                                                        "status": "success",
                                                        "tool_name": "Sync",
                                                        "content": "同步完成",
                                                        "source": "Sync"
                                                    }
                                                }),
                                            );
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
                                    emit_sync_log(&handle_clone, "error", &format!("❌ 同步连接失败 [{}]: {}", error_code, err_msg));
                                    emit_sync_log(&handle_clone, "error", &format!("👉 排查建议: {}", solution));
                                    publish_sync_status(&handle_clone, session_id, &connection_status_for_task, "error", &err_msg).await;
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
                if sync_success {
                    break; // 同步完成，退出外层 loop
                } else {
                    if fatal_error {
                        break;
                    }
                    // 同步未成功完成，但内层循环已跳出（说明中途断网）
                    retry_count += 1;
                    if retry_count >= MAX_RETRIES {
                        let err_msg = "同步中途异常断开，已达到最大重试次数";
                        publish_sync_status(
                            &handle_clone,
                            session_id,
                            &connection_status_for_task,
                            "error",
                            err_msg,
                        )
                        .await;
                        break;
                    }
                    let backoff = retry_delay * 2u32.pow(retry_count - 1);
                    let err_msg = format!(
                        "同步中途异常断开，{:?} 后尝试重新连接... (次数: {}/{})",
                        backoff, retry_count, MAX_RETRIES
                    );
                    emit_sync_log(&handle_clone, "warn", &err_msg);
                    if cancel_token.is_cancelled() {
                        break;
                    }
                    if cancelled_during(&cancel_token, backoff).await {
                        break;
                    }
                    continue; // 重新尝试连接
                }
            }
            Err(e) => {
                let diagnosis = diagnose_connection_failure(&ws_url, &http_url, &e).await;
                let is_fatal = diagnosis.error_code == "CONFIG_LOOPBACK_ON_MOBILE"
                    || diagnosis.error_code == "TOKEN_MISMATCH"
                    || diagnosis.error_code == "WS_PATH_INVALID";

                if is_fatal || retry_count >= MAX_RETRIES {
                    emit_sync_log(
                        &handle_clone,
                        "error",
                        &format!(
                            "❌ 同步连接失败 [{}]: {}",
                            diagnosis.error_code, diagnosis.error_message
                        ),
                    );
                    emit_sync_log(
                        &handle_clone,
                        "error",
                        &format!("👉 排查建议: {}", diagnosis.solution),
                    );
                    emit_sync_log(
                        &handle_clone,
                        "error",
                        &format!("🔍 调试细节: {}", diagnosis.error_detail),
                    );

                    publish_sync_status(
                        &handle_clone,
                        session_id,
                        &connection_status_for_task,
                        "error",
                        &format!("同步连接失败: {}", diagnosis.error_message),
                    )
                    .await;
                    break;
                }

                let warn_msg = format!(
                    "连接失败，第 {} 次重试 | {} ({})",
                    retry_count + 1,
                    diagnosis.error_message,
                    diagnosis.error_code
                );
                emit_sync_log(&handle_clone, "warning", &warn_msg);

                retry_count += 1;
                if cancel_token.is_cancelled() {
                    break;
                }
                if cancelled_during(&cancel_token, retry_delay).await {
                    break;
                }
                retry_delay = (retry_delay * 2).min(Duration::from_secs(5));
            }
        }
    }

    // No session is considered stopped until its children can no longer enqueue
    // writes and the session-local queue has drained everything already accepted.
    cancel_token.cancel();
    if let Err(error) = write_queue.flush().await {
        let message = format!("Sync session shutdown write drain failed: {}", error);
        log::error!("[SyncService] {}", message);
        emit_sync_log(&app_handle, "error", &message);
        publish_sync_status(
            &app_handle,
            session_id,
            &connection_status,
            "error",
            &message,
        )
        .await;
    }
    // 失败 attempt 也可能已有部分实体写入；离开 session 前必须丢弃旧 Facade cache。
    crate::vcp_modules::sync::sync_finalize::invalidate_sync_entity_caches(&app_handle);

    let sync_state = app_handle.state::<SyncState>();
    let _owner_commit = sync_state.owner_commit.lock().await;
    if sync_state.current_session_id.load(Ordering::SeqCst) == session_id {
        sync_state.ws_sender.clear_if_owner(session_id);
        let mut logger_guard = sync_state.current_logger.write().unwrap();
        *logger_guard = None;
        drop(logger_guard);

        #[cfg(target_os = "android")]
        let _ = tauri_plugin_vcp_mobile::stream::stop_stream_service_inner(
            &app_handle,
            "[数据同步] VCP Mobile",
        );
    }
}

/// 每批最多包含的消息数，控制单次 WS payload 大小（约 10000 条消息 ≈ 1.5-2MB JSON）
const MAX_MESSAGES_PER_BATCH: usize = 10000;

fn build_diff_batches(
    topic_states: std::collections::HashMap<
        String,
        crate::vcp_modules::sync_pipeline::phase3_message::TopicLocalState,
    >,
) -> std::collections::VecDeque<serde_json::Map<String, serde_json::Value>> {
    let mut batches = std::collections::VecDeque::new();
    let mut current_batch = serde_json::Map::new();
    let mut current_msg_count = 0usize;

    for (topic_id, state) in topic_states {
        let msg_count = state.messages.len();
        // 如果当前批次非空且加入此 topic 会超限，先结算当前批次
        if current_msg_count > 0 && current_msg_count + msg_count > MAX_MESSAGES_PER_BATCH {
            batches.push_back(current_batch);
            current_batch = serde_json::Map::new();
            current_msg_count = 0;
        }

        let mut msg_map = serde_json::Map::new();
        for (msg_id, hash) in state.messages {
            msg_map.insert(msg_id, serde_json::Value::String(hash));
        }
        let topic_obj = serde_json::json!({
            "topicHash": state.topic_hash,
            "messages": msg_map,
        });
        current_batch.insert(topic_id, topic_obj);
        current_msg_count += msg_count;
    }

    if !current_batch.is_empty() {
        batches.push_back(current_batch);
    }

    batches
}

pub(crate) fn emit_sync_log<R: Runtime>(app_handle: &AppHandle<R>, level: &str, message: &str) {
    let _ = app_handle.emit(
        "vcp-log",
        serde_json::json!({
            "id": format!("{}_{}", level, chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)),
            "level": level,
            "category": "sync",
            "message": message,
        }),
    );

    // 整合：写入 log 文件和控制台！
    let sync_state = app_handle.state::<SyncState>();
    if let Some(logger_arc) = sync_state
        .current_logger
        .read()
        .ok()
        .and_then(|guard| guard.clone())
    {
        if let Ok(mut logger) = logger_arc.lock() {
            let log_level = match level {
                "error" => LogLevel::Error,
                "warn" | "warning" => LogLevel::Info,
                _ => LogLevel::Info,
            };
            logger.log_direct(log_level, "sync", message);
        }
    } else {
        let rust_log_level = match level {
            "error" => log::Level::Error,
            "warn" | "warning" => log::Level::Warn,
            _ => log::Level::Info,
        };
        log::log!(rust_log_level, "[Sync] [{}] {}", level, message);
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
    {
        let mut logger_guard = state.current_logger.write().unwrap();
        *logger_guard = None;
    }

    #[cfg(target_os = "android")]
    let _ = tauri_plugin_vcp_mobile::stream::stop_stream_service_inner(
        &handle,
        "[数据同步] VCP Mobile",
    );

    join_result
}

async fn cancel_and_join_session(session: SyncSessionHandle) -> Result<(), String> {
    log::info!("[SyncService] Cancelling session {}", session.session_id);
    session.cancel_token.cancel();
    let _ = session.command_tx.send(SyncCommand::Cancel);
    session
        .join_handle
        .await
        .map_err(|error| format!("同步会话退出失败: {}", error))
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
                return Err("同步已在进行中".to_string());
            }
        }
        session.take()
    };
    if let Some(finished_session) = finished_session {
        finished_session
            .join_handle
            .await
            .map_err(|error| format!("上一同步会话退出失败: {}", error))?;
    }

    // VCPLog 是全局重要通道，未连接时直接拦截同步，避免进入同步主循环后长时间挂起
    let log_status = get_vcp_log_status_internal().await;
    if log_status != "connected" {
        return Err("VCPLog 未连接，请先建立 VCPLog 连接后再进行同步".to_string());
    }

    let (tx, rx) = mpsc::unbounded_channel::<SyncCommand>();
    tx.send(SyncCommand::StartManualSync)
        .map_err(|e| e.to_string())?;
    let session_id = state.next_session_id.fetch_add(1, Ordering::SeqCst) + 1;
    let command_tx = tx.clone();
    {
        let _owner_commit = state.owner_commit.lock().await;
        state.current_session_id.store(session_id, Ordering::SeqCst);
        state.ws_sender.install(session_id, command_tx.clone());
        *state.connection_status.write().await = "disconnected".to_string();
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
        .await;
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
        .map_err(|e| e.to_string())?
        .join("sync_logs");
    if !log_dir.exists() {
        return Ok(Vec::new());
    }

    let mut entries = Vec::new();
    let mut read_dir = tokio::fs::read_dir(&log_dir)
        .await
        .map_err(|e| e.to_string())?;
    while let Ok(Some(entry)) = read_dir.next_entry().await {
        let metadata = entry.metadata().await.map_err(|e| e.to_string())?;
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
        .map_err(|e| e.to_string())?
        .join("sync_logs");
    let file_path = log_dir.join(&filename);

    // 安全检查：确保文件在 sync_logs 目录内
    let canonical_dir = log_dir.canonicalize().map_err(|e| e.to_string())?;
    let canonical_file = file_path.canonicalize().map_err(|e| e.to_string())?;
    if !canonical_file.starts_with(&canonical_dir) {
        return Err("Invalid file path".to_string());
    }

    let content = tokio::fs::read_to_string(&canonical_file)
        .await
        .map_err(|e| e.to_string())?;
    Ok(content)
}

#[tauri::command]
pub async fn clear_old_sync_logs(app: AppHandle, keep_days: u32) -> Result<u32, String> {
    let log_dir = app
        .path()
        .app_log_dir()
        .map_err(|e| e.to_string())?
        .join("sync_logs");
    if !log_dir.exists() {
        return Ok(0);
    }

    let cutoff =
        std::time::SystemTime::now() - std::time::Duration::from_secs(keep_days as u64 * 86400);
    let mut removed = 0u32;

    let mut read_dir = tokio::fs::read_dir(&log_dir)
        .await
        .map_err(|e| e.to_string())?;
    while let Ok(Some(entry)) = read_dir.next_entry().await {
        let metadata = entry.metadata().await.map_err(|e| e.to_string())?;
        if metadata.is_file() {
            let modified = metadata
                .modified()
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            if modified < cutoff {
                let _ = tokio::fs::remove_file(entry.path()).await;
                removed += 1;
            }
        }
    }

    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use tokio_tungstenite::tungstenite::error::Error as WsError;
    use tokio_tungstenite::tungstenite::http::{Response, StatusCode};

    #[test]
    fn protocol_send_failure_names_frame_and_transport_error() {
        assert_eq!(
            protocol_send_failure_message("owner metadata manifest", "socket closed"),
            "Failed to send owner metadata manifest: socket closed"
        );
    }

    #[test]
    fn protocol_1_1_version_ack_is_strict_and_uses_public_field_names() {
        let ack = parse_version_ack(&json!({
            "type": "VERSION_ACK",
            "pluginVersion": "1.1.0",
            "protocolVersion": "1.1",
        }))
        .expect("strict 1.1 acknowledgement");
        assert_eq!(ack.plugin_version, "1.1.0");
        assert_eq!(ack.protocol_version, "1.1");

        assert!(parse_version_ack(&json!({
            "type": "VERSION_ACK",
            "version": "1.1.0",
        }))
        .is_err());
        assert!(parse_version_ack(&json!({
            "type": "VERSION_ACK",
            "pluginVersion": "1.1.0",
            "protocolVersion": 1.1,
        }))
        .is_err());
    }

    #[test]
    fn changed_topic_list_rejects_wrong_types_empty_ids_and_duplicates() {
        assert_eq!(
            parse_unique_nonempty_strings(&json!(["topic-a", "topic-b"]), "changedTopics")
                .expect("valid topic list"),
            vec!["topic-a".to_string(), "topic-b".to_string()]
        );
        assert!(parse_unique_nonempty_strings(&json!("topic-a"), "changedTopics").is_err());
        assert!(parse_unique_nonempty_strings(&json!([""]), "changedTopics").is_err());
        assert!(
            parse_unique_nonempty_strings(&json!(["topic-a", "topic-a"]), "changedTopics").is_err()
        );
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
                message,
            }) => {
                assert_eq!(attempt_id, 7);
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

    #[tokio::test]
    async fn test_diagnose_unauthorized_token() {
        // Create an HTTP Error with status 401
        let response = Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .body(None)
            .unwrap();
        let err = WsError::Http(response);

        let diagnosis = diagnose_connection_failure(
            "ws://192.168.1.100:3000/ws-sync",
            "http://192.168.1.100:3000",
            &err,
        )
        .await;

        assert_eq!(diagnosis.error_code, "TOKEN_MISMATCH");
        assert!(diagnosis.error_message.contains("身份认证失败"));
        assert!(diagnosis.solution.contains("同步令牌"));
    }

    #[tokio::test]
    async fn test_diagnose_not_found_path() {
        // Create an HTTP Error with status 404
        let response = Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(None)
            .unwrap();
        let err = WsError::Http(response);

        let diagnosis = diagnose_connection_failure(
            "ws://192.168.1.100:3000/ws-sync",
            "http://192.168.1.100:3000",
            &err,
        )
        .await;

        assert_eq!(diagnosis.error_code, "WS_PATH_INVALID");
        assert!(diagnosis.error_message.contains("路径不存在"));
    }

    #[tokio::test]
    async fn test_diagnose_connection_refused() {
        // Simulate a connection refused error on localhost on port 1.
        let io_err =
            std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "connection refused");
        let err = WsError::Io(io_err);

        let diagnosis =
            diagnose_connection_failure("ws://127.0.0.1:1/ws-sync", "http://127.0.0.1:1", &err)
                .await;

        assert!(
            diagnosis.error_code == "CONNECTION_REFUSED"
                || diagnosis.error_code == "NETWORK_TIMEOUT"
        );
        assert!(
            diagnosis.error_message.contains("连接被拒绝")
                || diagnosis.error_message.contains("连接超时")
        );
        assert!(diagnosis.solution.contains("启动") || diagnosis.solution.contains("同一个 WiFi"));
    }

    #[tokio::test]
    async fn test_diagnose_network_unreachable() {
        // Simulate an unreachable address error.
        let io_err = std::io::Error::new(
            std::io::ErrorKind::AddrNotAvailable,
            "address not available",
        );
        let err = WsError::Io(io_err);

        let diagnosis = diagnose_connection_failure(
            "ws://non-existent-domain-vcp-test.xyz/ws-sync",
            "http://non-existent-domain-vcp-test.xyz",
            &err,
        )
        .await;

        assert_eq!(diagnosis.error_code, "NETWORK_UNREACHABLE");
        assert!(
            diagnosis.error_message.contains("网络不可达")
                || diagnosis.error_message.contains("地址无效")
        );
        assert!(diagnosis.solution.contains("网络状态"));
    }
}

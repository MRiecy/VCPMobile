use crate::vcp_modules::media_processor::convert_local_image_for_multimodal;
use dashmap::mapref::entry::Entry;
use dashmap::DashMap;
use futures_util::StreamExt;
#[cfg(not(target_os = "android"))]
use futures_util::TryStreamExt;
use reqwest::header::{AUTHORIZATION, CONTENT_LENGTH, CONTENT_TYPE};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use std::sync::Arc;
use std::time::Duration;
use tauri::{ipc::Channel, AppHandle, Manager, Runtime};
use tokio::sync::{futures::OwnedNotified, Notify};
use tokio_util::codec::FramedRead;
#[cfg(target_os = "android")]
use tokio_util::codec::LengthDelimitedCodec;
#[cfg(not(target_os = "android"))]
use tokio_util::io::StreamReader;
use tokio_util::sync::CancellationToken;

use super::vcp_http_endpoint::{
    resolve_chat_endpoint, resolve_model_discovery_endpoint, ChatRequestPurpose,
};
use crate::vcp_modules::aurora_pipeline::{AuroraBuffer, AuroraUpdate, AuroraUpdateKind};
use crate::vcp_modules::content_parser::ContentBlock;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::settings_manager::{ChatEndpointMode, Settings};
use crate::vcp_modules::topic_types::{MessageKey, TopicKey};

mod request_body;
use request_body::PreparedBody;

#[cfg(target_os = "android")]
const HELPER_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
#[cfg(target_os = "android")]
const HELPER_IO_TIMEOUT: Duration = Duration::from_secs(3);

#[cfg(target_os = "android")]
fn helper_frame_codec() -> LengthDelimitedCodec {
    LengthDelimitedCodec::builder()
        .max_frame_length(10 * 1024 * 1024)
        .new_codec()
}

#[cfg(target_os = "android")]
async fn connect_helper_port(port: u16) -> Result<tokio::net::TcpStream, String> {
    connect_helper_port_with_timeout(port, HELPER_CONNECT_TIMEOUT).await
}

#[cfg(target_os = "android")]
async fn connect_helper_port_with_timeout(
    port: u16,
    timeout: Duration,
) -> Result<tokio::net::TcpStream, String> {
    tokio::time::timeout(
        timeout,
        tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port)),
    )
    .await
    .map_err(|_| format!("Helper connect timed out on port {}", port))?
    .map_err(|e| format!("Helper connect failed on port {}: {}", port, e))
}

/// =================================================================
/// vcp_modules/vcp_client.rs - 统一的 VCP 请求处理模块 (Rust 重写版)
/// =================================================================
/// 该模块对应原项目的 modules/vcpClient.js，负责处理所有与 VCP 服务器的通信。
/// 包含动态路由、上下文注入（音乐、UI 规范）、流式 SSE 解析以及请求中止机制。
/// 请求参数结构体
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VcpRequestPayload {
    #[serde(default)]
    pub vcp_url: String, // turn 起点已冻结的最终 Chat 端点
    #[serde(default)]
    pub vcp_api_key: String, // API密钥
    pub messages: Vec<Value>,   // 消息数组
    pub model_config: Value,    // 模型配置 (包含 model, stream, temperature 等)
    pub message_id: String,     // 消息ID (用于跟踪和中止)
    pub context: Option<Value>, // 上下文信息 (agentId, topicId等)
    /// 每个模型 step 的内部网络/helper 身份；不进入 StreamEvent 或 DB 可见身份。
    #[serde(default)]
    pub transport_request_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ChatConnectionSnapshot {
    pub endpoint_url: String,
    pub api_key: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatEndpointPreview {
    pub final_url: String,
    pub model_discovery_url: Option<String>,
}

pub fn freeze_chat_connection(
    settings: &Settings,
    purpose: ChatRequestPurpose,
) -> Result<ChatConnectionSnapshot, String> {
    if settings.server.url.is_empty() {
        return Err("VCP Server URL is not configured.".to_string());
    }
    Ok(ChatConnectionSnapshot {
        endpoint_url: resolve_chat_endpoint(
            &settings.server.url,
            settings.server.endpoint_mode,
            purpose,
        )?,
        api_key: settings.server.api_key.clone(),
    })
}

#[tauri::command]
pub fn preview_chat_endpoint(
    vcp_url: String,
    chat_endpoint_mode: ChatEndpointMode,
) -> Result<ChatEndpointPreview, String> {
    Ok(ChatEndpointPreview {
        final_url: resolve_chat_endpoint(
            &vcp_url,
            chat_endpoint_mode,
            ChatRequestPurpose::Interactive,
        )?,
        model_discovery_url: resolve_model_discovery_endpoint(&vcp_url, chat_endpoint_mode)?,
    })
}

impl VcpRequestPayload {
    /// 内部 transport 未显式分配时兼容既有单 step 请求身份。
    pub fn effective_transport_request_id(&self) -> &str {
        self.transport_request_id
            .as_deref()
            .filter(|request_id| !request_id.trim().is_empty())
            .unwrap_or(&self.message_id)
    }
}

pub fn message_transport_request_id(key: &MessageKey) -> String {
    let identity = serde_json::json!([
        &key.topic.owner_type,
        &key.topic.owner_id,
        &key.topic.topic_id,
        &key.msg_id,
    ])
    .to_string();
    crate::vcp_modules::infra::utils::calculate_sha256(identity.as_bytes())
}

/// 流式事件结构体，用于向前端发送数据
#[derive(Debug, Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct StreamEvent {
    pub r#type: String, // "end" 是唯一 durable 终态；"error" 表示交付/提交未完成、仍可恢复
    pub chunk: Option<Value>, // 数据块 (仅 type="data" 时有效)
    pub message_id: String, // 消息ID
    pub context: Option<Value>, // 透传的上下文信息
    pub finish_reason: Option<String>, // 结束原因
    pub error: Option<String>, // 错误信息 (仅 type="error" 时有效)
    pub content: Option<String>, // durable end 的权威正文，或未提交错误时已完整接收的正文前缀
    pub aurora: Option<AuroraUpdate>, // Aurora 语义沉淀更新 (type="aurora" 时有效)
    pub blocks: Option<Vec<ContentBlock>>, // 持久化后的预渲染块 (仅 type="end" 时有效)
    pub timestamp: Option<u64>, // ⚡ 新增物理落笔时间戳
    pub topic_updated_at: Option<i64>, // durable message bubble 后的话题列表权威时间
    /// Orders delivery consumers independently of the Aurora AST frame sequence.
    pub consumer_revision: u64,
}

impl StreamEvent {
    pub fn thinking(message_id: String, context: Option<Value>) -> Self {
        Self {
            r#type: "thinking".into(),
            message_id,
            context,
            ..Default::default()
        }
    }

    pub fn aurora(message_id: String, aurora: AuroraUpdate, context: Option<Value>) -> Self {
        Self {
            r#type: "aurora".into(),
            aurora: Some(aurora),
            message_id,
            context,
            ..Default::default()
        }
    }

    pub fn end(
        message_id: String,
        context: Option<Value>,
        finish_reason: Option<String>,
        content: Option<String>,
        blocks: Option<Vec<ContentBlock>>,
        timestamp: Option<u64>,
        topic_updated_at: Option<i64>,
    ) -> Self {
        Self {
            r#type: "end".into(),
            message_id,
            context,
            finish_reason,
            content,
            blocks,
            timestamp,
            topic_updated_at,
            ..Default::default()
        }
    }

    pub fn error(message_id: String, context: Option<Value>, error: String) -> Self {
        Self {
            r#type: "error".into(),
            message_id,
            context,
            finish_reason: Some("error".to_string()),
            error: Some(error),
            ..Default::default()
        }
    }
}

/// Transport-owned text is moved into the durable finalizer. Internal callers
/// consume these fields directly instead of serializing a second JSON response.
#[derive(Debug)]
pub struct VcpResponse {
    pub full_content: String,
    pub finish_reason: Option<String>,
    pub is_aborted: bool,
}

impl VcpResponse {
    pub(crate) fn cancelled() -> Self {
        Self {
            full_content: String::new(),
            finish_reason: Some("cancelled_by_user".to_string()),
            is_aborted: true,
        }
    }
}

/// VCP 请求失败的内部类型。流式路径显式携带已经接收的完整 partial，
/// 防止上层只能从尚未写入正文的 pending 数据库行猜测内容。
#[derive(Debug, Clone)]
pub struct VcpRequestFailure {
    message: String,
    partial_content: Option<String>,
    recovery_pending: bool,
    stop_pending: bool,
    preparation_failure: bool,
}

impl VcpRequestFailure {
    fn streaming(message: impl Into<String>, partial_content: String) -> Self {
        Self {
            message: message.into(),
            partial_content: Some(partial_content),
            recovery_pending: false,
            stop_pending: false,
            preparation_failure: false,
        }
    }

    #[cfg(any(target_os = "android", test))]
    fn recoverable_helper(message: impl Into<String>, partial_content: Option<String>) -> Self {
        Self {
            message: message.into(),
            partial_content,
            recovery_pending: true,
            stop_pending: false,
            preparation_failure: false,
        }
    }

    pub(crate) fn is_recovery_pending(&self) -> bool {
        self.recovery_pending
    }

    pub(crate) fn is_stop_pending(&self) -> bool {
        self.stop_pending
    }

    fn preparation(message: String) -> Self {
        Self {
            preparation_failure: true,
            ..Self::from(format!(
                "附件准备失败，本次请求未发送。原消息与附件仍保留，可重试或移除问题附件。\n{message}"
            ))
        }
    }

    pub(crate) fn is_preparation_failure(&self) -> bool {
        self.preparation_failure
    }

    #[cfg(target_os = "android")]
    fn stopping(message: impl Into<String>, partial_content: Option<String>) -> Self {
        Self {
            message: message.into(),
            partial_content,
            recovery_pending: true,
            stop_pending: true,
            preparation_failure: false,
        }
    }

    pub fn into_parts(self) -> (String, Option<String>) {
        (self.message, self.partial_content)
    }
}

impl std::fmt::Display for VcpRequestFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for VcpRequestFailure {}

impl From<String> for VcpRequestFailure {
    fn from(message: String) -> Self {
        Self {
            message,
            partial_content: None,
            recovery_pending: false,
            stop_pending: false,
            preparation_failure: false,
        }
    }
}

const AURORA_PARSE_INTERVAL: Duration = Duration::from_millis(33);

pub(crate) fn stream_error_content(partial_content: &str, error: &str) -> String {
    let suffix = format!("\n\n> VCP流式错误: {error}");
    if partial_content.ends_with(&suffix) {
        partial_content.to_string()
    } else if partial_content.is_empty() {
        suffix
    } else {
        format!("{partial_content}{suffix}")
    }
}

/// 单次请求注册。`attempt_id` 使迟到任务无法删除同 message id 的新请求。
pub struct ActiveRequestEntry {
    attempt_id: uuid::Uuid,
    cancellation_token: CancellationToken,
    output: Arc<RequestOutput>,
    released: CancellationToken,
}

type ChatEventFeed = Arc<dyn Fn(&StreamEvent) + Send + Sync>;
type ChatListenerLookup = Arc<dyn Fn(&str) -> bool + Send + Sync>;

#[derive(Default)]
struct RequestOutputState {
    channel: Option<Channel<StreamEvent>>,
    snapshot_pending: bool,
    closed: bool,
    consumer_revision: u64,
    owner_released: bool,
    origin_user_message_id: Option<String>,
    chat_feed: Option<ChatEventFeed>,
    has_chat_listener: Option<ChatListenerLookup>,
    pending_terminal: Option<Arc<crate::vcp_modules::chat::message_service::PendingStreamTerminal>>,
    discarded: bool,
}

static NEXT_CONSUMER_REVISION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn next_consumer_revision() -> u64 {
    NEXT_CONSUMER_REVISION.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// AuroraBuffer owns live content. Once a transport without a helper finishes,
/// this owner retains its terminal result until SQLite accepts it or history is
/// explicitly discarded. A failed commit must not become a lost generation.
#[derive(Default)]
struct RequestOutput {
    state: std::sync::Mutex<RequestOutputState>,
    changed: tokio::sync::Notify,
    #[cfg(target_os = "android")]
    helper_handoff: std::sync::Mutex<Option<HelperHandoff>>,
}

/// Keep the consumer socket alive through the database transaction. Dropping it
/// before commit would turn every online result into an offline disk write.
#[cfg(target_os = "android")]
struct HelperHandoff {
    transport_request_id: String,
    generation: String,
    _stream: Option<tokio::net::TcpStream>,
    completed_file: Option<std::path::PathBuf>,
}

impl RequestOutput {
    fn pending_terminal(
        &self,
    ) -> Option<Arc<crate::vcp_modules::chat::message_service::PendingStreamTerminal>> {
        self.state.lock().ok()?.pending_terminal.clone()
    }

    fn settle_terminal(&self, discarded: bool) {
        if let Ok(mut state) = self.state.lock() {
            state.pending_terminal = None;
            state.discarded |= discarded;
        }
    }

    fn reopen_for_terminal_retry(&self) -> Result<(), String> {
        let mut state = self.state.lock().map_err(|error| error.to_string())?;
        state.closed = state.discarded;
        state.owner_released = false;
        state.channel = None;
        Ok(())
    }

    fn bind_chat_feed<R: Runtime>(&self, app: &AppHandle<R>) {
        if let Ok(mut state) = self.state.lock() {
            if state.chat_feed.is_none() {
                let feed_app = app.clone();
                let check_app = app.clone();
                state.chat_feed = Some(Arc::new(move |event| {
                    crate::vcp_modules::chat::voice_service::feed_chat_event(&feed_app, event)
                }));
                state.has_chat_listener = Some(Arc::new(move |origin| {
                    crate::vcp_modules::chat::voice_service::has_chat_listener(&check_app, origin)
                }));
            }
        }
    }

    fn send_activity(&self, key: &MessageKey, request_id: uuid::Uuid, active: bool) -> bool {
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        if state.owner_released {
            return false;
        }
        if !active {
            state.owner_released = true;
        }
        // Owner metadata survives a content end/error so the lease can always
        // release its preparation/busy indicator after durable settlement.
        Self::send_locked(
            &state,
            StreamEvent {
                r#type: "request_activity".to_string(),
                context: Some(json!({
                    "ownerType": key.topic.owner_type, "ownerId": key.topic.owner_id,
                    "topicId": key.topic.topic_id, "requestId": request_id,
                })),
                chunk: Some(json!({ "active": active })),
                ..Default::default()
            },
        )
    }
    #[cfg(target_os = "android")]
    fn retain_helper_handoff(
        &self,
        transport_request_id: String,
        generation: String,
        stream: Option<tokio::net::TcpStream>,
    ) {
        if let Ok(mut handoff) = self.helper_handoff.lock() {
            *handoff = Some(HelperHandoff {
                transport_request_id,
                generation,
                _stream: stream,
                completed_file: None,
            });
        }
    }

    #[cfg(target_os = "android")]
    fn retain_completed_file(&self, file: &OfflineCompletedFile) {
        if let Ok(mut handoff) = self.helper_handoff.lock() {
            *handoff = Some(HelperHandoff {
                transport_request_id: file.metadata.request_id.clone(),
                generation: file.metadata.generation.clone(),
                _stream: None,
                completed_file: Some(file.path.clone()),
            });
        }
    }

    fn bind_initial(&self, channel: Option<Channel<StreamEvent>>) {
        if let Ok(mut state) = self.state.lock() {
            if state.channel.is_none() && !state.closed {
                state.channel = channel;
                state.consumer_revision = next_consumer_revision();
            }
        }
    }

    fn attach(&self, channel: Channel<StreamEvent>) -> Result<bool, String> {
        self.attach_connection(channel, false)
    }

    fn attach_connection(
        &self,
        channel: Channel<StreamEvent>,
        replace_connection: bool,
    ) -> Result<bool, String> {
        let mut state = self.state.lock().map_err(|error| error.to_string())?;
        if state.closed || state.owner_released {
            return Ok(false);
        }
        if !replace_connection
            && state
                .channel
                .as_ref()
                .is_some_and(|current| current.id() == channel.id())
        {
            return Ok(true);
        }
        state.channel = Some(channel);
        state.snapshot_pending = true;
        state.consumer_revision = next_consumer_revision();
        drop(state);
        // One producer waits here; notify_one retains a permit if it is between
        // awaits. The flag also covers replacement while it sends an update.
        self.changed.notify_one();
        Ok(true)
    }

    fn send(&self, event: StreamEvent) -> bool {
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        if state.closed {
            return false;
        }
        // end follows the durable commit; error closes this delivery attempt
        // after a failed commit. Neither allows a later attach to miss its result.
        if event.r#type == "end" || event.r#type == "error" {
            state.closed = true;
        }
        let delivered = Self::send_locked(&state, event);
        if !delivered {
            state.channel = None;
            state.snapshot_pending = true;
        }
        delivered
    }

    fn send_locked(state: &RequestOutputState, mut event: StreamEvent) -> bool {
        event.consumer_revision = state.consumer_revision;
        if let Some(origin) = &state.origin_user_message_id {
            let context = event.context.get_or_insert_with(|| json!({}));
            if let Some(context) = context.as_object_mut() {
                context.insert("originUserMessageId".into(), json!(origin));
            }
        }
        if let Some(feed) = &state.chat_feed {
            feed(&event);
        }
        match &state.channel {
            Some(channel) => channel
                .send(event)
                .map_err(|error| {
                    log::warn!("[VCPClient] Stream consumer delivery failed: {error}");
                })
                .is_ok(),
            None => true,
        }
    }

    fn send_aurora(
        &self,
        buffer: &mut AuroraBuffer,
        message_id: &str,
        context: &Option<Value>,
        finish_reason: Option<String>,
        force_snapshot: bool,
    ) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        if state.closed {
            return;
        }
        let feeds_tts = state.origin_user_message_id.as_ref().is_some_and(|origin| {
            state
                .has_chat_listener
                .as_ref()
                .is_some_and(|listener| listener(origin))
        });
        if state.channel.is_none() && !feeds_tts {
            buffer.discard_pending_delivery();
            state.snapshot_pending = true;
            return;
        }
        let prepared = if state.snapshot_pending || force_snapshot {
            Some(buffer.prepare_snapshot_update())
        } else {
            buffer.prepare_delta_update()
        };
        let Some((update, commit)) = prepared else {
            return;
        };
        let mut event = StreamEvent::aurora(message_id.to_string(), update, context.clone());
        event.finish_reason = finish_reason;
        if Self::send_locked(&state, event) {
            buffer.commit_delivery(commit);
            state.snapshot_pending = false;
        } else {
            state.channel = None;
            state.snapshot_pending = true;
            buffer.discard_pending_delivery();
        }
    }

    fn close(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.closed = true;
            state.channel = None;
            state.chat_feed = None;
            state.has_chat_listener = None;
        }
    }
}

fn request_output<R: Runtime>(
    app: &AppHandle<R>,
    key: &MessageKey,
    channel: Option<Channel<StreamEvent>>,
) -> Arc<RequestOutput> {
    // A reconnect can finish discovery after another WebView has connected.
    // New owners always take the current consumer, never that stale caller's.
    let requests = app.try_state::<ActiveRequests>();
    let connection = requests
        .as_ref()
        .and_then(|requests| requests.1.lock().ok());
    let channel = connection
        .as_ref()
        .and_then(|connection| {
            connection
                .as_ref()
                .map(|connection| connection.channel.clone())
        })
        .or(channel);
    let output = app
        .try_state::<ActiveRequests>()
        .and_then(|requests| requests.0.get(key).map(|entry| entry.output.clone()))
        .unwrap_or_default();
    output.bind_chat_feed(app);
    let group = app
        .try_state::<ActiveGroupTurns>()
        .and_then(|turns| turns.0.get(&key.topic).map(|entry| entry.value().clone()));
    if let Some(group) = group {
        if let Ok(mut current) = group.channel.lock() {
            if current.is_none() {
                *current = channel;
            }
            // Group rebinding and a new speaker's initial binding share this lock.
            output.bind_initial(current.clone());
        }
    } else {
        output.bind_initial(channel);
    }
    output
}

pub(crate) fn deliver_request_event<R: Runtime>(
    app: &AppHandle<R>,
    key: &MessageKey,
    channel: Option<Channel<StreamEvent>>,
    event: StreamEvent,
) {
    request_output(app, key, channel).send(event);
}

pub(crate) fn retain_request_terminal<R: Runtime>(
    app: &AppHandle<R>,
    key: &MessageKey,
    terminal: Arc<crate::vcp_modules::chat::message_service::PendingStreamTerminal>,
) {
    let Some(output) = app
        .try_state::<ActiveRequests>()
        .and_then(|requests| requests.0.get(key).map(|entry| entry.output.clone()))
    else {
        return;
    };
    // Android already retains these results through its helper handoff. Do not
    // keep a second resident owner beside a live helper or its completed file.
    #[cfg(target_os = "android")]
    if output
        .helper_handoff
        .lock()
        .is_ok_and(|handoff| handoff.is_some())
    {
        return;
    }
    if let Ok(mut state) = output.state.lock() {
        if !state.discarded && state.pending_terminal.is_none() {
            state.pending_terminal = Some(terminal);
        }
    };
}

/// Only the durable owner can acknowledge a helper result. Delivery to a
/// WebView is deliberately unrelated to this resource handoff.
pub(crate) async fn release_committed_helper<R: Runtime>(app: &AppHandle<R>, key: &MessageKey) {
    settle_helper_handoff(app, key, true).await;
}

pub(crate) async fn discard_request_helper<R: Runtime>(app: &AppHandle<R>, key: &MessageKey) {
    settle_helper_handoff(app, key, false).await;
}

/// Called after a deletion transaction commits, including when the main
/// process has restarted and no local request lease exists anymore.
pub(crate) async fn discard_deleted_generation<R: Runtime>(app: &AppHandle<R>, key: &MessageKey) {
    if let Some(requests) = app.try_state::<ActiveRequests>() {
        if let Some(request) = requests.0.get(key).map(|entry| entry.value().clone()) {
            request.output.settle_terminal(true);
            request.output.send_activity(key, request.attempt_id, false);
            request.output.close();
            let _ = requests.cancel(key);
            if request.released.is_cancelled() {
                requests
                    .0
                    .remove_if(key, |_, current| current.attempt_id == request.attempt_id);
            }
        }
    }
    #[cfg(target_os = "android")]
    {
        let request_id = message_transport_request_id(key);
        // Query only the existing helper; deleting history must not start a
        // foreground service. The UUID prevents a late control affecting a
        // different helper generation.
        let generation = query_helper_generation(app, &request_id).await.ok();
        let Some(db) = app.try_state::<DbState>() else {
            return;
        };
        let pending = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM active_generations
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?)",
        )
        .bind(&key.topic.owner_type)
        .bind(&key.topic.owner_id)
        .bind(&key.topic.topic_id)
        .bind(&key.msg_id)
        .fetch_one(&db.pool)
        .await;
        if !matches!(pending, Ok(false)) {
            return;
        }
        discard_request_helper(app, key).await;
        if let Some(generation) = generation {
            if let Err(error) = discard_helper_session(app, &request_id, &generation).await {
                log::warn!("[VCPClient] Deleted generation helper cleanup failed: {error}");
            }
        }
        let Ok(cache) = app.path().app_cache_dir() else {
            return;
        };
        let prefix = format!(
            "{}.",
            crate::vcp_modules::infra::utils::calculate_sha256(request_id.as_bytes())
        );
        if let Ok(mut entries) = tokio::fs::read_dir(cache.join("sse_offline_completed")).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.starts_with(&prefix) && name.ends_with(".completed") {
                    if let Err(error) = tokio::fs::remove_file(entry.path()).await {
                        if error.kind() != std::io::ErrorKind::NotFound {
                            log::warn!(
                                "[VCPClient] Deleted generation file cleanup failed: {error}"
                            );
                        }
                    }
                }
            }
        }
    }
}

async fn settle_helper_handoff<R: Runtime>(app: &AppHandle<R>, key: &MessageKey, committed: bool) {
    let output = app
        .try_state::<ActiveRequests>()
        .and_then(|requests| requests.0.get(key).map(|entry| entry.output.clone()));
    if let Some(output) = &output {
        output.settle_terminal(!committed);
    }
    #[cfg(target_os = "android")]
    {
        let handoff = output.and_then(|output| {
            output
                .helper_handoff
                .lock()
                .ok()
                .and_then(|mut handoff| handoff.take())
        });
        if let Some(handoff) = handoff {
            let result = if committed {
                ack_helper_committed(app, &handoff.transport_request_id, &handoff.generation).await
            } else {
                discard_helper_session(app, &handoff.transport_request_id, &handoff.generation)
                    .await
            };
            if let Err(error) = result {
                // The database result remains authoritative. Dropping the
                // socket lets the helper publish an orphan for later cleanup.
                log::warn!("[VCPClient] Helper terminal handoff failed: {error}");
            }
            if let Some(path) = handoff.completed_file {
                if let Err(error) = remove_offline_completed(&path).await {
                    log::warn!("[VCPClient] Settled helper file cleanup failed: {error}");
                }
            }
        }
    }
    #[cfg(not(target_os = "android"))]
    let _ = (app, key, committed);
}

pub type ActiveRequestMap = Arc<DashMap<MessageKey, Arc<ActiveRequestEntry>>>;

/// 全局活跃请求管理器。相同 message id 只允许一个 live attempt。
pub struct ActiveRequests(
    pub ActiveRequestMap,
    std::sync::Mutex<Option<ChatOutputConnection>>,
);

struct ChatOutputConnection {
    id: String,
    channel: Channel<StreamEvent>,
}

impl Default for ActiveRequests {
    fn default() -> Self {
        log::info!("[VCPClient] Initialized ActiveRequests successfully.");
        Self(Arc::new(DashMap::new()), std::sync::Mutex::new(None))
    }
}

impl ActiveRequests {
    pub(crate) fn cancel(&self, key: &MessageKey) -> Result<bool, String> {
        let Some(entry) = self.0.get(key) else {
            return Ok(false);
        };
        entry.cancellation_token.cancel();
        drop(entry);
        Ok(true)
    }
}

/// RAII lease：仅当 map 中仍是本 attempt 时才移除，避免 ABA。
pub struct ActiveRequestLease {
    requests: ActiveRequestMap,
    key: MessageKey,
    entry: Arc<ActiveRequestEntry>,
}

impl ActiveRequestLease {
    pub(crate) fn set_origin_user_message(&self, id: &str) -> Result<(), String> {
        self.entry
            .output
            .state
            .lock()
            .map_err(|error| error.to_string())?
            .origin_user_message_id = Some(id.to_string());
        Ok(())
    }

    pub(crate) fn bind_output<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        channel: Option<Channel<StreamEvent>>,
    ) -> Result<(), String> {
        let output = request_output(app, &self.key, channel);
        output.send_activity(&self.key, self.entry.attempt_id, true);
        Ok(())
    }

    #[cfg(test)]
    pub fn try_acquire(
        requests: ActiveRequestMap,
        key: MessageKey,
    ) -> Result<(Self, CancellationToken), String> {
        Self::try_acquire_with_cancellation(requests, key, CancellationToken::new())
    }

    pub(crate) fn try_acquire_with_cancellation(
        requests: ActiveRequestMap,
        key: MessageKey,
        cancellation_token: CancellationToken,
    ) -> Result<(Self, CancellationToken), String> {
        Self::try_acquire_inner(requests, key, cancellation_token, false)
    }

    fn try_acquire_recovery(
        requests: ActiveRequestMap,
        key: MessageKey,
    ) -> Result<(Self, CancellationToken), String> {
        Self::try_acquire_inner(requests, key, CancellationToken::new(), true)
    }

    fn try_acquire_inner(
        requests: ActiveRequestMap,
        key: MessageKey,
        cancellation_token: CancellationToken,
        recovering: bool,
    ) -> Result<(Self, CancellationToken), String> {
        let attempt_id = uuid::Uuid::new_v4();
        let make_entry = |output| {
            Arc::new(ActiveRequestEntry {
                attempt_id,
                cancellation_token: cancellation_token.clone(),
                output,
                released: CancellationToken::new(),
            })
        };

        let entry = match requests.entry(key.clone()) {
            Entry::Vacant(slot) => {
                let entry = make_entry(Arc::default());
                slot.insert(entry.clone());
                entry
            }
            Entry::Occupied(mut slot) if recovering && slot.get().released.is_cancelled() => {
                // The producer finished, but its terminal result still belongs
                // to this output. Only one recovery may take the next lease.
                let output = slot.get().output.clone();
                output.reopen_for_terminal_retry()?;
                let entry = make_entry(output);
                slot.insert(entry.clone());
                entry
            }
            Entry::Occupied(_) => {
                return Err(format!(
                    "Request {} is already active for {}/{}/{}; duplicate attempt rejected",
                    key.msg_id, key.topic.owner_type, key.topic.owner_id, key.topic.topic_id
                ))
            }
        };
        Ok((
            Self {
                requests,
                key,
                entry,
            },
            cancellation_token,
        ))
    }
}

impl Drop for ActiveRequestLease {
    fn drop(&mut self) {
        self.entry
            .output
            .send_activity(&self.key, self.entry.attempt_id, false);
        self.entry.output.close();
        if let Entry::Occupied(entry) = self.requests.entry(self.key.clone()) {
            if entry.get().attempt_id == self.entry.attempt_id {
                // Publish release under the map guard so deletion cannot miss
                // both this cleanup and the retained-entry cleanup path.
                self.entry.released.cancel();
                if self.entry.output.pending_terminal().is_none() {
                    entry.remove();
                }
            }
        }
        self.entry.released.cancel();
    }
}

struct ActiveGroupTurnEntry {
    turn_id: uuid::Uuid,
    cancellation_token: CancellationToken,
    released: Arc<Notify>,
    channel: std::sync::Mutex<Option<Channel<StreamEvent>>>,
    origin_user_message_id: std::sync::Mutex<Option<String>>,
    chat_feed: std::sync::Mutex<Option<ChatEventFeed>>,
    session: std::sync::Mutex<Option<Arc<crate::vcp_modules::group::group_session::GroupSession>>>,
}

type ActiveGroupTurnMap = Arc<DashMap<TopicKey, Arc<ActiveGroupTurnEntry>>>;

/// 每个 Topic 同时只允许一个群组回合，并由同一所有者承载回合取消状态。
pub struct ActiveGroupTurns(
    ActiveGroupTurnMap,
    std::sync::Mutex<std::collections::VecDeque<(TopicKey, std::time::Instant)>>,
);

impl Default for ActiveGroupTurns {
    fn default() -> Self {
        log::info!("[VCPClient] Initialized ActiveGroupTurns successfully.");
        Self(
            Arc::new(DashMap::new()),
            std::sync::Mutex::new(std::collections::VecDeque::new()),
        )
    }
}

impl ActiveGroupTurns {
    pub(crate) fn accept_continue(&self, key: &TopicKey, debounce_ms: u32) -> Result<(), String> {
        let mut recent = self.1.lock().map_err(|e| e.to_string())?;
        recent.retain(|(_, time)| time.elapsed() < Duration::from_secs(10));
        if recent.iter().any(|(topic, time)| {
            topic == key && time.elapsed() < Duration::from_millis(debounce_ms as u64)
        }) {
            return Err("继续操作过于频繁，请稍后重试。".into());
        }
        recent.retain(|(topic, _)| topic != key);
        while recent.len() >= 128 {
            recent.pop_front();
        }
        recent.push_back((key.clone(), std::time::Instant::now()));
        Ok(())
    }

    pub(crate) fn try_acquire(&self, key: TopicKey) -> Result<ActiveGroupTurnLease, String> {
        self.try_acquire_or_wait(key.clone()).map_err(|_| {
            format!(
                "Group turn is already active for {}/{}/{}",
                key.owner_type, key.owner_id, key.topic_id
            )
        })
    }

    /// Atomically acquire the slot or subscribe to its current owner's release.
    pub(crate) fn try_acquire_or_wait(
        &self,
        key: TopicKey,
    ) -> Result<ActiveGroupTurnLease, OwnedNotified> {
        match self.0.entry(key.clone()) {
            Entry::Vacant(slot) => {
                let cancellation_token = CancellationToken::new();
                slot.insert(Arc::new(ActiveGroupTurnEntry {
                    turn_id: uuid::Uuid::new_v4(),
                    cancellation_token: cancellation_token.clone(),
                    released: Arc::new(Notify::new()),
                    channel: std::sync::Mutex::new(None),
                    origin_user_message_id: std::sync::Mutex::new(None),
                    chat_feed: std::sync::Mutex::new(None),
                    session: std::sync::Mutex::new(None),
                }));
                Ok(ActiveGroupTurnLease {
                    turns: self.0.clone(),
                    key,
                    cancellation_token,
                })
            }
            // Construct the future under the map guard. notify_waiters also
            // reaches futures created before release but first polled afterward.
            Entry::Occupied(slot) => Err(slot.get().released.clone().notified_owned()),
        }
    }

    pub(crate) fn cancel(&self, key: &TopicKey) -> bool {
        let Some(entry) = self.0.get(key) else {
            return false;
        };
        entry.cancellation_token.cancel();
        if let Ok(session) = entry.session.lock() {
            if let Some(session) = session.as_ref() {
                session.cancel_scopes();
                session.invalidate();
            }
        }
        let event = group_turn_event(key, &entry, true);
        if let Ok(channel) = entry.channel.lock() {
            if let Some(channel) = channel.as_ref() {
                let _ = channel.send(event);
            }
        }
        drop(entry);
        true
    }

    pub(crate) fn cancel_owner(&self, owner_type: &str, owner_id: &str) {
        let keys = self
            .0
            .iter()
            .filter(|entry| {
                entry.key().owner_type == owner_type && entry.key().owner_id == owner_id
            })
            .map(|entry| entry.key().clone())
            .collect::<Vec<_>>();
        for key in keys {
            self.cancel(&key);
        }
    }

    pub(crate) fn notify_history_changed(&self, key: &TopicKey) {
        if let Some(session) = self.session(key) {
            session.invalidate();
            self.publish(key);
        }
    }

    pub(crate) fn notify_group_changed(&self, group_id: &str) {
        let keys = self
            .0
            .iter()
            .filter(|entry| entry.key().owner_type == "group" && entry.key().owner_id == group_id)
            .map(|entry| entry.key().clone())
            .collect::<Vec<_>>();
        for key in keys {
            self.notify_history_changed(&key);
        }
    }

    pub(crate) fn notify_all_groups_changed(&self) {
        let keys = self
            .0
            .iter()
            .map(|entry| entry.key().clone())
            .collect::<Vec<_>>();
        for key in keys {
            self.notify_history_changed(&key);
        }
    }

    pub(crate) fn topic_for_session(
        &self,
        session: &Arc<crate::vcp_modules::group::group_session::GroupSession>,
    ) -> Option<TopicKey> {
        self.0.iter().find_map(|entry| {
            entry
                .session
                .lock()
                .ok()?
                .as_ref()
                .filter(|current| Arc::ptr_eq(current, session))
                .map(|_| entry.key().clone())
        })
    }

    pub(crate) fn session(
        &self,
        key: &TopicKey,
    ) -> Option<Arc<crate::vcp_modules::group::group_session::GroupSession>> {
        self.0
            .get(key)
            .and_then(|entry| entry.session.lock().ok()?.clone())
    }

    pub(crate) fn publish(&self, key: &TopicKey) {
        if let Some(entry) = self.0.get(key) {
            let event = group_turn_event(key, &entry, true);
            if let Ok(channel) = entry.channel.lock() {
                if let Some(channel) = channel.as_ref() {
                    let _ = channel.send(event);
                }
            }
        }
    }

    fn attach_output(
        &self,
        requests: &ActiveRequests,
        key: &TopicKey,
        channel: Channel<StreamEvent>,
    ) -> Result<Option<Value>, String> {
        let Some(turn) = self.0.get(key) else {
            return Ok(None);
        };
        let mut current = turn.channel.lock().map_err(|error| error.to_string())?;
        *current = Some(channel.clone());
        for request in requests
            .0
            .iter()
            .filter(|request| &request.key().topic == key)
        {
            request.output.attach(channel.clone())?;
        }
        let event = group_turn_event(key, &turn, true);
        if let Ok(feed) = turn.chat_feed.lock() {
            if let Some(feed) = feed.as_ref() {
                feed(&event);
            }
        }
        let _ = channel.send(event);
        let mut snapshot = json!({
            "ownerType": key.owner_type, "ownerId": key.owner_id,
            "topicId": key.topic_id, "turnId": turn.turn_id,
        });
        if let Ok(session) = turn.session.lock() {
            if let Some(session) = session.as_ref() {
                if let Ok(Value::Object(extra)) = session
                    .snapshot()
                    .and_then(|snapshot| serde_json::to_value(snapshot).map_err(|e| e.to_string()))
                {
                    if let Some(snapshot) = snapshot.as_object_mut() {
                        snapshot.extend(extra);
                    }
                }
            }
        }
        Ok(Some(snapshot))
    }
}

pub(crate) struct ActiveGroupTurnLease {
    turns: ActiveGroupTurnMap,
    key: TopicKey,
    cancellation_token: CancellationToken,
}

impl ActiveGroupTurnLease {
    pub(crate) fn start_session(
        &self,
        trigger: &'static str,
        force_continue: bool,
    ) -> Result<Arc<crate::vcp_modules::group::group_session::GroupSession>, String> {
        let turn = self.turns.get(&self.key).ok_or("群聊执行租约已经释放")?;
        let mut slot = turn.session.lock().map_err(|e| e.to_string())?;
        if slot.is_some() {
            return Err("群聊会话已经启动".into());
        }
        let session = crate::vcp_modules::group::group_session::GroupSession::new(
            turn.turn_id.to_string(),
            self.cancellation_token.clone(),
            trigger,
            force_continue,
        );
        *slot = Some(session.clone());
        Ok(session)
    }

    pub(crate) fn set_origin_user_message(&self, id: &str) -> Result<(), String> {
        if let Some(turn) = self.turns.get(&self.key) {
            *turn
                .origin_user_message_id
                .lock()
                .map_err(|error| error.to_string())? = Some(id.to_string());
        }
        Ok(())
    }

    pub(crate) fn origin_user_message(&self) -> Option<String> {
        self.turns
            .get(&self.key)
            .and_then(|turn| turn.origin_user_message_id.lock().ok()?.clone())
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancellation_token.is_cancelled()
    }

    pub(crate) fn child_token(&self) -> CancellationToken {
        self.cancellation_token.child_token()
    }

    pub(crate) fn cancel(&self) {
        self.cancellation_token.cancel();
    }

    pub(crate) fn bind_output<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        channel: Channel<StreamEvent>,
    ) -> Result<(), String> {
        self.bind_optional_output(app, Some(channel))
    }

    pub(crate) fn bind_optional_output<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        channel: Option<Channel<StreamEvent>>,
    ) -> Result<(), String> {
        let requests = app.state::<ActiveRequests>();
        let connection = requests.1.lock().map_err(|error| error.to_string())?;
        let channel = connection
            .as_ref()
            .map(|connection| connection.channel.clone())
            .or(channel);
        if let Some(turn) = self.turns.get(&self.key) {
            let feed_app = app.clone();
            *turn.chat_feed.lock().map_err(|error| error.to_string())? =
                Some(Arc::new(move |event| {
                    crate::vcp_modules::chat::voice_service::feed_chat_event(&feed_app, event)
                }));
            let mut current = turn.channel.lock().map_err(|error| error.to_string())?;
            *current = channel.clone();
            let event = group_turn_event(&self.key, &turn, true);
            if let Ok(feed) = turn.chat_feed.lock() {
                if let Some(feed) = feed.as_ref() {
                    feed(&event);
                }
            }
            if let Some(channel) = channel {
                let _ = channel.send(event);
            }
        }
        Ok(())
    }
}

impl Drop for ActiveGroupTurnLease {
    fn drop(&mut self) {
        if let Some((_, turn)) = self.turns.remove(&self.key) {
            if let Ok(session) = turn.session.lock() {
                if let Some(session) = session.as_ref() {
                    session.close(if self.cancellation_token.is_cancelled() {
                        Some("cancelled_by_user".into())
                    } else {
                        None
                    });
                }
            }
            let event = group_turn_event(&self.key, &turn, false);
            if let Ok(feed) = turn.chat_feed.lock() {
                if let Some(feed) = feed.as_ref() {
                    feed(&event);
                }
            }
            if let Ok(channel) = turn.channel.lock() {
                if let Some(channel) = channel.as_ref() {
                    let _ = channel.send(event);
                }
            }
            turn.released.notify_waiters();
        }
    }
}

fn group_turn_event(key: &TopicKey, turn: &ActiveGroupTurnEntry, active: bool) -> StreamEvent {
    let mut chunk = json!({ "active": active });
    if let Ok(session) = turn.session.lock() {
        if let Some(session) = session.as_ref() {
            if let Ok(Value::Object(extra)) = session
                .snapshot()
                .and_then(|snapshot| serde_json::to_value(snapshot).map_err(|e| e.to_string()))
            {
                if let Some(chunk) = chunk.as_object_mut() {
                    chunk.extend(extra);
                }
            }
        }
    }
    StreamEvent {
        r#type: "group_turn".to_string(),
        context: Some(json!({
            "ownerType": key.owner_type, "ownerId": key.owner_id,
            "topicId": key.topic_id, "turnId": turn.turn_id,
            "originUserMessageId": turn.origin_user_message_id.lock().ok().and_then(|id| id.clone()),
        })),
        chunk: Some(chunk),
        consumer_revision: next_consumer_revision(),
        ..Default::default()
    }
}

/// 中止群组的整个接力赛回合
#[tauri::command]
#[allow(non_snake_case)]
pub async fn interruptGroupTurn(
    app: AppHandle,
    state: tauri::State<'_, ActiveGroupTurns>,
    owner_id: String,
    owner_type: String,
    topic_id: String,
) -> Result<Value, String> {
    log::info!(
        "[VCPClient] interruptGroupTurn called for topicId: {}",
        topic_id
    );
    let key = TopicKey::new(owner_type, owner_id, topic_id);
    let origin = state
        .0
        .get(&key)
        .and_then(|turn| turn.origin_user_message_id.lock().ok()?.clone());
    if let Some(origin) = origin {
        let speech_app = app.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(error) = crate::vcp_modules::chat::voice_service::stop_chat_reply_by_origin(
                &speech_app,
                &origin,
            )
            .await
            {
                log::warn!("[VCPClient] Could not stop reply speech: {error}");
            }
        });
    }
    let db = app.state::<DbState>();
    let pending = sqlx::query_scalar::<_, String>(
        "UPDATE active_generations SET stop_requested = 1
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? RETURNING msg_id",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .fetch_all(&db.pool)
    .await
    .map_err(|error| error.to_string())?;
    let cancelled = state.cancel(&key);
    let requests = app.state::<ActiveRequests>();
    for message_id in &pending {
        schedule_pending_stop(&app, &requests, MessageKey::new(key.clone(), message_id));
    }
    Ok(
        json!({"status": if cancelled || !pending.is_empty() { "cancelling" } else { "not_running" }}),
    )
}

fn schedule_pending_stop<R: Runtime>(
    app: &AppHandle<R>,
    requests: &ActiveRequests,
    key: MessageKey,
) {
    let producer = requests
        .0
        .get(&key)
        .filter(|entry| !entry.released.is_cancelled())
        .map(|entry| entry.value().clone());
    if let Some(entry) = &producer {
        entry.cancellation_token.cancel();
    }
    let channel = requests
        .1
        .lock()
        .ok()
        .and_then(|connection| {
            connection
                .as_ref()
                .map(|connection| connection.channel.clone())
        })
        .unwrap_or_else(|| Channel::new(|_| Ok(())));
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Some(entry) = producer {
            entry.released.cancelled().await;
            // Successful cancellation or deletion already settled the owner.
            // Only a received terminal whose commit failed needs this retry.
            if entry.output.pending_terminal().is_none() {
                return;
            }
        }
        if let Err(error) = recover_active_generation(
            app.clone(),
            app.state::<ActiveRequests>(),
            key.topic.owner_id,
            key.topic.owner_type,
            key.topic.topic_id,
            key.msg_id,
            channel,
        )
        .await
        {
            log::warn!("[VCPClient] Stop remains pending: {error}");
        }
    });
}

/// 从正式应用服务冻结的请求上下文取得完整消息身份。
/// 请求入口负责以下步骤：
/// 1. 数据验证与规范化 (通过 Rust 类型系统自动处理部分)
/// 2. 按 turn 起点冻结的 typed route 选择普通 completions 或 /v1/chatvcp/completions
/// 3. 上下文注入 (音乐信息、UI 规范要求)
/// 4. 发起 HTTP 请求 (支持流式和非流式)
/// 5. 注册 AbortController 实现中止机制
fn message_key_from_context(
    context: &Option<Value>,
    message_id: &str,
) -> Result<MessageKey, String> {
    let context = context
        .as_ref()
        .ok_or_else(|| "VCP request context is required".to_string())?;
    let topic_id = context["topicId"]
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "VCP request topicId is required".to_string())?;
    let owner_id = context["ownerId"]
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "VCP request ownerId is required".to_string())?;
    let owner_type = context["ownerType"]
        .as_str()
        .filter(|value| *value == "agent" || *value == "group")
        .ok_or_else(|| "VCP request ownerType is invalid".to_string())?;
    Ok(MessageKey::new(
        TopicKey::new(owner_type, owner_id, topic_id),
        message_id,
    ))
}

fn extract_text_for_hash(content: &Value) -> String {
    if let Some(s) = content.as_str() {
        return s.to_string();
    }
    if let Some(arr) = content.as_array() {
        let text_parts: Vec<String> = arr
            .iter()
            .filter(|part| part["type"].as_str() == Some("text"))
            .filter_map(|part| part["text"].as_str())
            .map(|s| s.to_string())
            .collect();
        return text_parts.join("\n");
    }
    if let Some(obj) = content.as_object() {
        if let Some(s) = obj.get("text").and_then(|t| t.as_str()) {
            return s.to_string();
        }
    }
    String::new()
}

fn get_or_calculate_message_hash(content: &Value) -> String {
    use crate::vcp_modules::infra::utils::calculate_sha256;

    let text = extract_text_for_hash(content);
    let hash = calculate_sha256(text.as_bytes());
    format!("sha256:{}", hash)
}

/// 执行一个已经注册的请求。调用方持有 lease，决定请求在 finalizer 提交后才退出 live 状态。
pub async fn perform_vcp_request_registered<R: Runtime>(
    app: &AppHandle<R>,
    payload: VcpRequestPayload,
    stream_channel: Option<Channel<StreamEvent>>,
    cancellation_token: CancellationToken,
) -> Result<VcpResponse, VcpRequestFailure> {
    log::info!(
        "[VCPClient] perform_vcp_request called for messageId: {}, context: {:?}",
        payload.message_id,
        payload.context
    );

    let message_id = payload.message_id.clone();
    let context = payload.context.clone();
    let transport_request_id = payload.effective_transport_request_id().to_string();
    let output = match message_key_from_context(&context, &message_id) {
        Ok(key) => request_output(app, &key, stream_channel),
        Err(_) => {
            let output = Arc::new(RequestOutput::default());
            output.bind_initial(stream_channel);
            output
        }
    };

    // === 1. 数据验证和多模态资产转换 ===
    if cancellation_token.is_cancelled() {
        return Ok(VcpResponse::cancelled());
    }
    let preparation_permit = tokio::select! {
        biased;
        _ = cancellation_token.cancelled() => return Ok(VcpResponse::cancelled()),
        permit = request_body::PREPARATION_SLOT.acquire() =>
            permit.map_err(|error| VcpRequestFailure::preparation(error.to_string()))?,
    };
    let prepared = preprocess_multimodal_messages(app, payload.messages, &cancellation_token).await;
    // Cancellation wins a simultaneous native failure, and no request reaches
    // either transport after the user stopped during attachment preparation.
    if cancellation_token.is_cancelled() {
        return Ok(VcpResponse::cancelled());
    }
    let mut messages = prepared.map_err(VcpRequestFailure::preparation)?;

    // === 2. 使用 turn 起点冻结的最终端点；重试与 Android Helper 始终复用此值 ===
    let final_url = payload.vcp_url.clone();

    // === 3. 补充 System 提示词首部 ===
    let has_system = messages.iter().any(|m| m["role"] == "system");
    if !has_system {
        messages.insert(0, json!({"role": "system", "content": ""}));
    }

    // === 4. 剥离并生成元数据时间戳绑定 ===
    let timestamp_bindings = extract_timestamp_bindings(&mut messages);

    // === 5. 准备请求体 ===
    let is_stream = payload.model_config["stream"].as_bool().unwrap_or(false);
    let mut request_body = payload.model_config;
    if let Some(obj) = request_body.as_object_mut() {
        obj.insert("messages".to_string(), Value::Array(messages));
        obj.insert("requestId".to_string(), json!(transport_request_id.clone()));
        obj.insert("stream".to_string(), json!(is_stream));
        if !timestamp_bindings.is_empty() {
            obj.insert(
                "vcpchatExtensions".to_string(),
                json!({
                    "schemaVersion": 1,
                    "messageMetadataMode": "hash_only",
                    "messageTimestampBindings": timestamp_bindings
                }),
            );
        }
    }

    let prepared_body = request_body::prepare(
        app.path()
            .app_cache_dir()
            .map_err(|error| VcpRequestFailure::preparation(error.to_string()))?,
        request_body,
        cancellation_token.clone(),
        preparation_permit,
    )
    .await;
    if cancellation_token.is_cancelled() {
        return Ok(VcpResponse::cancelled());
    }
    let request_body = prepared_body.map_err(VcpRequestFailure::preparation)?;

    // === 6. 配置网络请求（共享 ChatStream 画像 Client，克隆仅复制 Arc 句柄、共享连接池） ===
    let client = super::http_clients::client(super::http_clients::HttpProfile::ChatStream).clone();

    // === 7. 分发至专职处理器执行请求 ===
    if cancellation_token.is_cancelled() {
        return Ok(VcpResponse::cancelled());
    }
    if is_stream {
        handle_streaming_request(
            app,
            client,
            &final_url,
            &payload.vcp_api_key,
            Some(request_body),
            message_id,
            transport_request_id,
            context,
            cancellation_token,
            output,
            false,
        )
        .await
    } else {
        handle_non_streaming_request(
            client,
            &final_url,
            &payload.vcp_api_key,
            request_body,
            message_id,
            context,
            cancellation_token,
            output,
        )
        .await
        .map_err(VcpRequestFailure::from)
    }
}

fn bounded_attachment_label(value: &str) -> String {
    let basename = value.rsplit(['/', '\\']).next().unwrap_or_default();
    let filtered = basename
        .chars()
        .filter(|character| !character.is_control())
        .collect::<String>();
    let trimmed = filtered.trim();
    if trimmed.is_empty() {
        return "附件".to_string();
    }
    let mut end = trimmed.len().min(256);
    while end > 0 && !trimmed.is_char_boundary(end) {
        end -= 1;
    }
    trimmed[..end].to_string()
}

fn short_hash(value: &str) -> String {
    value
        .chars()
        .filter(char::is_ascii_hexdigit)
        .take(12)
        .collect()
}

fn remove_internal_local_attachment_metadata(message: &mut Value) {
    if let Some(object) = message.as_object_mut() {
        object.remove("__vcpLocalAttachments");
    }
}

// Count all audio data URLs, including historical/prepared context, separately
// from the existing image budget. This bounds the actual outbound payload.
const MODEL_REQUEST_AUDIO_DATA_MAX_BYTES: usize = 24 * 1024 * 1024;

fn account_audio_data(total: &mut usize, bytes: usize) -> Result<(), String> {
    *total = total.checked_add(bytes).ok_or("模型音频请求大小溢出")?;
    if *total > MODEL_REQUEST_AUDIO_DATA_MAX_BYTES {
        return Err("本次请求音频超过 24 MiB 编码预算，请减少音频数量或缩短包含音频的历史".into());
    }
    Ok(())
}

/// Resolve CAS attachments and normalize their model payload before transport.
async fn preprocess_multimodal_messages<R: Runtime>(
    app: &AppHandle<R>,
    raw_messages: Vec<Value>,
    cancellation: &CancellationToken,
) -> Result<Vec<Value>, String> {
    let mut messages: Vec<Value> = Vec::new();
    let mut image_data_bytes = 0usize;
    let mut audio_data_bytes = 0usize;
    for mut msg in raw_messages {
        crate::vcp_modules::media_processor::check_media_cancelled(cancellation)?;
        if !msg.is_object() {
            messages.push(json!({"role": "system", "content": "[Invalid message]"}));
            continue;
        }

        remove_internal_local_attachment_metadata(&mut msg);
        let had_content = msg.get("content").is_some();
        let content = msg
            .get_mut("content")
            .map(Value::take)
            .unwrap_or(Value::Null);

        // 处理多模态或复杂内容数组
        if let Value::Array(content_array) = content {
            let mut new_parts = Vec::new();
            for part in content_array {
                crate::vcp_modules::media_processor::check_media_cancelled(cancellation)?;
                if let Some(obj) = part.as_object() {
                    // local_file 只携带 CAS hash；host path 不进入模型消息。
                    if obj.get("type").and_then(|t| t.as_str()) == Some("local_file") {
                        let hash = obj.get("hash").and_then(Value::as_str).unwrap_or_default();
                        let label = bounded_attachment_label(
                            obj.get("name").and_then(Value::as_str).unwrap_or("附件"),
                        );
                        let declared_mime = obj
                            .get("mime")
                            .and_then(Value::as_str)
                            .unwrap_or("application/octet-stream");
                        let resolved = {
                            let pool = &app.state::<crate::vcp_modules::db_manager::DbState>().pool;
                            crate::vcp_modules::file_manager::resolve_attachment_cas_file(
                                app, pool, hash,
                            )
                            .await
                        };
                        let effective_mime = resolved
                            .as_ref()
                            .map(|record| record.mime_type.as_str())
                            .unwrap_or(declared_mime);
                        let media_kind = if effective_mime.starts_with("image/") {
                            "image"
                        } else if effective_mime.starts_with("audio/") {
                            "audio"
                        } else if effective_mime.starts_with("video/") {
                            "video"
                        } else {
                            "application"
                        };
                        let mut converted = false;
                        if let Ok(record) = resolved {
                            let path_buf = record.path;
                            if media_kind == "image" {
                                // 静态用途派生：保留原件，单图与整个请求分别执行预算。
                                let path_buf_clone = path_buf.clone();
                                let app_clone = app.clone();
                                match tokio::task::spawn_blocking(move || {
                                    convert_local_image_for_multimodal(&app_clone, &path_buf_clone)
                                })
                                .await
                                {
                                    Ok(Ok(data_url)) => {
                                        image_data_bytes = image_data_bytes
                                            .checked_add(data_url.len())
                                            .ok_or("模型图片请求大小溢出")?;
                                        if image_data_bytes > crate::vcp_modules::media_processor::image_extractor::MODEL_REQUEST_IMAGE_DATA_MAX_BYTES {
                                            return Err("本次请求图片超过 16 MiB 编码预算，请减少图片数量或缩短包含图片的历史".into());
                                        }
                                        new_parts.push(json!({
                                            "type": "image_url",
                                            "image_url": { "url": data_url }
                                        }));
                                        converted = true;
                                    }
                                    Ok(Err(e)) => {
                                        return Err(format!("图片 {label} 处理失败：{e}"))
                                    }
                                    Err(e) => {
                                        log::warn!(
                                            "[VCPClient] Image conversion task panicked: {}",
                                            e
                                        );
                                    }
                                }
                            } else if media_kind == "video" {
                                let frames = crate::vcp_modules::media_processor::process_video_for_multimodal(
                                    app, &path_buf, cancellation,
                                )
                                .await
                                .map_err(|error| format!("视频附件 {label} 处理失败：{error}"))?;
                                for frame_url in frames {
                                    image_data_bytes = image_data_bytes
                                        .checked_add(frame_url.len())
                                        .ok_or("模型图片请求大小溢出")?;
                                    if image_data_bytes > crate::vcp_modules::media_processor::image_extractor::MODEL_REQUEST_IMAGE_DATA_MAX_BYTES {
                                        return Err("本次请求图片超过 16 MiB 编码预算，请减少图片或视频数量".into());
                                    }
                                    new_parts.push(json!({
                                        "type": "image_url",
                                        "image_url": { "url": frame_url }
                                    }));
                                }
                                converted = true;
                            } else if media_kind == "audio" {
                                let audio_url = crate::vcp_modules::media_processor::process_audio_for_multimodal(
                                    app, &path_buf, cancellation,
                                )
                                .await
                                .map_err(|error| format!("音频附件 {label} 处理失败：{error}"))?;
                                account_audio_data(&mut audio_data_bytes, audio_url.len())?;
                                // VCP's multimodal proxy reads MIME-bearing data
                                // URLs in image_url, including audio/mp4 payloads.
                                new_parts.push(json!({
                                    "type": "image_url",
                                    "image_url": { "url": audio_url }
                                }));
                                converted = true;
                            }
                        } else {
                            if matches!(media_kind, "audio" | "video") {
                                return Err(format!(
                                    "音视频附件 {label} 的本地文件不可用，请重新导入后重试"
                                ));
                            }
                            log::warn!(
                                "[VCPClient] Attachment CAS {} is unavailable for multimodal preprocessing",
                                short_hash(hash)
                            );
                        }

                        // 文件不存在或读取失败时仅保留逻辑名，绝不回送 host path。
                        if !converted {
                            let mut warning = format!("[附件文件: {label}]");
                            if media_kind == "image" {
                                warning.push_str("\n<system_meta>[系统提示]：该图片的视觉信息提取失败，已转为纯文本占位符。</system_meta>");
                            }
                            new_parts.push(json!({
                                "type": "text",
                                "text": warning
                            }));
                        }
                    } else {
                        if obj.get("type").and_then(Value::as_str) == Some("image_url") {
                            if let Some(url) = obj
                                .get("image_url")
                                .and_then(|image| image.get("url"))
                                .and_then(Value::as_str)
                                .filter(|url| url.starts_with("data:"))
                            {
                                if url.starts_with("data:audio/") {
                                    account_audio_data(&mut audio_data_bytes, url.len())?;
                                } else {
                                    image_data_bytes = image_data_bytes
                                        .checked_add(url.len())
                                        .ok_or("模型图片请求大小溢出")?;
                                    if image_data_bytes > crate::vcp_modules::media_processor::image_extractor::MODEL_REQUEST_IMAGE_DATA_MAX_BYTES {
                                        return Err("本次请求图片超过 16 MiB 编码预算，请减少图片数量".into());
                                    }
                                }
                            }
                        }
                        new_parts.push(part);
                    }
                } else {
                    new_parts.push(part);
                }
            }
            msg["content"] = Value::Array(new_parts);
        } else if let Value::Object(mut object) = content {
            if let Some(text) = object.remove("text") {
                msg["content"] = text;
            } else {
                msg["content"] = json!(Value::Object(object).to_string());
            }
        } else if !content.is_string() && !content.is_null() {
            msg["content"] = json!(content.to_string());
        } else if had_content {
            msg["content"] = content;
        }

        messages.push(msg);
    }
    Ok(messages)
}

/// 2. 抽离时间戳与哈希绑定生成逻辑
fn extract_timestamp_bindings(messages: &mut [Value]) -> Vec<Value> {
    let mut message_timestamp_bindings = Vec::new();
    for (index, msg) in messages.iter_mut().enumerate() {
        let mut timestamp_meta = None;
        if let Some(obj) = msg.as_object_mut() {
            if let Some(meta) = obj.remove("__vcpchatTimestampMeta") {
                timestamp_meta = Some(meta);
            }
        }
        if let Some(meta) = timestamp_meta {
            if let (Some(message_id), Some(role), Some(timestamp)) = (
                meta.get("messageId").and_then(|id| id.as_str()),
                meta.get("role").and_then(|r| r.as_str()),
                meta.get("timestamp").and_then(|t| t.as_u64()),
            ) {
                use chrono::TimeZone;
                let timestamp_iso =
                    if let Some(dt) = chrono::Utc.timestamp_millis_opt(timestamp as i64).single() {
                        dt.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
                    } else {
                        "".to_string()
                    };

                let final_content_val = msg.get("content").unwrap_or(&Value::Null);
                let sent_message_hash = get_or_calculate_message_hash(final_content_val);

                message_timestamp_bindings.push(json!({
                    "messageId": message_id,
                    "role": role,
                    "timestamp": timestamp,
                    "timestampIso": timestamp_iso,
                    "source": "client_history",
                    "sentMessageHash": sent_message_hash,
                    "sentMessageIndex": index
                }));
            }
        }
    }
    message_timestamp_bindings
}

#[cfg(target_os = "android")]
async fn get_helper_port<R: Runtime>(app: &AppHandle<R>) -> Result<u16, String> {
    let cache_dir = app.path().app_cache_dir().map_err(|e| e.to_string())?;
    let port_file = cache_dir.join("sse_helper.port");
    let content = tokio::fs::read_to_string(port_file)
        .await
        .map_err(|e| e.to_string())?;
    let port = content.trim().parse::<u16>().map_err(|e| e.to_string())?;
    Ok(port)
}

#[cfg(target_os = "android")]
async fn connect_to_helper<R: Runtime>(
    app: &AppHandle<R>,
    action: &str,
    msg_id: &str,
    extra_params: Option<Value>,
) -> Result<tokio::net::TcpStream, String> {
    let cache_dir = app.path().app_cache_dir().map_err(|e| e.to_string())?;
    let port_file = cache_dir.join("sse_helper.port");

    // 1. 尝试使用已有的端口文件进行连接（适用于 helper 已经在运行且就绪的情况）
    if let Ok(content) = tokio::fs::read_to_string(&port_file).await {
        if let Ok(port) = content.trim().parse::<u16>() {
            if let Ok(stream) = connect_helper_port(port).await {
                log::info!(
                    "[VCPClient] Connected to existing sse helper socket on 127.0.0.1:{}",
                    port
                );
                return send_command_to_stream(stream, action, msg_id, extra_params).await;
            }
        }
    }
    // 2. 如果连接失败或文件不存在，启动/唤醒 helper 服务。
    // A missing/stale port file or connect timeout does not prove that the
    // generation vanished. Resume must obtain the helper's explicit response;
    // waking this service never starts another upstream request.
    log::info!(
        "[VCPClient] Helper not responding or port file missing. Starting/Waking helper service..."
    );
    tauri_plugin_vcp_mobile::stream::start_helper_service(app.clone())?;

    // 3. 循环等待新端口文件并尝试连接（最多尝试 60 次，每次间隔 50ms，总计 3 秒超时）
    let mut last_err = String::new();
    let max_attempts = 60;
    let delay = Duration::from_millis(50);
    let ready_deadline = tokio::time::Instant::now() + Duration::from_secs(3);

    for attempt in 1..=max_attempts {
        if tokio::time::Instant::now() >= ready_deadline {
            break;
        }
        let content = match tokio::fs::read_to_string(&port_file).await {
            Ok(c) => c,
            Err(e) => {
                last_err = format!("Read port file error: {}", e);
                tokio::time::sleep(delay).await;
                continue;
            }
        };

        let port_str = content.trim();
        if port_str.is_empty() {
            tokio::time::sleep(delay).await;
            continue;
        }

        let port = match port_str.parse::<u16>() {
            Ok(p) => p,
            Err(e) => {
                last_err = format!("Parse port error: {}", e);
                tokio::time::sleep(delay).await;
                continue;
            }
        };

        // 尝试连接
        match connect_helper_port_with_timeout(port, Duration::from_millis(100)).await {
            Ok(stream) => {
                log::info!(
                    "[VCPClient] Connected to sse helper socket on 127.0.0.1:{} after {} attempts",
                    port,
                    attempt
                );
                return send_command_to_stream(stream, action, msg_id, extra_params).await;
            }
            Err(e) => {
                last_err = format!("Connect to 127.0.0.1:{} failed: {}", port, e);
                tokio::time::sleep(delay).await;
            }
        }
    }

    let error = format!(
        "Failed to connect to sse helper after {}s (last error: {})",
        (max_attempts as f32 * 0.05),
        last_err
    );
    tauri_plugin_vcp_mobile::events::report_service_failure(app, "helper", &error);
    Err(error)
}

// 辅助函数：向已连接的 TcpStream 发送 JSON 指令
#[cfg(target_os = "android")]
async fn send_command_to_stream(
    mut stream: tokio::net::TcpStream,
    action: &str,
    msg_id: &str,
    extra_params: Option<Value>,
) -> Result<tokio::net::TcpStream, String> {
    let mut cmd = json!({
        "action": action,
        "requestId": msg_id
    });
    if let Some(Value::Object(params)) = extra_params {
        if let Some(obj) = cmd.as_object_mut() {
            for (k, v) in params {
                obj.insert(k, v);
            }
        }
    }

    use tokio::io::AsyncWriteExt;
    let cmd_str = cmd.to_string();
    let cmd_bytes = cmd_str.as_bytes();
    let len = cmd_bytes.len() as u32;
    tokio::time::timeout(HELPER_IO_TIMEOUT, async {
        stream
            .write_all(&len.to_be_bytes())
            .await
            .map_err(|e| format!("Write command length error: {}", e))?;
        stream
            .write_all(cmd_bytes)
            .await
            .map_err(|e| format!("Write command error: {}", e))?;
        stream
            .flush()
            .await
            .map_err(|e| format!("Flush command error: {}", e))?;
        Ok::<(), String>(())
    })
    .await
    .map_err(|_| "Helper command write timed out".to_string())??;
    Ok(stream)
}

#[cfg(target_os = "android")]
async fn observe_detached_helper<R: Runtime>(
    app: &AppHandle<R>,
    request_id: &str,
    generation: Option<&str>,
) {
    // Only an already-running helper can own a detached upstream. A pending
    // database row, a completed mailbox file or an empty port creates no lease.
    let observation = async {
        let stream = connect_helper_port(get_helper_port(app).await?).await?;
        let stream = send_command_to_stream(
            stream,
            "observe_activity",
            request_id,
            generation.map(|id| json!({ "generation": id })),
        )
        .await?;
        let mut reader = FramedRead::new(stream, helper_frame_codec());
        let Some(frame) = reader.next().await else {
            return Ok(None);
        };
        let response: Value = serde_json::from_slice(&frame.map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
        let observed = response["generation"].as_str();
        if response["requestId"].as_str() != Some(request_id)
            || response["eventType"].as_str() != Some("observing")
            || observed.is_none()
            || (generation.is_some() && generation != observed)
        {
            return Err("Invalid helper activity confirmation".to_string());
        }
        Ok::<_, String>(Some(reader))
    };
    if let Ok(Ok(Some(mut reader))) = tokio::time::timeout(HELPER_IO_TIMEOUT, observation).await {
        let guard = super::vcp_ws_activity::ChatActivityGuard::acquire(app);
        tokio::spawn(async move {
            // Completion, discard, a replacement observer and helper process
            // death all close the socket. No timer or result ACK is necessary.
            let _ = reader.next().await;
            drop(guard);
        });
    }
}

#[cfg(target_os = "android")]
async fn query_helper_generation<R: Runtime>(
    app: &AppHandle<R>,
    request_id: &str,
) -> Result<String, String> {
    let port = get_helper_port(app).await?;
    let stream = connect_helper_port(port).await?;
    let stream = send_command_to_stream(stream, "query", request_id, None).await?;
    let mut reader = FramedRead::new(stream, helper_frame_codec());
    let frame = tokio::time::timeout(HELPER_IO_TIMEOUT, reader.next())
        .await
        .map_err(|_| "Helper generation query timed out".to_string())?
        .ok_or_else(|| "Helper generation query ended at EOF".to_string())?
        .map_err(|error| format!("Helper generation query frame error: {error}"))?;
    let response: Value = serde_json::from_slice(&frame)
        .map_err(|error| format!("Invalid helper query response: {error}"))?;
    if response["requestId"].as_str() != Some(request_id) {
        return Err("Helper generation query identity mismatch".to_string());
    }
    response["generation"]
        .as_str()
        .filter(|generation| !generation.is_empty())
        .map(str::to_string)
        .ok_or_else(|| HELPER_SESSION_NOT_FOUND.to_string())
}

#[cfg(target_os = "android")]
async fn helper_session_control<R: Runtime>(
    app: &AppHandle<R>,
    action: &str,
    request_id: &str,
    generation: &str,
) -> Result<(), String> {
    let port = get_helper_port(app).await?;
    let stream = connect_helper_port(port).await?;
    let stream = send_command_to_stream(
        stream,
        action,
        request_id,
        Some(json!({ "generation": generation })),
    )
    .await?;
    let mut reader = FramedRead::new(stream, helper_frame_codec());
    let frame = tokio::time::timeout(HELPER_IO_TIMEOUT, reader.next())
        .await
        .map_err(|_| format!("Helper {action} ACK timed out"))?
        .ok_or_else(|| format!("Helper {action} ACK ended at EOF"))?
        .map_err(|error| format!("Helper {action} ACK frame error: {error}"))?;
    let ack: Value = serde_json::from_slice(&frame)
        .map_err(|error| format!("Invalid helper {action} ACK: {error}"))?;
    validate_helper_control_ack(&ack, action, request_id, generation)
}

#[cfg(target_os = "android")]
fn validate_helper_control_ack(
    ack: &Value,
    action: &str,
    request_id: &str,
    generation: &str,
) -> Result<(), String> {
    let expected_action = format!("{action}_ack");
    if ack.get("action").and_then(Value::as_str) == Some(expected_action.as_str())
        && ack.get("requestId").and_then(Value::as_str) == Some(request_id)
        && ack.get("generation").and_then(Value::as_str) == Some(generation)
        && ack.get("accepted").and_then(Value::as_bool) == Some(true)
    {
        return Ok(());
    }
    Err(format!(
        "Helper {action} ACK rejected for request {request_id}"
    ))
}

#[cfg(target_os = "android")]
pub(crate) async fn ack_helper_committed<R: Runtime>(
    app: &AppHandle<R>,
    request_id: &str,
    generation: &str,
) -> Result<(), String> {
    helper_session_control(app, "ack_committed", request_id, generation).await
}

#[cfg(target_os = "android")]
pub(crate) async fn discard_helper_session<R: Runtime>(
    app: &AppHandle<R>,
    request_id: &str,
    generation: &str,
) -> Result<(), String> {
    helper_session_control(app, "discard", request_id, generation).await
}

/// Decode complete SSE events, independently of HTTP chunk or line boundaries.
/// EOF does not dispatch a pending event without its terminating blank line.
#[derive(Default)]
#[cfg(any(not(target_os = "android"), test))]
struct SseDecoder {
    scanned: usize,
    skip_lf: bool,
    first_line_seen: bool,
    data: Option<String>,
}

#[cfg(any(not(target_os = "android"), test))]
impl tokio_util::codec::Decoder for SseDecoder {
    type Item = String;
    type Error = std::io::Error;

    fn decode(&mut self, source: &mut bytes::BytesMut) -> Result<Option<String>, Self::Error> {
        loop {
            if self.skip_lf {
                if source.is_empty() {
                    return Ok(None);
                }
                if source[0] == b'\n' {
                    let _ = source.split_to(1);
                }
                self.skip_lf = false;
            }
            let Some(end) = source[self.scanned..]
                .iter()
                .position(|byte| matches!(byte, b'\n' | b'\r'))
                .map(|offset| self.scanned + offset)
            else {
                self.scanned = source.len();
                return Ok(None);
            };
            let line = source.split_to(end + 1);
            self.scanned = 0;
            self.skip_lf = line[end] == b'\r';
            let line = String::from_utf8_lossy(&line[..end]);
            let line = if self.first_line_seen {
                line.as_ref()
            } else {
                self.first_line_seen = true;
                line.strip_prefix('\u{feff}').unwrap_or(&line)
            };
            if line.is_empty() {
                if let Some(mut data) = self.data.take() {
                    data.pop(); // Every data field appends exactly one newline.
                    return Ok(Some(data));
                }
                continue;
            }
            if line.starts_with(':') {
                continue;
            }
            let (field, value) = line.split_once(':').unwrap_or((line, ""));
            if field == "data" {
                let data = self.data.get_or_insert_with(String::new);
                data.push_str(value.strip_prefix(' ').unwrap_or(value));
                data.push('\n');
            }
        }
    }

    fn decode_eof(&mut self, source: &mut bytes::BytesMut) -> Result<Option<String>, Self::Error> {
        if let Some(event) = self.decode(source)? {
            return Ok(Some(event));
        }
        source.clear();
        self.scanned = 0;
        self.data = None;
        Ok(None)
    }
}

#[cfg(any(not(target_os = "android"), test))]
fn absorb_sse_data(
    data: &str,
    pending_chunk: &mut String,
    last_finish_reason: &mut Option<String>,
) -> Result<bool, String> {
    if data.trim() == "[DONE]" {
        return Ok(true);
    }
    if data.trim().is_empty() {
        return Ok(false);
    }
    let value: Value =
        serde_json::from_str(data).map_err(|error| format!("Invalid SSE JSON: {error}"))?;
    if let Some(error) = value.get("error").filter(|error| !error.is_null()) {
        let message = error
            .as_str()
            .or_else(|| error.get("message").and_then(Value::as_str))
            .unwrap_or("Upstream SSE request failed");
        return Err(message.to_string());
    }
    if let Some(choice) = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
    {
        if let Some(reason) = choice
            .get("finish_reason")
            .and_then(Value::as_str)
            .filter(|reason| !reason.is_empty())
        {
            *last_finish_reason = Some(reason.to_string());
        }
        if let Some(content) = choice
            .get("delta")
            .and_then(|delta| delta.get("content"))
            .and_then(Value::as_str)
        {
            pending_chunk.push_str(content);
        }
    }
    Ok(false)
}

pub(crate) const STREAM_CAPACITY_EXHAUSTED: &str = "同时进行的请求已达到 8 路，请稍后重试";
#[cfg(any(target_os = "android", test))]
pub(crate) const HELPER_SESSION_NOT_FOUND: &str = "helper_session_not_found";
static CHAT_TRANSPORT_SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(8);

fn flush_aurora_window(buffer: &mut AuroraBuffer, pending_chunk: &mut String) {
    if !pending_chunk.is_empty() {
        buffer.append_chunk(pending_chunk);
        pending_chunk.clear();
        buffer.process_queue();
    }
}

/// One transport owns one generation. Rendering runs on its own 33ms clock;
/// neither a model-round finish reason nor a suspended WebView ends the stream.
#[allow(clippy::too_many_arguments, unused_variables)]
async fn handle_streaming_request<R: Runtime>(
    app: &AppHandle<R>,
    client: Client,
    final_url: &str,
    api_key: &str,
    request_body: Option<PreparedBody>,
    message_id: String,
    transport_request_id: String,
    context: Option<Value>,
    cancellation_token: CancellationToken,
    output: Arc<RequestOutput>,
    is_resume: bool,
) -> Result<VcpResponse, VcpRequestFailure> {
    if !is_resume && cancellation_token.is_cancelled() {
        return Ok(VcpResponse::cancelled());
    }
    let _transport_slot = CHAT_TRANSPORT_SLOTS
        .try_acquire()
        .map_err(|_| VcpRequestFailure::from(STREAM_CAPACITY_EXHAUSTED.to_string()))?;
    let mut aurora_buffer = AuroraBuffer::new();
    let mut pending_chunk = String::new();
    let mut last_finish_reason = None;
    let send_update = |buffer: &mut AuroraBuffer| {
        output.send_aurora(buffer, &message_id, &context, None, false);
    };

    #[cfg(target_os = "android")]
    let mut reader = {
        let (action, params) = if is_resume {
            ("resume", None)
        } else {
            let body = request_body
                .as_ref()
                .ok_or_else(|| VcpRequestFailure::preparation("模型请求正文未准备".into()))?;
            let mut helper_context = context.clone().unwrap_or_else(|| json!({}));
            helper_context["messageId"] = json!(message_id);
            (
                "start",
                Some(json!({
                    "url": final_url,
                    "headers": json!({
                        "Authorization": format!("Bearer {api_key}"),
                        "Content-Type": "application/json",
                    }).to_string(),
                    "bodyToken": body.token,
                    "bodyLength": body.length,
                    "context": helper_context,
                })),
            )
        };
        let stream = match connect_to_helper(app, action, &transport_request_id, params).await {
            Ok(stream) => stream,
            Err(error) => {
                // A failed local write/attach does not prove the helper never
                // admitted this request. Observe only if it confirms ownership.
                observe_detached_helper(app, &transport_request_id, None).await;
                if cancellation_token.is_cancelled() {
                    checkpoint_stop_partial(app, &context, &message_id, None).await?;
                    return Err(VcpRequestFailure::stopping(error, None));
                }
                return Err(VcpRequestFailure::recoverable_helper(error, None));
            }
        };
        FramedRead::new(stream, helper_frame_codec())
    };
    #[cfg(target_os = "android")]
    let mut helper_generation: Option<String> = None;
    #[cfg(target_os = "android")]
    let mut snapshot_in_progress = false;
    #[cfg(target_os = "android")]
    let mut snapshot_received = false;
    #[cfg(target_os = "android")]
    let mut helper_terminal_received = false;
    #[cfg(target_os = "android")]
    let mut _resumed_chat_activity = None;
    #[cfg(target_os = "android")]
    let mut helper_rejected = false;
    #[cfg(target_os = "android")]
    let mut cancel_sent = false;

    #[cfg(not(target_os = "android"))]
    let mut reader = {
        if is_resume {
            return Err("This platform has no surviving background transport"
                .to_string()
                .into());
        }
        let body = request_body
            .ok_or_else(|| VcpRequestFailure::preparation("模型请求正文未准备".into()))?;
        let length = body.length;
        let upload = body
            .into_http_body()
            .await
            .map_err(VcpRequestFailure::preparation)?;
        let response = tokio::select! {
            biased;
            _ = cancellation_token.cancelled() => {
                return Ok(VcpResponse::cancelled());
            }
            response = client.post(final_url)
                .header(AUTHORIZATION, format!("Bearer {api_key}"))
                .header(CONTENT_TYPE, "application/json")
                .header(CONTENT_LENGTH, length)
                .body(upload)
                .send() => response.map_err(|error| error.to_string())?,
        };
        if !response.status().is_success() {
            return Err(format!("VCP Error: {}", response.status()).into());
        }
        let stream = response.bytes_stream().map_err(std::io::Error::other);
        FramedRead::new(StreamReader::new(stream), SseDecoder::default())
    };

    let mut render_clock = tokio::time::interval_at(
        tokio::time::Instant::now() + AURORA_PARSE_INTERVAL,
        AURORA_PARSE_INTERVAL,
    );
    render_clock.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut cancelled = false;
    #[cfg(target_os = "android")]
    let mut cancel_deadline = None;

    let outcome: Result<(), String> = async {
        loop {
            #[cfg(target_os = "android")]
            if cancelled && !cancel_sent {
                if let Some(generation) = helper_generation.as_deref() {
                    helper_session_control(app, "cancel", &transport_request_id, generation).await?;
                    cancel_sent = true;
                }
            }

            tokio::select! {
                biased;
                _ = cancellation_token.cancelled(), if !cancelled => {
                    cancelled = true;
                    #[cfg(target_os = "android")]
                    {
                        cancel_deadline = Some(tokio::time::Instant::now() + Duration::from_secs(10));
                    }
                    #[cfg(not(target_os = "android"))]
                    {
                        last_finish_reason = Some("cancelled_by_user".to_string());
                        return Ok(());
                    }
                }
                _ = async {
                    #[cfg(target_os = "android")]
                    if let Some(deadline) = cancel_deadline {
                        tokio::time::sleep_until(deadline).await;
                        return;
                    }
                    std::future::pending::<()>().await;
                } => {
                    return Err("Helper cancellation did not reach its terminal state".to_string());
                }
                _ = output.changed.notified() => {
                    #[cfg(target_os = "android")]
                    if snapshot_in_progress {
                        continue;
                    }
                    flush_aurora_window(&mut aurora_buffer, &mut pending_chunk);
                    send_update(&mut aurora_buffer);
                }
                _ = render_clock.tick() => {
                    #[cfg(target_os = "android")]
                    if snapshot_in_progress {
                        continue;
                    }
                    if !pending_chunk.is_empty() {
                        flush_aurora_window(&mut aurora_buffer, &mut pending_chunk);
                        send_update(&mut aurora_buffer);
                    }
                }
                next = reader.next() => {
                    #[cfg(not(target_os = "android"))]
                    match next {
                        Some(Ok(data)) => {
                            if absorb_sse_data(&data, &mut pending_chunk, &mut last_finish_reason)? {
                                return Ok(());
                            }
                        }
                        Some(Err(error)) => return Err(format!("SSE stream read failed: {error}")),
                        None => return Ok(()),
                    }
                    #[cfg(target_os = "android")]
                    match next {
                        Some(Ok(frame)) => {
                            let event: Value = serde_json::from_slice(&frame)
                                .map_err(|error| format!("Invalid helper stream frame: {error}"))?;
                            if event["requestId"].as_str() != Some(transport_request_id.as_str()) {
                                return Err("Helper stream identity mismatch".to_string());
                            }
                            let event_type = event["eventType"].as_str().unwrap_or("");
                            if event_type == "error" && event["code"].as_str() == Some("session_not_found") {
                                return Err(HELPER_SESSION_NOT_FOUND.to_string());
                            }
                            if event_type == "error" && event["code"].as_str() == Some("capacity_exhausted") {
                                helper_rejected = true;
                                return Err(STREAM_CAPACITY_EXHAUSTED.to_string());
                            }
                            if event_type == "error" && matches!(event["code"].as_str(), Some("invalid_request" | "service_stopping")) {
                                helper_rejected = true;
                            }
                            if let Some(generation) = event["generation"].as_str() {
                                if let Some(current) = helper_generation.as_deref() {
                                    if current != generation {
                                        return Err("Helper generation changed during stream".to_string());
                                    }
                                } else {
                                    helper_generation = Some(generation.to_string());
                                }
                            } else if event_type != "error" {
                                return Err("Helper stream frame is missing generation".to_string());
                            }
                            match event_type {
                                "started" => {}
                                "data" => {
                                    if snapshot_in_progress || (is_resume && !snapshot_received) {
                                        return Err("Helper data arrived before a complete snapshot".to_string());
                                    }
                                    let content = event["content"].as_str()
                                        .ok_or_else(|| "Helper data frame is missing content".to_string())?;
                                    pending_chunk.push_str(content);
                                    if let Some(reason) = event["finishReason"].as_str() {
                                        last_finish_reason = Some(reason.to_string());
                                    }
                                }
                                "snapshot_start" => {
                                    if !is_resume || snapshot_in_progress || !aurora_buffer.full_text.is_empty() || !pending_chunk.is_empty() {
                                        return Err("Unexpected helper snapshot boundary".to_string());
                                    }
                                    snapshot_in_progress = true;
                                }
                                "snapshot_chunk" => {
                                    if !snapshot_in_progress {
                                        return Err("Helper snapshot chunk has no active snapshot".to_string());
                                    }
                                    let content = event["content"].as_str()
                                        .ok_or_else(|| "Helper snapshot chunk is missing content".to_string())?;
                                    pending_chunk.push_str(content);
                                }
                                "snapshot_end" => {
                                    if !snapshot_in_progress {
                                        return Err("Helper snapshot end has no active snapshot".to_string());
                                    }
                                    let completed = event["completed"].as_bool()
                                        .ok_or_else(|| "Helper snapshot end is missing completion state".to_string())?;
                                    snapshot_in_progress = false;
                                    snapshot_received = true;
                                    flush_aurora_window(&mut aurora_buffer, &mut pending_chunk);
                                    output.send_aurora(&mut aurora_buffer, &message_id, &context, None, true);
                                    if let Some(reason) = event["finishReason"].as_str() {
                                        last_finish_reason = Some(reason.to_string());
                                    }
                                    if completed {
                                        helper_terminal_received = true;
                                        if let Some(error) = event["error"].as_str() {
                                            return Err(error.to_string());
                                        }
                                        return Ok(());
                                    }
                                    _resumed_chat_activity = Some(super::vcp_ws_activity::ChatActivityGuard::acquire(app));
                                }
                                "terminal" => {
                                    if snapshot_in_progress || (is_resume && !snapshot_received) {
                                        return Err("Helper terminal arrived before a complete snapshot".to_string());
                                    }
                                    helper_terminal_received = true;
                                    if let Some(reason) = event["finishReason"].as_str() {
                                        last_finish_reason = Some(reason.to_string());
                                    }
                                    if let Some(error) = event["error"].as_str() {
                                        return Err(error.to_string());
                                    }
                                    return Ok(());
                                }
                                "error" => {
                                    return Err(event["error"].as_str()
                                        .unwrap_or("Helper stream request failed").to_string());
                                }
                                _ => return Err("Unknown helper stream event".to_string()),
                            }
                        }
                        Some(Err(error)) => return Err(format!("Helper stream read failed: {error}")),
                        None => return Err("Helper connection closed before its terminal event".to_string()),
                    }
                }
            }
        }
    }.await;

    #[cfg(target_os = "android")]
    let outcome = match outcome {
        Err(message) if !helper_terminal_received && !helper_rejected => {
            // Losing the local transport says nothing about the upstream
            // generation. The helper remains its owner; a later explicit
            // recovery can claim a complete snapshot or its completed mailbox.
            drop(reader);
            let partial_content = if is_resume && !snapshot_received {
                None
            } else {
                flush_aurora_window(&mut aurora_buffer, &mut pending_chunk);
                send_update(&mut aurora_buffer);
                Some(aurora_buffer.full_text)
            };
            // The helper may have published its completed result after the
            // five-second ACK grace. EOF is not a new generation failure.
            let message = match read_completed_transport(app, &transport_request_id, &output).await {
                Ok(Some(result)) => return result,
                Ok(None) => message,
                Err(error) => error.to_string(),
            };
            if message != HELPER_SESSION_NOT_FOUND {
                observe_detached_helper(app, &transport_request_id, helper_generation.as_deref())
                    .await;
            }
            if cancelled || cancellation_token.is_cancelled() {
                checkpoint_stop_partial(app, &context, &message_id, partial_content.as_deref())
                    .await?;
                return Err(VcpRequestFailure::stopping(message, partial_content));
            }
            return Err(VcpRequestFailure::recoverable_helper(
                message,
                partial_content,
            ));
        }
        outcome => outcome,
    };

    #[cfg(target_os = "android")]
    if let Some(generation) = helper_generation {
        // A terminal TCP write is not a database commit. Keep the helper's
        // connection alive until the finalizer commits or abandons this attempt.
        output.retain_helper_handoff(transport_request_id, generation, Some(reader.into_inner()));
    }
    flush_aurora_window(&mut aurora_buffer, &mut pending_chunk);
    aurora_buffer.finalize();
    send_update(&mut aurora_buffer);
    if let Err(message) = outcome {
        return Err(VcpRequestFailure::streaming(
            message,
            aurora_buffer.full_text,
        ));
    }
    let is_aborted = last_finish_reason.as_deref() == Some("cancelled_by_user");
    Ok(VcpResponse {
        full_content: aurora_buffer.full_text,
        finish_reason: Some(last_finish_reason.unwrap_or_else(|| "completed".to_string())),
        is_aborted,
    })
}

/// 4. 抽离非流式请求循环
#[allow(clippy::too_many_arguments)]
async fn handle_non_streaming_request(
    client: Client,
    final_url: &str,
    api_key: &str,
    request_body: PreparedBody,
    message_id: String,
    context: Option<Value>,
    cancellation_token: CancellationToken,
    output: Arc<RequestOutput>,
) -> Result<VcpResponse, String> {
    let _transport_slot = CHAT_TRANSPORT_SLOTS
        .try_acquire()
        .map_err(|_| STREAM_CAPACITY_EXHAUSTED.to_string())?;
    let send_stream_event = |event: StreamEvent| {
        output.send(event);
    };
    let length = request_body.length;
    let upload = request_body.into_http_body().await?;
    let request_future = client
        .post(final_url)
        .header(AUTHORIZATION, format!("Bearer {}", api_key))
        .header(CONTENT_TYPE, "application/json")
        .header(CONTENT_LENGTH, length)
        .body(upload)
        .send();

    let response = tokio::select! {
        _ = cancellation_token.cancelled() => {
            log::warn!("[VCPClient] Non-streaming request aborted before response for message: {}", message_id);
            // 用户取消由上层统一 Finalizer 持久化并发送唯一终态事件。
            return Ok(VcpResponse::cancelled());
        }
        res = request_future => {
            match res {
                Ok(resp) => resp,
                Err(e) => {
                    let err_msg = format!("VCP请求失败: {}", e);
                    return Err(err_msg);
                }
            }
        }
    };

    if !response.status().is_success() {
        let status = response.status();
        let text = tokio::select! {
            _ = cancellation_token.cancelled() => return Ok(VcpResponse::cancelled()),
            text = response.text() => text.unwrap_or_default(),
        };
        let err_msg = format!("VCP服务器错误: {} - {}", status, text);
        return Err(err_msg);
    }

    let mut vcp_response = tokio::select! {
        _ = cancellation_token.cancelled() => return Ok(VcpResponse::cancelled()),
        value = response.json::<Value>() => value.map_err(|error| format!("JSON解析失败: {error}"))?,
    };

    // 从标准的 OpenAI 格式中提取文本和结束原因
    let first_choice = vcp_response
        .get_mut("choices")
        .and_then(Value::as_array_mut)
        .and_then(|choices| choices.first_mut());
    let (full_content, finish_reason) = if let Some(choice) = first_choice {
        let content = match choice
            .get_mut("message")
            .and_then(|message| message.get_mut("content"))
            .map(Value::take)
        {
            Some(Value::String(content)) => content,
            _ => String::new(),
        };
        let reason = match choice.get_mut("finish_reason").map(Value::take) {
            Some(Value::String(reason)) if reason == "stop" => Some("completed".to_string()),
            Some(Value::String(reason)) => Some(reason),
            _ => None,
        };
        (content, reason)
    } else {
        (String::new(), None)
    };
    drop(vcp_response);

    // 发送单次 aurora 事件以将文本呈现在 UI 中
    let update = AuroraUpdate {
        kind: AuroraUpdateKind::Snapshot,
        stream_id: None,
        stable_blocks: None,
        stable_append: None,
        tail_block: None,
        tail_mode: None,
        tail_op: None,
        tail_frame: None,
        content: Some(full_content.clone()),
        chunk: None,
    };
    send_stream_event(StreamEvent::aurora(
        message_id.clone(),
        update,
        context.clone(),
    ));

    Ok(VcpResponse {
        full_content,
        finish_reason,
        is_aborted: false,
    })
}

/// 中止请求 Command: interruptRequest
/// 通过完整消息身份立即取消对应 outer request 的共享取消域
#[tauri::command]
#[allow(non_snake_case)]
pub async fn interruptRequest(
    app: AppHandle,
    state: tauri::State<'_, ActiveRequests>,
    owner_id: String,
    owner_type: String,
    topic_id: String,
    message_id: String,
) -> Result<Value, String> {
    let key = MessageKey::new(
        TopicKey::new(owner_type, owner_id, topic_id),
        message_id.clone(),
    );
    let origin = state.0.get(&key).and_then(|entry| {
        entry
            .output
            .state
            .lock()
            .ok()?
            .origin_user_message_id
            .clone()
    });
    if let Some(origin) = origin {
        let speech_app = app.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(error) = crate::vcp_modules::chat::voice_service::stop_chat_reply_by_origin(
                &speech_app,
                &origin,
            )
            .await
            {
                log::warn!("[VCPClient] Could not stop reply speech: {error}");
            }
        });
    }
    log::info!(
        "[VCPClient] interruptRequest called for messageId: {}. Active requests: {}",
        message_id,
        state.0.len()
    );
    let db = app.state::<DbState>();
    let pending = sqlx::query(
        "UPDATE active_generations SET stop_requested = 1
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?",
    )
    .bind(&key.topic.owner_type)
    .bind(&key.topic.owner_id)
    .bind(&key.topic.topic_id)
    .bind(&key.msg_id)
    .execute(&db.pool)
    .await
    .map_err(|error| error.to_string())?
    .rows_affected()
        != 0;
    let entry = state
        .0
        .get(&key)
        .filter(|entry| !entry.released.is_cancelled())
        .map(|entry| entry.value().clone());
    if entry.is_some() || pending {
        schedule_pending_stop(&app, &state, key.clone());
    }
    if entry.is_none() && pending {
        return Ok(json!({ "success": true, "status": "cancelling" }));
    }
    let settlement = async {
        if let Some(entry) = entry {
            entry.released.cancelled().await;
        }
        let db = app.state::<DbState>();
        let reason: Option<Option<String>> = sqlx::query_scalar(
            "SELECT finish_reason FROM messages WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ? AND deleted_at IS NULL",
        )
        .bind(&key.topic.owner_type)
        .bind(&key.topic.owner_id)
        .bind(&key.topic.topic_id)
        .bind(&key.msg_id)
        .fetch_optional(&db.pool)
        .await
        .map_err(|error| error.to_string())?;
        let retained_terminal = state.0.get(&key).is_some_and(|entry| {
            entry.released.is_cancelled() && entry.output.pending_terminal().is_some()
        });
        match reason.flatten() {
            Some(reason) => {
                Ok(json!({ "success": true, "status": "completed", "finishReason": reason }))
            }
            None if retained_terminal => Ok(json!({ "success": true, "status": "cancelling" })),
            None if pending => Ok(json!({ "success": true, "status": "cancelling" })),
            None => Err(format!(
                "Request {} has no committed terminal result",
                message_id
            )),
        }
    };
    // Cancelling this wait never aborts the request owner or its commit.
    match tokio::time::timeout(Duration::from_secs(5), settlement).await {
        Ok(result) => result,
        Err(_) => Ok(json!({ "success": true, "status": "cancelling" })),
    }
}

/// 测试模型发现端点；Raw 无法安全推导 `/v1/models` 时返回显式不可用状态。
#[tauri::command]
pub async fn test_vcp_connection(
    vcp_url: String,
    vcp_api_key: String,
    chat_endpoint_mode: ChatEndpointMode,
) -> Result<Value, String> {
    log::info!("[VCPClient] test_vcp_connection called");

    let Some(models_url) = resolve_model_discovery_endpoint(&vcp_url, chat_endpoint_mode)? else {
        return Ok(json!({
            "success": true,
            "status": 0,
            "modelCount": 0,
            "models": Value::Null,
            "modelDiscoveryAvailable": false
        }));
    };

    log::info!("[VCPClient] Testing derived model discovery endpoint");

    // 一次性连接探测：按 http_clients.rs 规矩 4 的有据例外，瞬时 Client 用完即弃，
    // 避免探测结果受共享池内半死连接干扰。
    let client = Client::builder()
        .timeout(Duration::from_secs(10)) // 测试连接 10s 超时即可
        .build()
        .map_err(|e| e.to_string())?;

    let res = client
        .get(&models_url)
        .header(AUTHORIZATION, format!("Bearer {}", vcp_api_key))
        .send()
        .await
        .map_err(|e| format!("网络请求失败: {}", e))?;

    let status = res.status();
    if status.is_success() {
        let json_res: Value = res
            .json()
            .await
            .map_err(|e| format!("JSON解析失败: {}", e))?;

        // 尝试提取模型数量，对齐桌面端 `cachedModels = data.data || []`
        let model_count = json_res
            .get("data")
            .and_then(|data| data.as_array())
            .map(|arr| arr.len())
            .unwrap_or(0);

        Ok(json!({
            "success": true,
            "status": status.as_u16(),
            "modelCount": model_count,
            "models": json_res,
            "modelDiscoveryAvailable": true
        }))
    } else {
        let text = res.text().await.unwrap_or_default();
        Err(format!("验证失败 ({}): {}", status.as_u16(), text))
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ActiveGeneration {
    pub msg_id: String,
    pub topic_id: String,
    pub owner_id: String,
    pub owner_type: String,
    pub created_at: i64,
    pub agent_id: Option<String>,
    pub agent_name: Option<String>,
    pub stop_requested: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReconnectChatStreamsResult {
    generations: Vec<ActiveGeneration>,
    group_turns: Vec<Value>,
    active_requests: Vec<Value>,
}

/// Replaces the interface consumer once, then delegates every transport or
/// mailbox reconciliation to its existing request lease. No generation is
/// awaited by this command and no response text is copied into its receipt.
#[tauri::command]
pub async fn reconnect_chat_streams<R: Runtime>(
    app: AppHandle<R>,
    webview: tauri::Webview<R>,
    connection_id: String,
    channel: Option<tauri::ipc::JavaScriptChannelId>,
) -> Result<ReconnectChatStreamsResult, String> {
    let channel = channel.map(|channel| channel.channel_on(webview.clone()));
    reconnect_chat_output_for_webview(app, connection_id, channel, Some(webview)).await
}

#[cfg(test)]
async fn reconnect_chat_output<R: Runtime>(
    app: AppHandle<R>,
    connection_id: String,
    channel: Option<Channel<StreamEvent>>,
) -> Result<ReconnectChatStreamsResult, String> {
    reconnect_chat_output_for_webview(app, connection_id, channel, None).await
}

fn is_current_webview<R: Runtime>(app: &AppHandle<R>, webview: &tauri::Webview<R>) -> bool {
    let Some(current) = app.get_webview_window(webview.label()) else {
        return false;
    };
    // Tauri 2.12.1 clones share their Arc<ResourceTable>; a replacement with the
    // same label has a different table. Drop each guard before taking the next:
    // current and caller can refer to the same non-reentrant mutex.
    let current_table = {
        let resources = current.resources_table();
        std::ptr::from_ref(&*resources) as usize
    };
    let caller_table = {
        let resources = webview.resources_table();
        std::ptr::from_ref(&*resources) as usize
    };
    current_table == caller_table
}

async fn reconnect_chat_output_for_webview<R: Runtime>(
    app: AppHandle<R>,
    connection_id: String,
    channel: Option<Channel<StreamEvent>>,
    webview: Option<tauri::Webview<R>>,
) -> Result<ReconnectChatStreamsResult, String> {
    let requests = app.state::<ActiveRequests>();
    let turns = app.state::<ActiveGroupTurns>();
    let mut keys = std::collections::HashSet::new();
    let mut group_turns = Vec::new();
    let mut active_requests = Vec::new();
    let channel = {
        let mut current = requests.1.lock().map_err(|error| error.to_string())?;
        if webview
            .as_ref()
            .is_some_and(|webview| !is_current_webview(&app, webview))
        {
            return Err("connection_stale".to_string());
        }
        let replace_connection = channel.is_some();
        if let Some(channel) = channel {
            *current = Some(ChatOutputConnection {
                id: connection_id.clone(),
                channel,
            });
        }
        let connection = current.as_ref().ok_or("connection_not_registered")?;
        if connection.id != connection_id {
            return Err("connection_stale".to_string());
        }
        let channel = connection.channel.clone();
        // A new WebView may reuse a numeric JS callback ID. Registration
        // replaces the consumer by connection identity, not by that number.
        for request in requests.0.iter() {
            if request
                .output
                .attach_connection(channel.clone(), replace_connection)?
                && request
                    .output
                    .send_activity(request.key(), request.attempt_id, true)
            {
                active_requests.push(json!({
                    "ownerType": request.key().topic.owner_type,
                    "ownerId": request.key().topic.owner_id,
                    "topicId": request.key().topic.topic_id,
                    "requestId": request.attempt_id,
                }));
            }
            keys.insert(request.key().clone());
        }
        let active_turns = turns
            .0
            .iter()
            .map(|turn| turn.key().clone())
            .collect::<Vec<_>>();
        for key in active_turns {
            if let Some(turn) = turns.attach_output(&requests, &key, channel.clone())? {
                group_turns.push(turn);
            }
        }
        channel
    };
    let db = app.state::<DbState>();
    let generations = pending_generations(&db.pool).await?;
    for generation in &generations {
        keys.insert(MessageKey::new(
            TopicKey::new(
                &generation.owner_type,
                &generation.owner_id,
                &generation.topic_id,
            ),
            &generation.msg_id,
        ));
    }
    for path in offline_completed_paths(&app, None).await? {
        match open_offline_completed(path.clone()).await {
            Ok(file) => {
                let key = file.metadata.message_key()?;
                if file.metadata.request_id == message_transport_request_id(&key) {
                    keys.insert(key);
                }
            }
            Err(error) => {
                log::warn!("[VCPClient] Keeping unreadable offline SSE file {path:?}: {error}")
            }
        }
    }
    if requests
        .1
        .lock()
        .map_err(|error| error.to_string())?
        .as_ref()
        .is_none_or(|connection| connection.id != connection_id)
    {
        return Err("connection_stale".to_string());
    }
    for key in keys {
        let app = app.clone();
        let channel = channel.clone();
        tauri::async_runtime::spawn(async move {
            let result = recover_active_generation(
                app.clone(),
                app.state::<ActiveRequests>(),
                key.topic.owner_id.clone(),
                key.topic.owner_type.clone(),
                key.topic.topic_id.clone(),
                key.msg_id.clone(),
                channel.clone(),
            )
            .await;
            if let Err(error) = result {
                // The owner sends detailed recoverable failures itself. A DB or
                // mailbox failure before a transport exists still needs an event.
                log::warn!(
                    "[VCPClient] Reconnect for {} remains pending: {error}",
                    key.msg_id
                );
            }
        });
    }
    Ok(ReconnectChatStreamsResult {
        generations,
        group_turns,
        active_requests,
    })
}

async fn pending_generations(pool: &sqlx::SqlitePool) -> Result<Vec<ActiveGeneration>, String> {
    let rows = sqlx::query(
        "SELECT ag.msg_id, ag.topic_id, ag.owner_id, ag.owner_type, ag.created_at, \
                m.agent_id, COALESCE(m.name, a.name) AS agent_name, ag.stop_requested \
         FROM active_generations ag \
         JOIN messages m ON m.owner_type = ag.owner_type \
            AND m.owner_id = ag.owner_id AND m.topic_id = ag.topic_id AND m.msg_id = ag.msg_id \
         LEFT JOIN agents a ON a.owner_type = 'agent' AND a.agent_id = m.agent_id \
         WHERE m.deleted_at IS NULL AND m.finish_reason IS NULL \
         ORDER BY ag.created_at ASC",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| e.to_string())?;

    let mut list = Vec::new();
    for row in rows {
        use sqlx::Row;
        let msg_id: String = row.get("msg_id");
        let topic_id: String = row.get("topic_id");
        let owner_id: String = row.get("owner_id");
        let owner_type: String = row.get("owner_type");
        // A replacement WebView needs pending messages even when their request
        // still runs. Recovery attaches its output instead of issuing another request.
        list.push(ActiveGeneration {
            msg_id,
            topic_id,
            owner_id,
            owner_type,
            created_at: row.get("created_at"),
            agent_id: row.get("agent_id"),
            agent_name: row.get("agent_name"),
            stop_requested: row.get("stop_requested"),
        });
    }
    Ok(list)
}

pub(crate) async fn finalize_stream_error<R: Runtime>(
    app_handle: &AppHandle<R>,
    pool: &sqlx::Pool<sqlx::Sqlite>,
    key: &MessageKey,
    partial_content: String,
    error: String,
    stream_channel: Option<Channel<StreamEvent>>,
    agent_id: Option<String>,
) -> Result<Option<String>, String> {
    let error = if error.trim().is_empty() {
        "生成意外中断"
    } else {
        error.as_str()
    };
    // The caller already owns the speaker identity. Retain the complete error
    // terminal before any database access; the finalizer's transaction alone
    // decides whether the message is pending, already settled, or removed.
    let committed = crate::vcp_modules::chat::message_service::finalize_stream_message(
        app_handle.clone(),
        pool,
        key,
        stream_error_content(&partial_content, error),
        false,
        Some("error".to_string()),
        stream_channel,
        agent_id,
    )
    .await?;
    Ok(committed
        .filter(|message| message.newly_committed)
        .map(|message| message.content))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OfflineCompletedContext {
    message_id: String,
    owner_type: String,
    owner_id: String,
    topic_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OfflineCompletedMetadata {
    schema_version: u32,
    request_id: String,
    generation: String,
    completed_at: i64,
    context: OfflineCompletedContext,
    finish_reason: Option<String>,
    error: Option<String>,
}

impl OfflineCompletedMetadata {
    fn message_key(&self) -> Result<MessageKey, String> {
        if self.schema_version != 1 || uuid::Uuid::parse_str(&self.generation).is_err() {
            return Err("Unsupported offline SSE metadata".to_string());
        }
        let context = &self.context;
        if !matches!(context.owner_type.as_str(), "agent" | "group")
            || context.owner_id.is_empty()
            || context.topic_id.is_empty()
            || context.message_id.is_empty()
        {
            return Err("Offline SSE metadata has no complete message identity".to_string());
        }
        Ok(MessageKey::new(
            TopicKey::new(&context.owner_type, &context.owner_id, &context.topic_id),
            &context.message_id,
        ))
    }

    fn file_name(&self) -> String {
        let digest = crate::vcp_modules::infra::utils::calculate_sha256(self.request_id.as_bytes());
        format!("{digest}.{}.completed", self.generation)
    }
}

struct OfflineCompletedFile {
    path: std::path::PathBuf,
    metadata: OfflineCompletedMetadata,
    content_offset: u64,
}

async fn open_offline_completed(path: std::path::PathBuf) -> Result<OfflineCompletedFile, String> {
    use tokio::io::AsyncBufReadExt;

    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|error| error.to_string())?;
    let mut reader = tokio::io::BufReader::new(file);
    let mut header = String::new();
    let content_offset = reader
        .read_line(&mut header)
        .await
        .map_err(|error| error.to_string())? as u64;
    if !header.ends_with('\n') {
        return Err("Offline SSE metadata line is incomplete".to_string());
    }
    let metadata: OfflineCompletedMetadata = serde_json::from_str(&header)
        .map_err(|error| format!("Invalid offline SSE metadata: {error}"))?;
    metadata.message_key()?;
    if path.file_name().and_then(|name| name.to_str()) != Some(metadata.file_name().as_str()) {
        return Err("Offline SSE filename does not match request generation".to_string());
    }
    Ok(OfflineCompletedFile {
        path,
        metadata,
        content_offset,
    })
}

async fn offline_completed_paths<R: Runtime>(
    app: &AppHandle<R>,
    request_id: Option<&str>,
) -> Result<Vec<std::path::PathBuf>, String> {
    let directory = app
        .path()
        .app_cache_dir()
        .map_err(|error| error.to_string())?
        .join("sse_offline_completed");
    let mut entries = match tokio::fs::read_dir(directory).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.to_string()),
    };
    let prefix = request_id.map(|request_id| {
        format!(
            "{}.",
            crate::vcp_modules::infra::utils::calculate_sha256(request_id.as_bytes())
        )
    });
    let mut paths = Vec::new();
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| error.to_string())?
    {
        let path = entry.path();
        if path
            .extension()
            .is_none_or(|extension| extension != "completed")
            || !entry
                .file_type()
                .await
                .map_err(|error| error.to_string())?
                .is_file()
            || prefix
                .as_ref()
                .is_some_and(|prefix| !entry.file_name().to_string_lossy().starts_with(prefix))
        {
            continue;
        }
        paths.push(path);
    }
    Ok(paths)
}

async fn remove_offline_completed(path: &std::path::Path) -> Result<(), String> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(target_os = "android")]
async fn read_completed_transport<R: Runtime>(
    app: &AppHandle<R>,
    request_id: &str,
    output: &RequestOutput,
) -> Result<Option<Result<VcpResponse, VcpRequestFailure>>, VcpRequestFailure> {
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    let mut files = Vec::new();
    for path in offline_completed_paths(app, Some(request_id))
        .await
        .map_err(|error| VcpRequestFailure::recoverable_helper(error, None))?
    {
        files.push(
            open_offline_completed(path)
                .await
                .map_err(|error| VcpRequestFailure::recoverable_helper(error, None))?,
        );
    }
    files.sort_by_key(|file| std::cmp::Reverse(file.metadata.completed_at));
    let Some(file) = files.into_iter().next() else {
        return Ok(None);
    };
    let mut reader = tokio::fs::File::open(&file.path)
        .await
        .map_err(|error| VcpRequestFailure::recoverable_helper(error.to_string(), None))?;
    reader
        .seek(std::io::SeekFrom::Start(file.content_offset))
        .await
        .map_err(|error| VcpRequestFailure::recoverable_helper(error.to_string(), None))?;
    let mut content = String::new();
    reader
        .read_to_string(&mut content)
        .await
        .map_err(|error| VcpRequestFailure::recoverable_helper(error.to_string(), None))?;
    output.retain_completed_file(&file);
    if let Some(error) = file.metadata.error {
        return Ok(Some(Err(VcpRequestFailure::streaming(error, content))));
    }
    let reason = file
        .metadata
        .finish_reason
        .unwrap_or_else(|| "completed".to_string());
    let aborted = reason == "cancelled_by_user";
    Ok(Some(Ok(VcpResponse {
        full_content: content,
        finish_reason: Some(reason),
        is_aborted: aborted,
    })))
}

#[cfg(target_os = "android")]
async fn checkpoint_stop_partial<R: Runtime>(
    app: &AppHandle<R>,
    context: &Option<Value>,
    message_id: &str,
    partial: Option<&str>,
) -> Result<(), VcpRequestFailure> {
    let key = message_key_from_context(context, message_id)?;
    let db = app.state::<DbState>();
    sqlx::query(
        "UPDATE active_generations SET stop_requested = 1,
         stop_partial_content = COALESCE(?, stop_partial_content)
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?",
    )
    .bind(partial)
    .bind(&key.topic.owner_type)
    .bind(&key.topic.owner_id)
    .bind(&key.topic.topic_id)
    .bind(&key.msg_id)
    .execute(&db.pool)
    .await
    .map_err(|error| {
        VcpRequestFailure::stopping(
            format!("停止待确认，暂存正文失败：{error}"),
            partial.map(str::to_string),
        )
    })?;
    Ok(())
}

/// Recovery sends authoritative content through StreamEvent, never in a second
/// internal JSON response. Only the coordination outcome needs to be returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecoveryStatus {
    Attached,
    Completed,
    Failed,
    NotFound,
}

/// Called with the message's ActiveRequestLease held. The helper file remains
/// available after read or transaction failures; only a durable DB decision consumes it.
async fn finalize_offline_completed<R: Runtime>(
    app: &AppHandle<R>,
    key: &MessageKey,
    file: OfflineCompletedFile,
    stream_channel: Channel<StreamEvent>,
) -> Result<RecoveryStatus, String> {
    use tokio::io::{AsyncReadExt, AsyncSeekExt};

    type OfflineMessageRow = (Option<String>, Option<i64>, Option<String>, bool);

    let db = app.state::<DbState>();
    let row: Option<OfflineMessageRow> = sqlx::query_as(
        "SELECT m.finish_reason, m.deleted_at, m.agent_id,
                EXISTS(SELECT 1 FROM active_generations ag
                       WHERE ag.owner_type = m.owner_type AND ag.owner_id = m.owner_id
                         AND ag.topic_id = m.topic_id AND ag.msg_id = m.msg_id)
         FROM messages m
         WHERE m.owner_type = ? AND m.owner_id = ? AND m.topic_id = ? AND m.msg_id = ?",
    )
    .bind(&key.topic.owner_type)
    .bind(&key.topic.owner_id)
    .bind(&key.topic.topic_id)
    .bind(&key.msg_id)
    .fetch_optional(&db.pool)
    .await
    .map_err(|error| error.to_string())?;
    let Some((finish_reason, deleted_at, agent_id, pending)) = row else {
        remove_offline_completed(&file.path).await?;
        return Ok(RecoveryStatus::NotFound);
    };
    if deleted_at.is_some() || finish_reason.is_some() {
        remove_offline_completed(&file.path).await?;
        return Ok(if deleted_at.is_some() {
            RecoveryStatus::NotFound
        } else {
            RecoveryStatus::Completed
        });
    }
    if !pending {
        return Err(format!(
            "Offline SSE message {} is not pending; retaining file",
            key.msg_id
        ));
    }
    let mut content = String::new();
    let mut reader = tokio::fs::File::open(&file.path)
        .await
        .map_err(|error| error.to_string())?;
    reader
        .seek(std::io::SeekFrom::Start(file.content_offset))
        .await
        .map_err(|error| error.to_string())?;
    reader
        .read_to_string(&mut content)
        .await
        .map_err(|error| error.to_string())?;
    let finish_reason = if let Some(error) = file.metadata.error.as_deref() {
        content = stream_error_content(&content, error);
        "error".to_string()
    } else {
        file.metadata
            .finish_reason
            .unwrap_or_else(|| "completed".to_string())
    };
    let committed = crate::vcp_modules::chat::message_service::finalize_recovered_stream_message(
        app.clone(),
        &db.pool,
        key,
        content,
        finish_reason.clone(),
        stream_channel,
        agent_id,
    )
    .await?;
    remove_offline_completed(&file.path).await?;
    Ok(if committed.is_some() {
        RecoveryStatus::Completed
    } else {
        RecoveryStatus::NotFound
    })
}

async fn recover_offline_generation<R: Runtime>(
    app: &AppHandle<R>,
    key: &MessageKey,
    stream_channel: Channel<StreamEvent>,
) -> Result<Option<RecoveryStatus>, String> {
    let request_id = message_transport_request_id(key);
    let mut files = Vec::new();
    for path in offline_completed_paths(app, Some(&request_id)).await? {
        let file = open_offline_completed(path).await?;
        if file.metadata.request_id != request_id || file.metadata.message_key()? != *key {
            return Err("Offline SSE file message identity mismatch".to_string());
        }
        files.push(file);
    }
    files.sort_by_key(|file| std::cmp::Reverse(file.metadata.completed_at));
    let mut recovered = None;
    for file in files {
        let result = finalize_offline_completed(app, key, file, stream_channel.clone()).await?;
        if recovered.is_none() {
            recovered = Some(result);
        }
    }
    Ok(recovered)
}

async fn terminal_generation_result(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    key: &MessageKey,
) -> Result<RecoveryStatus, String> {
    let terminal: Option<Option<String>> = sqlx::query_scalar(
        "SELECT finish_reason FROM messages
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?
           AND deleted_at IS NULL",
    )
    .bind(&key.topic.owner_type)
    .bind(&key.topic.owner_id)
    .bind(&key.topic.topic_id)
    .bind(&key.msg_id)
    .fetch_optional(pool)
    .await
    .map_err(|error| error.to_string())?;
    Ok(match terminal {
        Some(Some(_)) => RecoveryStatus::Completed,
        Some(None) => RecoveryStatus::Failed,
        None => RecoveryStatus::NotFound,
    })
}

#[allow(clippy::too_many_arguments)]
async fn recover_active_generation<R: Runtime>(
    app: AppHandle<R>,
    active_requests: tauri::State<'_, ActiveRequests>,
    owner_id: String,
    owner_type: String,
    topic_id: String,
    msg_id: String,
    mut stream_channel: Channel<StreamEvent>,
) -> Result<RecoveryStatus, String> {
    log::info!(
        "[VCPClient] recover_active_generation called for {}/{}/{}/{}",
        owner_type,
        owner_id,
        topic_id,
        msg_id,
    );
    let key = MessageKey::new(TopicKey::new(&owner_type, &owner_id, &topic_id), &msg_id);

    // Keep the live attempt and replace only its consumer. If finalization won
    // the output lock first, wait for that lease to leave before reading its
    // durable result; do not claim a second attempt beside a closing one.
    let (_recovery_lease, _recovery_cancellation_token) = loop {
        {
            let current = active_requests
                .1
                .lock()
                .map_err(|error| error.to_string())?;
            if let Some(channel) = current.as_ref() {
                stream_channel = channel.channel.clone();
            }
        }
        match ActiveRequestLease::try_acquire_recovery(active_requests.0.clone(), key.clone()) {
            Ok(claim) => break claim,
            Err(_) => {
                let Some(entry) = active_requests
                    .0
                    .get(&key)
                    .map(|entry| entry.value().clone())
                else {
                    continue;
                };
                let attached = {
                    let current = active_requests
                        .1
                        .lock()
                        .map_err(|error| error.to_string())?;
                    if let Some(channel) = current.as_ref() {
                        stream_channel = channel.channel.clone();
                    }
                    entry.output.attach(stream_channel.clone())?
                };
                if attached {
                    let db = app.state::<DbState>();
                    let pending: bool = sqlx::query_scalar(
                        "SELECT EXISTS(SELECT 1 FROM active_generations
                         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?)",
                    )
                    .bind(&key.topic.owner_type)
                    .bind(&key.topic.owner_id)
                    .bind(&key.topic.topic_id)
                    .bind(&key.msg_id)
                    .fetch_one(&db.pool)
                    .await
                    .map_err(|error| error.to_string())?;
                    if pending {
                        entry.output.send(recovery_state_event(
                            &key,
                            if entry.cancellation_token.is_cancelled() {
                                "stopping"
                            } else {
                                "connected"
                            },
                        ));
                    }
                    return Ok(RecoveryStatus::Attached);
                }
                entry.released.cancelled().await;
            }
        }
    };
    let output = request_output(&app, &key, Some(stream_channel.clone()));
    let result = async {

    if let Some(terminal) = output.pending_terminal() {
        let db = app.state::<DbState>();
        let committed = crate::vcp_modules::chat::message_service::retry_stream_terminal(
            app.clone(), &db.pool, &key, terminal, Some(stream_channel.clone()),
        ).await?;
        return Ok(if committed.is_some() { RecoveryStatus::Completed } else { RecoveryStatus::NotFound });
    }

    if let Some(recovered) = recover_offline_generation(&app, &key, stream_channel.clone()).await? {
        return Ok(recovered);
    }

    let db = app.state::<DbState>();
    type PendingGenerationRow = (Option<String>, bool, Option<String>, i64, Option<String>);
    let pending: Option<PendingGenerationRow> = sqlx::query_as(
        "SELECT m.agent_id, ag.stop_requested, ag.stop_partial_content, ag.created_at, m.name \
         FROM active_generations ag \
         JOIN messages m ON m.owner_type = ag.owner_type AND m.owner_id = ag.owner_id \
            AND m.topic_id = ag.topic_id AND m.msg_id = ag.msg_id \
         WHERE ag.owner_type = ? AND ag.owner_id = ? AND ag.topic_id = ? AND ag.msg_id = ? \
           AND m.deleted_at IS NULL AND m.finish_reason IS NULL",
    )
    .bind(&owner_type)
    .bind(&owner_id)
    .bind(&topic_id)
    .bind(&msg_id)
    .fetch_optional(&db.pool)
    .await
    .map_err(|e| e.to_string())?;

    let Some((agent_id, stop_requested, stop_partial, created_at, agent_name)) = pending else {
        return terminal_generation_result(&db.pool, &key).await;
    };
    let mut event = recovery_state_event(&key, if stop_requested { "stopping" } else { "reconnecting" });
    event.timestamp = Some(created_at.max(0) as u64);
    if let Some(context) = event.context.as_mut() {
        context["agentId"] = json!(agent_id);
        context["agentName"] = json!(agent_name);
    }
    event.content = stop_partial.clone();
    output.send(event);
    if stop_requested {
        _recovery_cancellation_token.cancel();
    }

    #[cfg(target_os = "android")]
    {
        return resume_claimed_generation(
            &app,
            &key,
            agent_id,
            stream_channel,
            _recovery_cancellation_token.clone(),
        )
        .await;
    }

    #[cfg(not(target_os = "android"))]
    {
        if stop_requested {
            let committed = crate::vcp_modules::chat::message_service::finalize_stream_message(
                app.clone(), &db.pool, &key, stop_partial.unwrap_or_default(), true,
                Some("cancelled_by_user".to_string()), Some(stream_channel.clone()), agent_id,
            ).await?;
            return Ok(if committed.is_some() { RecoveryStatus::Completed } else { RecoveryStatus::NotFound });
        }

        log::warn!(
        "[VCPClient] Active generation {} not found in active_requests and no local cache available. Marking as failed.",
        msg_id
    );

        let _ = finalize_stream_error(
            &app,
            &db.pool,
            &key,
            String::new(),
            "后台进程已被系统销毁，流式对话中断".to_string(),
            Some(stream_channel.clone()),
            agent_id,
        )
        .await?;

        Ok(RecoveryStatus::Failed)
    }
    }.await;
    match &result {
        Ok(RecoveryStatus::Completed | RecoveryStatus::Failed) => {
            // A terminal row can win just before this connection attaches. Its
            // durable result is delivered through the same channel, once.
            if let Err(error) = emit_persisted_terminal(&app, &key, &output).await {
                output.send(StreamEvent::error(
                    key.msg_id.clone(),
                    recovery_state_event(&key, "error").context,
                    error.clone(),
                ));
                return Err(error);
            }
        }
        Err(error) => {
            let mut event = StreamEvent::error(
                key.msg_id.clone(),
                recovery_state_event(&key, "error").context,
                error.clone(),
            );
            if _recovery_cancellation_token.is_cancelled() {
                event.r#type = "stopping".to_string();
            }
            output.send(event);
        }
        _ => {}
    }
    result
}

fn recovery_state_event(key: &MessageKey, state: &str) -> StreamEvent {
    StreamEvent {
        r#type: state.to_string(),
        message_id: key.msg_id.clone(),
        context: Some(json!({
            "ownerType": key.topic.owner_type, "ownerId": key.topic.owner_id,
            "topicId": key.topic.topic_id,
        })),
        ..Default::default()
    }
}

async fn emit_persisted_terminal<R: Runtime>(
    app: &AppHandle<R>,
    key: &MessageKey,
    output: &RequestOutput,
) -> Result<(), String> {
    let db = app.state::<DbState>();
    type PersistedTerminalRow = (String, String, i64, Option<String>, Option<String>, i64);
    let row: Option<PersistedTerminalRow> = sqlx::query_as(
        "SELECT m.content, m.finish_reason, m.timestamp, m.agent_id, m.name, t.updated_at
         FROM messages m JOIN topics t ON t.owner_type = m.owner_type AND t.owner_id = m.owner_id
           AND t.topic_id = m.topic_id
         WHERE m.owner_type = ? AND m.owner_id = ? AND m.topic_id = ? AND m.msg_id = ?
           AND m.deleted_at IS NULL AND m.finish_reason IS NOT NULL",
    )
    .bind(&key.topic.owner_type)
    .bind(&key.topic.owner_id)
    .bind(&key.topic.topic_id)
    .bind(&key.msg_id)
    .fetch_optional(&db.pool)
    .await
    .map_err(|error| error.to_string())?;
    if let Some((content, reason, timestamp, agent_id, name, topic_updated_at)) = row {
        let mut context = recovery_state_event(key, "end").context;
        if let Some(context) = context.as_mut() {
            context["agentId"] = json!(agent_id);
            context["agentName"] = json!(name);
            context["replayed"] = json!(true);
        }
        output.send(StreamEvent::end(
            key.msg_id.clone(),
            context,
            Some(reason),
            Some(content),
            None,
            Some(timestamp.max(0) as u64),
            Some(topic_updated_at),
        ));
    }
    Ok(())
}

#[cfg(any(target_os = "android", test))]
async fn handle_recovery_failure<R: Runtime>(
    app: &AppHandle<R>,
    key: &MessageKey,
    failure: VcpRequestFailure,
    stream_channel: Channel<StreamEvent>,
    agent_id: Option<String>,
) -> Result<RecoveryStatus, String> {
    let recovery_pending = failure.is_recovery_pending();
    let stop_pending = failure.is_stop_pending();
    let (error, partial_content) = failure.into_parts();
    if error == HELPER_SESSION_NOT_FOUND {
        // Publication and live-session removal are one helper transition.
        // Even a recoverable transport failure can confirm this specific
        // absence; recheck the mailbox before declaring the owner lost.
        if let Some(recovered) =
            recover_offline_generation(app, key, stream_channel.clone()).await?
        {
            return Ok(recovered);
        }
        let db = app.state::<DbState>();
        let stop: Option<Option<String>> = sqlx::query_scalar(
            "SELECT stop_partial_content FROM active_generations
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ? AND stop_requested = 1",
        )
        .bind(&key.topic.owner_type).bind(&key.topic.owner_id).bind(&key.topic.topic_id).bind(&key.msg_id)
        .fetch_optional(&db.pool).await.map_err(|error| error.to_string())?;
        if let Some(stored_partial) = stop {
            let committed = crate::vcp_modules::chat::message_service::finalize_stream_message(
                app.clone(),
                &db.pool,
                key,
                partial_content.or(stored_partial).unwrap_or_default(),
                true,
                Some("cancelled_by_user".to_string()),
                Some(stream_channel),
                agent_id,
            )
            .await?;
            return Ok(if committed.is_some() {
                RecoveryStatus::Completed
            } else {
                RecoveryStatus::NotFound
            });
        }
    } else if recovery_pending || error == STREAM_CAPACITY_EXHAUSTED {
        // The helper still owns the complete response. A local attachment or
        // admission failure must not commit our absent or incomplete snapshot.
        // Notify the current consumer as well: it may have attached from a
        // replacement WebView that cannot observe this invoke's rejection.
        let context = Some(json!({
            "ownerType": key.topic.owner_type,
            "ownerId": key.topic.owner_id,
            "topicId": key.topic.topic_id,
        }));
        let mut event = StreamEvent::error(key.msg_id.clone(), context, error.clone());
        event.content = partial_content;
        if stop_pending {
            event.r#type = "stopping".to_string();
        }
        deliver_request_event(app, key, Some(stream_channel), event);
        return Err(error);
    }

    let partial_content = partial_content.unwrap_or_default();
    let error = if error == HELPER_SESSION_NOT_FOUND {
        "后台进程已被系统销毁，流式对话中断".to_string()
    } else {
        format!("接续失败: {error}")
    };
    let db = app.state::<DbState>();
    let pool = &db.pool;
    let committed_content = finalize_stream_error(
        app,
        pool,
        key,
        partial_content,
        error,
        Some(stream_channel),
        agent_id,
    )
    .await?;
    if committed_content.is_none() {
        return terminal_generation_result(pool, key).await;
    }
    Ok(RecoveryStatus::Failed)
}

#[cfg(target_os = "android")]
async fn resume_claimed_generation<R: Runtime>(
    app: &AppHandle<R>,
    key: &MessageKey,
    agent_id: Option<String>,
    stream_channel: Channel<StreamEvent>,
    cancellation_token: CancellationToken,
) -> Result<RecoveryStatus, String> {
    let msg_id = key.msg_id.clone();
    let topic_id = key.topic.topic_id.clone();
    let owner_id = key.topic.owner_id.clone();
    let owner_type = key.topic.owner_type.clone();
    log::info!(
        "[VCPClient] Resuming helper snapshot for messageId: {}, topicId: {}",
        msg_id,
        topic_id
    );

    // 共享 ChatStream 画像 Client（见 http_clients.rs；克隆仅复制 Arc 句柄）
    let client = super::http_clients::client(super::http_clients::HttpProfile::ChatStream).clone();

    let pool = app.state::<DbState>().pool.clone();
    let transport_request_id = message_transport_request_id(key);

    let context = json!({
        "topicId": topic_id,
        "ownerId": owner_id,
        "ownerType": owner_type,
        "groupId": if owner_type == "group" { Some(&owner_id) } else { None },
        "agentId": if owner_type == "agent" { Some(&owner_id) } else { None },
    });

    let response = match handle_streaming_request(
        app,
        client,
        "",
        "",
        None,
        msg_id.clone(),
        transport_request_id,
        Some(context.clone()),
        cancellation_token,
        request_output(app, key, Some(stream_channel.clone())),
        true,
    )
    .await
    {
        Ok(val) => val,
        Err(failure) => {
            log::error!(
                "[VCPClient] Claimed recovery failed during handle_streaming_request: {}",
                failure
            );
            return handle_recovery_failure(app, key, failure, stream_channel, agent_id).await;
        }
    };

    log::info!("[VCPClient] Claimed recovery completed. Finalizing message.");
    let committed = crate::vcp_modules::chat::message_service::finalize_stream_message(
        app.clone(),
        &pool,
        key,
        response.full_content,
        response.is_aborted,
        response.finish_reason,
        Some(stream_channel),
        agent_id,
    )
    .await?;

    Ok(if committed.is_some() {
        RecoveryStatus::Completed
    } else {
        RecoveryStatus::NotFound
    })
}

#[cfg(test)]
mod offline_completed_tests {
    use super::*;

    async fn fixture() -> (DbState, MessageKey, tempfile::TempDir, std::path::PathBuf) {
        fixture_with_options(sqlx::sqlite::SqlitePoolOptions::new().max_connections(1)).await
    }

    async fn fixture_with_options(
        options: sqlx::sqlite::SqlitePoolOptions,
    ) -> (DbState, MessageKey, tempfile::TempDir, std::path::PathBuf) {
        let pool = options
            .connect("sqlite::memory:")
            .await
            .expect("test database");
        sqlx::raw_sql(include_str!("../../../migrations/0100_baseline_v2.sql"))
            .execute(&pool)
            .await
            .expect("schema");
        sqlx::raw_sql(include_str!(
            "../../../migrations/0102_stream_stop_intent.sql"
        ))
        .execute(&pool)
        .await
        .expect("stop intent schema");
        sqlx::raw_sql(
            "INSERT INTO agents (owner_type, agent_id, name, model, updated_at)
             VALUES ('agent', 'offline-agent', 'Agent', 'test', 1);
             INSERT INTO topics (topic_id, owner_type, owner_id, title, created_at, updated_at)
             VALUES ('offline-topic', 'agent', 'offline-agent', 'Topic', 1, 1);",
        )
        .execute(&pool)
        .await
        .expect("conversation");
        let db = DbState::new(pool, std::path::PathBuf::new());
        let key = MessageKey::new(
            TopicKey::new("agent", "offline-agent", "offline-topic"),
            "offline-message",
        );
        crate::vcp_modules::chat::message_service::begin_stream_message(
            &db,
            &key,
            Some("offline-agent"),
            Some("Agent"),
        )
        .await
        .expect("pending message");
        tokio::fs::create_dir_all("target/test-fixtures")
            .await
            .expect("fixture root");
        let directory = tempfile::Builder::new()
            .prefix("offline-sse-")
            .tempdir_in("target/test-fixtures")
            .expect("fixture directory");
        let request_id = message_transport_request_id(&key);
        let metadata = json!({
            "schemaVersion": 1,
            "requestId": request_id,
            "generation": uuid::Uuid::new_v4().to_string(),
            "completedAt": 42,
            "context": {
                "messageId": key.msg_id,
                "ownerType": key.topic.owner_type,
                "ownerId": key.topic.owner_id,
                "topicId": key.topic.topic_id,
            },
            "finishReason": "stop",
        });
        let parsed: OfflineCompletedMetadata =
            serde_json::from_value(metadata.clone()).expect("metadata");
        let path = directory.path().join(parsed.file_name());
        tokio::fs::write(
            &path,
            format!("{metadata}\n第一行 🦀\n{{\"正文不是JSON\":true}}\n"),
        )
        .await
        .expect("offline file");
        (db, key, directory, path)
    }

    fn channel() -> (
        Channel<StreamEvent>,
        tokio::sync::mpsc::UnboundedReceiver<Value>,
    ) {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        (
            Channel::new(move |body| {
                let _ = sender.send(body.deserialize::<Value>()?);
                Ok(())
            }),
            receiver,
        )
    }

    async fn reject_terminal_commits(pool: &sqlx::SqlitePool) {
        sqlx::raw_sql(
            "CREATE TRIGGER reject_terminal_commit BEFORE UPDATE OF finish_reason ON messages
             WHEN NEW.finish_reason IS NOT NULL
             BEGIN SELECT RAISE(ABORT, 'forced terminal commit failure'); END;",
        )
        .execute(pool)
        .await
        .unwrap();
    }

    async fn receive_event(
        events: &mut tokio::sync::mpsc::UnboundedReceiver<Value>,
        event_type: &str,
    ) -> Value {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let event = events
                    .recv()
                    .await
                    .expect("stream channel remains available");
                if event["type"] == event_type {
                    return event;
                }
            }
        })
        .await
        .expect("stream event")
    }

    #[tokio::test]
    async fn failed_terminal_commit_recovers_its_body_and_reason_on_a_new_connection() {
        for (aborted, reason) in [(false, "length"), (true, "cancelled_by_user")] {
            let (db, key, _directory, _path) = fixture().await;
            let pool = db.pool.clone();
            let app = tauri::test::mock_app();
            app.manage(db);
            app.manage(ActiveRequests::default());
            app.manage(ActiveGroupTurns::default());
            let requests = app.state::<ActiveRequests>();
            let (lease, cancellation) =
                ActiveRequestLease::try_acquire(requests.0.clone(), key.clone()).unwrap();
            let output = Arc::downgrade(&lease.entry.output);
            let (original, mut original_events) = channel();
            lease
                .bind_output(app.handle(), Some(original.clone()))
                .unwrap();
            if aborted {
                cancellation.cancel();
            }
            reject_terminal_commits(&pool).await;
            let content = "已收到的回答 🦀\n\n```rust\nlet x = 1;\n```";
            let error = crate::vcp_modules::chat::message_service::finalize_stream_message(
                app.handle().clone(),
                &pool,
                &key,
                content.into(),
                aborted,
                Some(reason.into()),
                Some(original),
                Some("offline-agent".into()),
            )
            .await
            .expect_err("the injected transaction failure must surface");
            assert!(error.contains("forced terminal commit failure"));
            assert_eq!(
                receive_event(&mut original_events, "error").await["content"],
                content
            );
            drop(lease);
            let unfinished: (String, Option<String>) =
                sqlx::query_as("SELECT content, finish_reason FROM messages")
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(
                unfinished,
                (String::new(), None),
                "the failed transaction must not claim persistence"
            );
            sqlx::query("DROP TRIGGER reject_terminal_commit")
                .execute(&pool)
                .await
                .unwrap();

            let (replacement, mut events) = channel();
            let receipt = reconnect_chat_output(
                app.handle().clone(),
                "replacement".into(),
                Some(replacement),
            )
            .await
            .unwrap();
            assert_eq!(receipt.generations.len(), 1);
            let end = receive_event(&mut events, "end").await;
            assert_eq!(end["content"], content);
            assert_eq!(end["finishReason"], reason);
            assert_eq!(end["context"]["replayed"], true);
            let durable: (String, String) =
                sqlx::query_as("SELECT content, finish_reason FROM messages")
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(durable, (content.into(), reason.into()));
            assert_eq!(
                sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM active_generations")
                    .fetch_one(&pool)
                    .await
                    .unwrap(),
                0
            );
            tokio::time::timeout(Duration::from_secs(5), async {
                while output.upgrade().is_some() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("committed terminal owner must be released");
            while let Ok(event) = original_events.try_recv() {
                assert_ne!(event["type"], "end");
            }
        }
    }

    #[tokio::test]
    async fn failed_error_query_retains_group_body_reason_and_speaker_for_recovery() {
        let (db, _, _directory, _path) = fixture_with_options(
            sqlx::sqlite::SqlitePoolOptions::new()
                .max_connections(1)
                .acquire_timeout(Duration::from_millis(100)),
        )
        .await;
        let pool = db.pool.clone();
        sqlx::raw_sql(
            "INSERT INTO groups (owner_type, group_id, name, updated_at)
             VALUES ('group', 'error-group', 'Group', 1);
             INSERT INTO topics (topic_id, owner_type, owner_id, title, created_at, updated_at)
             VALUES ('error-topic', 'group', 'error-group', 'Topic', 1, 1);",
        )
        .execute(&pool)
        .await
        .unwrap();
        let key = MessageKey::new(
            TopicKey::new("group", "error-group", "error-topic"),
            "error-message",
        );
        crate::vcp_modules::chat::message_service::begin_stream_message(
            &db,
            &key,
            Some("offline-agent"),
            Some("Agent"),
        )
        .await
        .unwrap();
        let app = tauri::test::mock_app();
        app.manage(db);
        app.manage(ActiveRequests::default());
        app.manage(ActiveGroupTurns::default());
        let (lease, _) =
            ActiveRequestLease::try_acquire(app.state::<ActiveRequests>().0.clone(), key.clone())
                .unwrap();
        let output = Arc::downgrade(&lease.entry.output);
        let (original, mut original_events) = channel();
        lease
            .bind_output(app.handle(), Some(original.clone()))
            .unwrap();

        // Exhaust the pool before the error entry point's first database access.
        let connection = pool.acquire().await.unwrap();
        let partial = "网络断开前已收到的群回复 🦀";
        let expected = format!("{partial}\n\n> VCP流式错误: connection reset");
        let error = finalize_stream_error(
            app.handle(),
            &pool,
            &key,
            partial.into(),
            "connection reset".into(),
            Some(original),
            Some("offline-agent".into()),
        )
        .await
        .expect_err("the first database access must time out");
        assert!(
            error.contains("timed out"),
            "unexpected database failure: {error}"
        );
        drop(lease);
        drop(connection);
        assert!(
            output.upgrade().is_some(),
            "failed error finalization must retain its terminal owner"
        );
        assert_eq!(
            receive_event(&mut original_events, "error").await["content"],
            expected
        );

        let (replacement, mut events) = channel();
        let recovered = recover_active_generation(
            app.handle().clone(),
            app.state::<ActiveRequests>(),
            key.topic.owner_id.clone(),
            key.topic.owner_type.clone(),
            key.topic.topic_id.clone(),
            key.msg_id.clone(),
            replacement,
        )
        .await
        .unwrap();
        assert!(matches!(recovered, RecoveryStatus::Completed));
        let end = receive_event(&mut events, "end").await;
        assert_eq!(end["content"], expected);
        assert_eq!(end["finishReason"], "error");
        assert_eq!(end["context"]["agentId"], "offline-agent");
        assert_eq!(end["context"]["replayed"], true);
        let (content, reason, agent_id, name, timestamp, hash): (
            String,
            String,
            String,
            String,
            i64,
            String,
        ) = sqlx::query_as(
            "SELECT content, finish_reason, agent_id, name, timestamp, content_hash
                 FROM messages WHERE msg_id = 'error-message'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(content, expected);
        assert_eq!(reason, "error");
        assert_eq!(agent_id, "offline-agent");
        assert_eq!(
            hash,
            crate::vcp_modules::sync_hash::HashAggregator::compute_message_fingerprint(
                &key.msg_id,
                "assistant",
                Some(&name),
                &content,
                timestamp as u64,
                Some(&agent_id),
                &[],
            )
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM active_generations WHERE msg_id = 'error-message'",
            )
            .fetch_one(&pool)
            .await
            .unwrap(),
            0
        );
        assert!(
            output.upgrade().is_none(),
            "committed terminal owner must be released"
        );
    }

    #[tokio::test]
    async fn late_error_does_not_advance_a_settled_or_deleted_message() {
        for deleted in [false, true] {
            let (db, key, _directory, _path) = fixture().await;
            let pool = db.pool.clone();
            let app = tauri::test::mock_app();
            app.manage(db);
            app.manage(ActiveRequests::default());
            crate::vcp_modules::chat::message_service::finalize_stream_message(
                app.handle().clone(),
                &pool,
                &key,
                "already completed".into(),
                false,
                Some("stop".into()),
                None,
                Some("offline-agent".into()),
            )
            .await
            .unwrap();
            if deleted {
                sqlx::query("UPDATE messages SET deleted_at = 42")
                    .execute(&pool)
                    .await
                    .unwrap();
            }
            let (lease, _) = ActiveRequestLease::try_acquire(
                app.state::<ActiveRequests>().0.clone(),
                key.clone(),
            )
            .unwrap();
            let output = Arc::downgrade(&lease.entry.output);
            let (replacement, mut events) = channel();
            lease
                .bind_output(app.handle(), Some(replacement.clone()))
                .unwrap();
            let result = finalize_stream_error(
                app.handle(),
                &pool,
                &key,
                "late partial".into(),
                "late failure".into(),
                Some(replacement),
                Some("offline-agent".into()),
            )
            .await
            .unwrap();
            assert!(
                result.is_none(),
                "late errors must not count as a new group speaker completion"
            );
            drop(lease);
            assert!(output.upgrade().is_none());
            let row: (String, String, Option<i64>) =
                sqlx::query_as("SELECT content, finish_reason, deleted_at FROM messages")
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(
                row,
                (
                    "already completed".into(),
                    "stop".into(),
                    deleted.then_some(42)
                )
            );
            while let Ok(event) = events.try_recv() {
                assert_ne!(event["type"], "error");
                if event["type"] == "end" {
                    assert!(!deleted, "deleted messages must not be replayed");
                    assert_eq!(event["content"], "already completed");
                    assert_eq!(event["context"]["replayed"], true);
                }
            }
        }
    }

    #[tokio::test]
    async fn stop_after_repeated_commit_failure_settles_the_received_terminal() {
        let (db, key, _directory, _path) = fixture().await;
        let pool = db.pool.clone();
        let app = tauri::test::mock_app();
        app.manage(db);
        app.manage(ActiveRequests::default());
        app.manage(ActiveGroupTurns::default());
        let requests = app.state::<ActiveRequests>();
        let (lease, _) = ActiveRequestLease::try_acquire(requests.0.clone(), key.clone()).unwrap();
        reject_terminal_commits(&pool).await;
        assert!(
            crate::vcp_modules::chat::message_service::finalize_stream_message(
                app.handle().clone(),
                &pool,
                &key,
                "complete response".into(),
                false,
                Some("completed".into()),
                None,
                Some("offline-agent".into()),
            )
            .await
            .is_err()
        );
        drop(lease);
        let (replacement, mut events) = channel();
        *requests.1.lock().unwrap() = Some(ChatOutputConnection {
            id: "replacement".into(),
            channel: replacement.clone(),
        });
        assert!(recover_active_generation(
            app.handle().clone(),
            app.state::<ActiveRequests>(),
            key.topic.owner_id.clone(),
            key.topic.owner_type.clone(),
            key.topic.topic_id.clone(),
            key.msg_id.clone(),
            replacement,
        )
        .await
        .is_err());
        assert_eq!(
            receive_event(&mut events, "error").await["content"],
            "complete response"
        );
        sqlx::query("DROP TRIGGER reject_terminal_commit")
            .execute(&pool)
            .await
            .unwrap();
        // This is the same durable intent / scheduling boundary used by the
        // stop commands when no producer lease remains.
        sqlx::query("UPDATE active_generations SET stop_requested = 1")
            .execute(&pool)
            .await
            .unwrap();
        schedule_pending_stop(app.handle(), &requests, key);
        let end = receive_event(&mut events, "end").await;
        assert_eq!(end["content"], "complete response");
        assert_eq!(
            end["finishReason"], "completed",
            "stop cannot rewrite a result already received"
        );
        let durable: (String, String) =
            sqlx::query_as("SELECT content, finish_reason FROM messages")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(durable, ("complete response".into(), "completed".into()));
    }

    #[tokio::test]
    async fn stop_waits_for_a_producer_that_leaves_a_failed_terminal() {
        let (db, key, _directory, _path) = fixture().await;
        let pool = db.pool.clone();
        let app = tauri::test::mock_app();
        app.manage(db);
        app.manage(ActiveRequests::default());
        let requests = app.state::<ActiveRequests>();
        let (lease, _) = ActiveRequestLease::try_acquire(requests.0.clone(), key.clone()).unwrap();
        let (consumer, mut events) = channel();
        *requests.1.lock().unwrap() = Some(ChatOutputConnection {
            id: "current".into(),
            channel: consumer,
        });
        reject_terminal_commits(&pool).await;
        assert!(
            crate::vcp_modules::chat::message_service::finalize_stream_message(
                app.handle().clone(),
                &pool,
                &key,
                "received before stop".into(),
                false,
                Some("length".into()),
                None,
                Some("offline-agent".into()),
            )
            .await
            .is_err()
        );
        sqlx::query("UPDATE active_generations SET stop_requested = 1")
            .execute(&pool)
            .await
            .unwrap();
        schedule_pending_stop(app.handle(), &requests, key);
        sqlx::query("DROP TRIGGER reject_terminal_commit")
            .execute(&pool)
            .await
            .unwrap();
        drop(lease);
        let end = receive_event(&mut events, "end").await;
        assert_eq!(end["content"], "received before stop");
        assert_eq!(end["finishReason"], "length");
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM active_generations")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn deleting_a_failed_terminal_releases_it_without_replaying_the_body() {
        for release_producer_first in [false, true] {
            let (db, key, _directory, _path) = fixture().await;
            let pool = db.pool.clone();
            let app = tauri::test::mock_app();
            app.manage(db);
            app.manage(ActiveRequests::default());
            let requests = app.state::<ActiveRequests>();
            let (lease, _) =
                ActiveRequestLease::try_acquire(requests.0.clone(), key.clone()).unwrap();
            let output = Arc::downgrade(&lease.entry.output);
            let (consumer, mut events) = channel();
            reject_terminal_commits(&pool).await;
            assert!(
                crate::vcp_modules::chat::message_service::finalize_stream_message(
                    app.handle().clone(),
                    &pool,
                    &key,
                    "discarded answer".into(),
                    false,
                    Some("completed".into()),
                    Some(consumer),
                    Some("offline-agent".into()),
                )
                .await
                .is_err()
            );
            assert_eq!(
                receive_event(&mut events, "error").await["content"],
                "discarded answer"
            );
            let mut lease = Some(lease);
            if release_producer_first {
                drop(lease.take());
            }
            let deletion = crate::vcp_modules::chat::message_service::delete_messages(
                &app.state::<DbState>(),
                &key.topic,
                vec![key.msg_id.clone()],
                None,
            )
            .await
            .unwrap();
            assert_eq!(deletion.active_ids, vec![key.msg_id.clone()]);
            discard_deleted_generation(app.handle(), &key).await;
            drop(lease);
            assert!(
                output.upgrade().is_none(),
                "deleted terminal must have no retained owner"
            );
            sqlx::query("DROP TRIGGER reject_terminal_commit")
                .execute(&pool)
                .await
                .unwrap();
            let (replacement, mut replacement_events) = channel();
            assert_eq!(
                recover_active_generation(
                    app.handle().clone(),
                    app.state::<ActiveRequests>(),
                    key.topic.owner_id.clone(),
                    key.topic.owner_type.clone(),
                    key.topic.topic_id.clone(),
                    key.msg_id.clone(),
                    replacement,
                )
                .await
                .unwrap(),
                RecoveryStatus::NotFound
            );
            while let Ok(event) = replacement_events.try_recv() {
                assert_ne!(event["type"], "end");
            }
            assert!(
                sqlx::query_scalar::<_, Option<i64>>("SELECT deleted_at FROM messages")
                    .fetch_one(&pool)
                    .await
                    .unwrap()
                    .is_some()
            );
        }
    }

    #[tokio::test]
    async fn preparing_owner_reconnects_without_creating_a_pending_message() {
        let (db, key, _directory, _path) = fixture().await;
        sqlx::query("DELETE FROM active_generations")
            .execute(&db.pool)
            .await
            .unwrap();
        let app = tauri::test::mock_app();
        app.manage(db);
        app.manage(ActiveRequests::default());
        app.manage(ActiveGroupTurns::default());
        let requests = app.state::<ActiveRequests>();
        let (lease, _) = ActiveRequestLease::try_acquire(requests.0.clone(), key).unwrap();
        lease.bind_output(app.handle(), Some(channel().0)).unwrap();
        let (replacement, mut events) = channel();
        let receipt = reconnect_chat_output(
            app.handle().clone(),
            "preparing-interface".into(),
            Some(replacement),
        )
        .await
        .unwrap();
        assert!(receipt.generations.is_empty());
        assert_eq!(receipt.active_requests.len(), 1);
        tokio::task::yield_now().await;
        let active = events.recv().await.unwrap();
        assert_eq!(active["type"], "request_activity");
        assert_eq!(active["messageId"], "");
        assert_eq!(active["chunk"]["active"], true);
        drop(lease);
        let inactive = events.recv().await.unwrap();
        assert_eq!(inactive["type"], "request_activity");
        assert_eq!(inactive["chunk"]["active"], false);
        assert_eq!(
            active["context"]["requestId"],
            inactive["context"]["requestId"]
        );
        assert!(
            events.try_recv().is_err(),
            "preparation must not create a chat bubble"
        );
    }

    #[test]
    fn only_a_matching_live_tts_listener_keeps_aurora_delivery_without_a_webview() {
        let output = RequestOutput::default();
        let events = Arc::new(std::sync::Mutex::new(Vec::<serde_json::Value>::new()));
        {
            let captured = events.clone();
            let mut state = output.state.lock().unwrap();
            state.origin_user_message_id = Some("voice-user".into());
            state.chat_feed = Some(Arc::new(move |event| {
                captured
                    .lock()
                    .unwrap()
                    .push(serde_json::to_value(event).unwrap())
            }));
            state.has_chat_listener = Some(Arc::new(|origin| origin == "voice-user"));
        }
        let mut buffer = AuroraBuffer::new();
        buffer.append_chunk("后台朗读正文。");
        buffer.process_queue();
        output.send_aurora(&mut buffer, "reply", &None, None, false);
        assert_eq!(events.lock().unwrap().len(), 1);
        assert_eq!(
            events.lock().unwrap()[0]["context"]["originUserMessageId"],
            "voice-user"
        );
        output.state.lock().unwrap().has_chat_listener = Some(Arc::new(|_| false));
        buffer.append_chunk("没有朗读消费者时不构建交付。");
        buffer.process_queue();
        output.send_aurora(&mut buffer, "reply", &None, None, false);
        assert_eq!(events.lock().unwrap().len(), 1);
        assert!(output.state.lock().unwrap().snapshot_pending);
    }

    #[tokio::test]
    async fn voice_origin_follows_the_real_output_owner_not_the_fallback_channel() {
        let app = tauri::test::mock_app();
        app.manage(ActiveRequests::default());
        app.manage(ActiveGroupTurns::default());
        let (connected, mut events) = channel();
        *app.state::<ActiveRequests>().1.lock().unwrap() = Some(ChatOutputConnection {
            id: "connected".into(),
            channel: connected,
        });
        let (fallback, mut unused) = channel();
        let key = MessageKey::new(TopicKey::new("agent", "owner", "topic"), "reply");
        let (lease, _) =
            ActiveRequestLease::try_acquire(app.state::<ActiveRequests>().0.clone(), key.clone())
                .unwrap();
        lease.set_origin_user_message("voice-user").unwrap();
        lease
            .bind_output(app.handle(), Some(fallback.clone()))
            .unwrap();
        let activity = events.recv().await.unwrap();
        assert_eq!(activity["context"]["originUserMessageId"], "voice-user");
        deliver_request_event(
            app.handle(),
            &key,
            Some(fallback),
            StreamEvent::thinking(
                key.msg_id.clone(),
                Some(
                    json!({ "ownerId": "owner", "ownerType": "agent", "topicId": "topic", "agentName": "Agent" }),
                ),
            ),
        );
        let thinking = events.recv().await.unwrap();
        assert_eq!(thinking["context"]["originUserMessageId"], "voice-user");
        assert_eq!(thinking["context"]["agentName"], "Agent");
        assert!(unused.try_recv().is_err());
        drop(lease);
        assert_eq!(
            events.recv().await.unwrap()["context"]["originUserMessageId"],
            "voice-user"
        );
    }

    #[tokio::test]
    async fn stale_webview_with_the_same_label_cannot_replace_the_current_connection() {
        let previous_app = tauri::test::mock_app();
        let previous = tauri::WebviewWindowBuilder::new(
            &previous_app,
            "main",
            tauri::WebviewUrl::App("index.html".into()),
        )
        .build()
        .unwrap();
        let app = tauri::test::mock_app();
        let current = tauri::WebviewWindowBuilder::new(
            &app,
            "main",
            tauri::WebviewUrl::App("index.html".into()),
        )
        .build()
        .unwrap();
        app.manage(ActiveRequests::default());
        app.manage(ActiveGroupTurns::default());
        assert!(is_current_webview(app.handle(), current.as_ref()));
        assert!(!is_current_webview(app.handle(), previous.as_ref()));
        *app.state::<ActiveRequests>().1.lock().unwrap() = Some(ChatOutputConnection {
            id: "current-interface".into(),
            channel: channel().0,
        });
        let result = reconnect_chat_output_for_webview(
            app.handle().clone(),
            "late-old-interface".into(),
            Some(channel().0),
            Some(previous.as_ref().clone()),
        )
        .await;
        assert_eq!(result.err().as_deref(), Some("connection_stale"));
        assert_eq!(
            app.state::<ActiveRequests>()
                .1
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .id,
            "current-interface"
        );
    }

    #[tokio::test]
    async fn reconnect_returns_while_generation_runs_and_reuses_registered_consumer() {
        let (db, key, _directory, _path) = fixture().await;
        let app = tauri::test::mock_app();
        app.manage(db);
        app.manage(ActiveRequests::default());
        app.manage(ActiveGroupTurns::default());
        let requests = app.state::<ActiveRequests>();
        let (_lease, token) =
            ActiveRequestLease::try_acquire(requests.0.clone(), key.clone()).unwrap();
        let output = request_output(app.handle(), &key, None);
        let (consumer, mut events) = channel();
        let receipt = tokio::time::timeout(
            Duration::from_secs(2),
            reconnect_chat_output(
                app.handle().clone(),
                "interface-a".into(),
                Some(consumer.clone()),
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(receipt.generations.len(), 1);
        assert!(!token.is_cancelled());
        assert!(requests.0.contains_key(&key));
        let mut buffer = AuroraBuffer::new();
        buffer.append_chunk("已有正文");
        buffer.process_queue();
        output.send_aurora(&mut buffer, &key.msg_id, &None, None, false);
        let revision = output.state.lock().unwrap().consumer_revision;
        reconnect_chat_output(app.handle().clone(), "interface-a".into(), None)
            .await
            .unwrap();
        assert_eq!(output.state.lock().unwrap().consumer_revision, revision);
        buffer.append_chunk("继续");
        buffer.process_queue();
        output.send_aurora(&mut buffer, &key.msg_id, &None, None, false);
        let mut aurora = Vec::new();
        while let Ok(event) = events.try_recv() {
            if event["type"] == "aurora" {
                aurora.push(event);
            }
        }
        assert_eq!(aurora.len(), 2);
        assert_eq!(aurora[0]["aurora"]["kind"], "snapshot");
        assert_eq!(aurora[1]["aurora"]["kind"], "delta");
        assert_eq!(aurora[1]["aurora"]["chunk"], "继续");
        // Callback numbers are local to an interface. A new connection must
        // replace the consumer even when that numeric ID is reused.
        reconnect_chat_output(app.handle().clone(), "interface-b".into(), Some(consumer))
            .await
            .unwrap();
        let replacement_revision = output.state.lock().unwrap().consumer_revision;
        assert!(replacement_revision > revision);
        assert_eq!(
            reconnect_chat_output(app.handle().clone(), "interface-a".into(), None)
                .await
                .err()
                .as_deref(),
            Some("connection_stale")
        );
        assert_eq!(
            output.state.lock().unwrap().consumer_revision,
            replacement_revision
        );
    }

    #[tokio::test]
    #[cfg(not(target_os = "android"))]
    async fn cold_reconnect_settles_persisted_stop_without_error_text() {
        let (db, key, _directory, _path) = fixture().await;
        sqlx::query("UPDATE active_generations SET stop_requested = 1, stop_partial_content = ?")
            .bind("保留已经收到的正文")
            .execute(&db.pool)
            .await
            .unwrap();
        let app = tauri::test::mock_app();
        let pool = db.pool.clone();
        app.manage(db);
        app.manage(ActiveRequests::default());
        app.manage(ActiveGroupTurns::default());
        let (consumer, mut events) = channel();
        let receipt = reconnect_chat_output(
            app.handle().clone(),
            "stop-interface".into(),
            Some(consumer),
        )
        .await
        .unwrap();
        assert!(receipt.generations[0].stop_requested);
        let end = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let event = events.recv().await.unwrap();
                if event["type"] == "end" {
                    break event;
                }
                assert_eq!(event["type"], "stopping");
            }
        })
        .await
        .unwrap();
        assert_eq!(end["content"], "保留已经收到的正文");
        assert_eq!(end["finishReason"], "cancelled_by_user");
        let pending: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM active_generations")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(pending, 0);
        assert_eq!(end["messageId"], key.msg_id);
    }

    #[test]
    fn group_turn_child_cancellation_and_drop_use_the_current_consumer() {
        let app = tauri::test::mock_app();
        app.manage(ActiveRequests::default());
        app.manage(ActiveGroupTurns::default());
        let turns = app.state::<ActiveGroupTurns>();
        let requests = app.state::<ActiveRequests>();
        let key = TopicKey::new("group", "group-a", "topic-a");
        let turn = turns.try_acquire(key.clone()).unwrap();
        let (original, mut original_events) = channel();
        turn.bind_output(app.handle(), original).unwrap();
        assert_eq!(original_events.try_recv().unwrap()["chunk"]["active"], true);
        let child = turn.child_token();
        let (replacement, mut events) = channel();
        turns.attach_output(&requests, &key, replacement).unwrap();
        let active = events.try_recv().unwrap();
        turn.cancel();
        assert!(child.is_cancelled());
        drop(turn);
        let inactive = events.try_recv().unwrap();
        assert_eq!(inactive["chunk"]["active"], false);
        assert_eq!(active["context"]["turnId"], inactive["context"]["turnId"]);
        assert!(
            inactive["consumerRevision"].as_u64().unwrap()
                > active["consumerRevision"].as_u64().unwrap()
        );
        assert!(original_events.try_recv().is_err());
        assert!(turns
            .attach_output(&requests, &key, channel().0)
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn recovery_capacity_and_partial_snapshot_failures_preserve_pending_content() {
        let (db, key, _directory, path) = fixture().await;
        let pool = db.pool.clone();
        let original: (String, Option<String>) =
            sqlx::query_as("SELECT content, finish_reason FROM messages")
                .fetch_one(&pool)
                .await
                .expect("pending content");
        let app = tauri::test::mock_app();
        app.manage(db);
        app.manage(ActiveRequests::default());
        let (sent, mut events) = tokio::sync::mpsc::unbounded_channel();
        let channel = Channel::new(move |body| {
            let _ = sent.send(body.deserialize::<Value>()?);
            Ok(())
        });
        for failure in [
            VcpRequestFailure::from(STREAM_CAPACITY_EXHAUSTED.to_string()),
            VcpRequestFailure::recoverable_helper("incomplete snapshot", None),
            VcpRequestFailure::recoverable_helper(
                "local connection lost",
                Some("第一行".to_string()),
            ),
        ] {
            let requests = app.state::<ActiveRequests>();
            let (_lease, _) = ActiveRequestLease::try_acquire(requests.0.clone(), key.clone())
                .expect("recovery lease");
            let old_channel = Channel::new(|_| panic!("replaced WebView received an event"));
            let output = request_output(app.handle(), &key, Some(old_channel.clone()));
            output.attach(channel.clone()).expect("replacement WebView");
            let reason = failure.to_string();
            let expected_partial = failure.partial_content.clone();
            let result = handle_recovery_failure(
                app.handle(),
                &key,
                failure,
                old_channel,
                Some("offline-agent".into()),
            )
            .await;
            assert_eq!(
                result.expect_err("attachment failure must remain retryable"),
                reason
            );
            let after: (String, Option<String>) =
                sqlx::query_as("SELECT content, finish_reason FROM messages")
                    .fetch_one(&pool)
                    .await
                    .expect("retained content");
            assert_eq!(after, original);
            let pending: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM active_generations")
                .fetch_one(&pool)
                .await
                .expect("pending count");
            assert_eq!(pending, 1);
            assert!(tokio::fs::try_exists(&path)
                .await
                .expect("complete backup retained"));
            let event = events.try_recv().expect("replacement consumer notified");
            assert_eq!(event["type"], "error");
            assert_eq!(event["content"], json!(expected_partial));
            assert!(events.try_recv().is_err());
            drop(_lease);
            let released = events.try_recv().expect("request activity released");
            assert_eq!(released["type"], "request_activity");
            assert_eq!(released["chunk"]["active"], false);
            assert!(events.try_recv().is_err());
        }
    }

    #[tokio::test]
    async fn offline_completed_is_consumed_only_after_sqlite_commit() {
        let (db, key, _directory, path) = fixture().await;
        let pool = db.pool.clone();
        let app = tauri::test::mock_app();
        app.manage(db);
        sqlx::raw_sql(
            "CREATE TRIGGER reject_offline_commit BEFORE UPDATE ON messages
             BEGIN SELECT RAISE(ABORT, 'forced commit failure'); END;",
        )
        .execute(&pool)
        .await
        .expect("failure injection");
        let channel = Channel::new(|_| Ok(()));
        let file = open_offline_completed(path.clone())
            .await
            .expect("read offline file");
        assert!(
            finalize_offline_completed(app.handle(), &key, file, channel.clone())
                .await
                .is_err()
        );
        assert!(tokio::fs::try_exists(&path).await.expect("file remains"));
        let pending: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM active_generations")
            .fetch_one(&pool)
            .await
            .expect("pending count");
        assert_eq!(pending, 1);
        sqlx::query("DROP TRIGGER reject_offline_commit")
            .execute(&pool)
            .await
            .expect("remove failure");

        let file = open_offline_completed(path.clone())
            .await
            .expect("reread retained file");
        let (events, mut received) = tokio::sync::mpsc::unbounded_channel();
        let channel = Channel::new(move |body| {
            let _ = events.send(body.deserialize::<Value>()?);
            Ok(())
        });
        let result = finalize_offline_completed(app.handle(), &key, file, channel)
            .await
            .expect("durable recovery");
        assert_eq!(result, RecoveryStatus::Completed);
        let event = received.try_recv().expect("recovered terminal event");
        assert_eq!(event["type"], "end");
        assert_eq!(event["context"]["replayed"], true);
        assert!(!tokio::fs::try_exists(&path).await.expect("file consumed"));
        let durable: (String, Option<String>) =
            sqlx::query_as("SELECT content, finish_reason FROM messages")
                .fetch_one(&pool)
                .await
                .expect("committed message");
        assert_eq!(durable.0, "第一行 🦀\n{\"正文不是JSON\":true}\n");
        assert_eq!(durable.1.as_deref(), Some("stop"));
        let pending: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM active_generations")
            .fetch_one(&pool)
            .await
            .expect("pending count");
        assert_eq!(pending, 0);
    }

    #[tokio::test]
    async fn offline_completed_tombstone_is_cleaned_without_emitting_old_content() {
        let (db, key, _directory, path) = fixture().await;
        sqlx::raw_sql("UPDATE messages SET deleted_at = 9; DELETE FROM active_generations;")
            .execute(&db.pool)
            .await
            .expect("delete message");
        let app = tauri::test::mock_app();
        app.manage(db);
        let sent = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let event_count = sent.clone();
        let channel = Channel::new(move |_| {
            event_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        });
        let file = open_offline_completed(path.clone())
            .await
            .expect("read file");
        let result = finalize_offline_completed(app.handle(), &key, file, channel)
            .await
            .expect("discard deleted file");
        assert_eq!(result, RecoveryStatus::NotFound);
        assert_eq!(sent.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert!(!tokio::fs::try_exists(&path).await.expect("file consumed"));
        let terminal = terminal_generation_result(&app.state::<DbState>().pool, &key)
            .await
            .expect("late recovery result");
        assert_eq!(terminal, RecoveryStatus::NotFound);
    }

    #[tokio::test]
    async fn offline_completed_rejects_a_different_generation_filename() {
        let (_db, _key, directory, path) = fixture().await;
        let wrong_path = directory.path().join("different-generation.completed");
        tokio::fs::rename(path, &wrong_path)
            .await
            .expect("rename fixture");
        assert!(open_offline_completed(wrong_path.clone()).await.is_err());
        assert!(tokio::fs::try_exists(wrong_path)
            .await
            .expect("unreadable file retained"));
    }
}

#[cfg(test)]
mod active_request_tests {
    use super::*;

    #[test]
    fn sse_decoder_handles_utf8_bom_and_all_line_endings_across_byte_splits() {
        use tokio_util::codec::Decoder;
        let mut decoder = SseDecoder::default();
        let mut buffer = bytes::BytesMut::new();
        let mut events = Vec::new();
        for byte in
            "\u{feff}: heartbeat\r\ndata: first\rdata: 中国\r\rdata:\n\ndata: discarded".as_bytes()
        {
            buffer.extend_from_slice(&[*byte]);
            while let Some(event) = decoder.decode(&mut buffer).unwrap() {
                events.push(event);
            }
        }
        assert_eq!(events, ["first\n中国", ""]);
        assert!(decoder.decode_eof(&mut buffer).unwrap().is_none());
    }

    #[test]
    fn sse_decoder_keeps_large_events_and_discards_only_unterminated_events() {
        use tokio_util::codec::Decoder;
        let content = "x".repeat(600 * 1024);
        let mut decoder = SseDecoder::default();
        let mut buffer =
            bytes::BytesMut::from(format!("data: {content}\n\ndata: unfinished\n").as_bytes());
        assert_eq!(decoder.decode(&mut buffer).unwrap(), Some(content));
        assert!(decoder.decode_eof(&mut buffer).unwrap().is_none());
    }

    #[test]
    fn sse_round_finish_keeps_collecting_until_done_and_reports_upstream_errors() {
        let mut text = String::new();
        let mut reason = None;
        for data in [
            r#"{"choices":[{"delta":{"content":"before"},"finish_reason":"tool_calls"}]}"#,
            r#"{"choices":[{"delta":{"content":"after"},"finish_reason":"stop"}]}"#,
        ] {
            assert!(!absorb_sse_data(data, &mut text, &mut reason).unwrap());
        }
        assert_eq!(text, "beforeafter");
        assert_eq!(reason.as_deref(), Some("stop"));
        assert!(absorb_sse_data("[DONE]", &mut text, &mut reason).unwrap());
        assert_eq!(
            absorb_sse_data(
                r#"{"error":{"message":"quota exhausted"}}"#,
                &mut text,
                &mut reason
            ),
            Err("quota exhausted".to_string()),
        );
        assert!(absorb_sse_data("{", &mut text, &mut reason).is_err());
        assert_eq!(text, "beforeafter");
    }

    fn event_channel() -> (
        Channel<StreamEvent>,
        tokio::sync::mpsc::UnboundedReceiver<Value>,
    ) {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        let channel = Channel::new(move |body| {
            let _ = sender.send(body.deserialize::<Value>()?);
            Ok(())
        });
        (channel, receiver)
    }

    #[test]
    fn failed_consumer_is_detached_until_a_new_consumer_takes_a_snapshot() {
        let failures = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let attempts = failures.clone();
        let output = RequestOutput::default();
        output.bind_initial(Some(Channel::new(move |_| {
            attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(std::io::Error::other("WebView closed").into())
        })));
        let mut buffer = AuroraBuffer::new();
        for _ in 0..100 {
            buffer.append_chunk("正文 ");
            buffer.process_queue();
            output.send_aurora(&mut buffer, "message", &None, None, false);
            assert!(buffer.pending_mutations.is_empty());
        }
        assert_eq!(failures.load(std::sync::atomic::Ordering::SeqCst), 1);
        let (channel, mut events) = event_channel();
        assert!(output.attach(channel).unwrap());
        output.send_aurora(&mut buffer, "message", &None, None, false);
        let event = events.try_recv().unwrap();
        assert_eq!(event["aurora"]["kind"], "snapshot");
        assert_eq!(event["aurora"]["content"], "正文 ".repeat(100));
        assert!(buffer.pending_mutations.is_empty());
    }

    async fn empty_request_body() -> Option<PreparedBody> {
        Some(
            request_body::prepare(
                std::path::PathBuf::from("target/test-fixtures/http-bodies"),
                json!({}),
                CancellationToken::new(),
                request_body::PREPARATION_SLOT.acquire().await.unwrap(),
            )
            .await
            .unwrap(),
        )
    }

    #[tokio::test]
    #[cfg(not(target_os = "android"))]
    async fn sse_http_eof_flushes_multiline_rounds_without_a_done_marker() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            let received = socket.read(&mut request).await.unwrap();
            assert!(received > 0, "client closed before sending a request");
            let body = concat!(
                "data: {\"choices\":[{\"delta\":{\"content\":\"first\"},\n",
                "data: \"finish_reason\":\"tool_calls\"}]}\n\n",
                "data: {\"choices\":[{\"delta\":{\"content\":\"tail\"},\"finish_reason\":\"stop\"}]}\n\n",
            );
            socket.write_all(format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{body}",
                body.len(),
            ).as_bytes()).await.unwrap();
        });
        let app = tauri::test::mock_app();
        let (channel, mut events) = event_channel();
        let output = Arc::new(RequestOutput::default());
        output.bind_initial(Some(channel));
        let result = handle_streaming_request(
            app.handle(),
            Client::new(),
            &endpoint,
            "",
            empty_request_body().await,
            "message".into(),
            "transport".into(),
            None,
            CancellationToken::new(),
            output,
            false,
        )
        .await
        .unwrap();
        assert_eq!(result.full_content, "firsttail");
        assert_eq!(result.finish_reason.as_deref(), Some("stop"));
        assert!(!result.is_aborted);
        let mut delivered = String::new();
        while let Ok(event) = events.try_recv() {
            assert_eq!(event["aurora"]["kind"], "delta");
            if let Some(chunk) = event["aurora"]["chunk"].as_str() {
                delivered.push_str(chunk);
            }
        }
        assert_eq!(delivered, "firsttail");
        server.await.unwrap();
    }

    #[tokio::test]
    #[cfg(not(target_os = "android"))]
    async fn sse_http_read_failure_preserves_received_text_without_reconnecting() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            let received = socket.read(&mut request).await.unwrap();
            assert!(received > 0, "client closed before sending a request");
            let body = "data: {\"choices\":[{\"delta\":{\"content\":\"saved partial\"}}]}\n\n";
            socket.write_all(format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{body}\r\n",
                body.len(),
            ).as_bytes()).await.unwrap();
            socket.shutdown().await.unwrap();
            assert!(
                tokio::time::timeout(Duration::from_millis(100), listener.accept())
                    .await
                    .is_err()
            );
        });
        let app = tauri::test::mock_app();
        let failure = handle_streaming_request(
            app.handle(),
            Client::new(),
            &endpoint,
            "",
            empty_request_body().await,
            "message".into(),
            "transport".into(),
            None,
            CancellationToken::new(),
            Arc::default(),
            false,
        )
        .await
        .unwrap_err();
        let (error, partial) = failure.into_parts();
        assert!(error.starts_with("SSE stream read failed:"), "{error}");
        assert_eq!(partial.as_deref(), Some("saved partial"));
        server.await.unwrap();
    }

    #[tokio::test]
    #[cfg(not(target_os = "android"))]
    async fn idle_stream_handoff_sends_a_snapshot_without_restarting_http() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let release_server = CancellationToken::new();
        let server_release = release_server.clone();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = vec![0; 4096];
            let length = socket.read(&mut request).await.unwrap();
            assert!(std::str::from_utf8(&request[..length])
                .unwrap()
                .starts_with("POST "));
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\ndata: {\"choices\":[{\"delta\":{\"content\":\"<think>first\"}}]}\n\n").await.unwrap();
            server_release.cancelled().await;
            assert!(
                tokio::time::timeout(Duration::from_millis(10), listener.accept())
                    .await
                    .is_err()
            );
        });
        let app = tauri::test::mock_app();
        let output = Arc::new(RequestOutput::default());
        let (old_channel, mut old_events) = event_channel();
        output.bind_initial(Some(old_channel));
        let cancellation = CancellationToken::new();
        let request_app = app.handle().clone();
        let request_output = output.clone();
        let request_cancel = cancellation.clone();
        let request = tokio::spawn(async move {
            handle_streaming_request(
                &request_app,
                Client::new(),
                &endpoint,
                "",
                empty_request_body().await,
                "message".to_string(),
                "transport".to_string(),
                None,
                request_cancel,
                request_output,
                false,
            )
            .await
        });
        let first = tokio::time::timeout(Duration::from_secs(2), old_events.recv())
            .await
            .unwrap()
            .unwrap();
        let (new_channel, mut new_events) = event_channel();
        assert!(output.attach(new_channel).unwrap());
        let snapshot = tokio::time::timeout(Duration::from_secs(1), new_events.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(snapshot["aurora"]["kind"], "snapshot");
        assert_eq!(snapshot["aurora"]["content"], "<think>first");
        assert!(
            snapshot["consumerRevision"].as_u64().unwrap()
                > first["consumerRevision"].as_u64().unwrap()
        );
        assert!(old_events.try_recv().is_err());
        cancellation.cancel();
        let result = tokio::time::timeout(Duration::from_secs(1), request)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(result.is_aborted);
        assert_eq!(result.full_content, "<think>first");
        release_server.cancel();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn group_handoff_covers_the_current_speaker_and_the_gap_before_the_next() {
        let app = tauri::test::mock_app();
        app.manage(ActiveRequests::default());
        app.manage(ActiveGroupTurns::default());
        let requests = app.state::<ActiveRequests>();
        let turns = app.state::<ActiveGroupTurns>();
        let topic = TopicKey::new("group", "group", "topic");
        let _turn = turns.try_acquire(topic.clone()).unwrap();
        let first_key = MessageKey::new(topic.clone(), "first");
        let (first_lease, _) =
            ActiveRequestLease::try_acquire(requests.0.clone(), first_key.clone()).unwrap();
        let (original, mut original_events) = event_channel();
        deliver_request_event(
            app.handle(),
            &first_key,
            Some(original.clone()),
            StreamEvent::thinking("first".into(), None),
        );
        assert_eq!(original_events.recv().await.unwrap()["messageId"], "first");
        let (replacement, mut replacement_events) = event_channel();
        turns.attach_output(&requests, &topic, replacement).unwrap();
        let turn_activity = replacement_events.recv().await.unwrap();
        assert_eq!(turn_activity["type"], "group_turn");
        assert_eq!(turn_activity["chunk"]["active"], true);
        deliver_request_event(
            app.handle(),
            &first_key,
            Some(original.clone()),
            StreamEvent::end(
                "first".into(),
                None,
                Some("completed".into()),
                Some("first result".into()),
                None,
                None,
                None,
            ),
        );
        assert_eq!(replacement_events.recv().await.unwrap()["type"], "end");
        assert!(!first_lease.entry.output.attach(original.clone()).unwrap());
        let released = first_lease.entry.released.clone();
        drop(first_lease);
        assert!(released.is_cancelled());
        let released_activity = replacement_events.recv().await.unwrap();
        assert_eq!(released_activity["type"], "request_activity");
        assert_eq!(released_activity["chunk"]["active"], false);

        let (gap_replacement, mut gap_events) = event_channel();
        turns
            .attach_output(&requests, &topic, gap_replacement)
            .unwrap();
        let gap_activity = gap_events.recv().await.unwrap();
        assert_eq!(gap_activity["type"], "group_turn");
        assert_eq!(gap_activity["chunk"]["active"], true);
        let second_key = MessageKey::new(topic, "second");
        let (_second_lease, _) =
            ActiveRequestLease::try_acquire(requests.0.clone(), second_key.clone()).unwrap();
        deliver_request_event(
            app.handle(),
            &second_key,
            Some(original),
            StreamEvent::thinking("second".into(), None),
        );
        assert_eq!(gap_events.recv().await.unwrap()["messageId"], "second");
        assert!(original_events.try_recv().is_err());
        assert!(replacement_events.try_recv().is_err());
    }

    #[tokio::test]
    async fn non_streaming_http_preserves_text_and_finish_metadata() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            for reason in [Some("stop"), Some("length"), None] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0; 4096];
                assert!(socket.read(&mut request).await.unwrap() > 0);
                let body = json!({
                    "choices": [{ "message": { "content": "正文 🦀\n\"quote\"\\tail" }, "finish_reason": reason }],
                    "usage": { "completion_tokens": 7 },
                }).to_string();
                socket.write_all(format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len(),
                ).as_bytes()).await.unwrap();
            }
        });
        for reason in [Some("completed"), Some("length"), None] {
            let (channel, mut events) = event_channel();
            let output = Arc::new(RequestOutput::default());
            output.bind_initial(Some(channel));
            let response = handle_non_streaming_request(
                Client::new(),
                &endpoint,
                "",
                empty_request_body().await.unwrap(),
                "message".into(),
                Some(json!({ "topicId": "topic" })),
                CancellationToken::new(),
                output,
            )
            .await
            .unwrap();
            assert_eq!(response.full_content, "正文 🦀\n\"quote\"\\tail");
            assert_eq!(response.finish_reason.as_deref(), reason);
            assert!(!response.is_aborted);
            let event = events.recv().await.unwrap();
            assert_eq!(event["aurora"]["content"], response.full_content);
            assert_eq!(event["context"]["topicId"], "topic");
            assert!(
                events.try_recv().is_err(),
                "the application finalizer owns the terminal event"
            );
        }
        server.await.unwrap();
    }

    #[tokio::test]
    async fn non_streaming_upload_preserves_prepared_utf8_json_bytes() {
        use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
        let value = json!({"messages": [{"role": "user", "content": "中文\n\"😀".repeat(4096)}]});
        let expected = serde_json::to_vec(&value).unwrap();
        let body = request_body::prepare(
            std::path::PathBuf::from("target/test-fixtures/http-bodies"),
            value,
            CancellationToken::new(),
            request_body::PREPARATION_SLOT.acquire().await.unwrap(),
        )
        .await
        .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut reader = tokio::io::BufReader::new(socket);
            let mut length = None;
            loop {
                let mut line = String::new();
                assert!(reader.read_line(&mut line).await.unwrap() > 0);
                if line == "\r\n" {
                    break;
                }
                if let Some((name, value)) = line.split_once(':') {
                    if name.eq_ignore_ascii_case("content-length") {
                        length = Some(value.trim().parse::<usize>().unwrap());
                    }
                }
            }
            assert_eq!(length, Some(expected.len()));
            let mut received = vec![0; expected.len()];
            reader.read_exact(&mut received).await.unwrap();
            assert_eq!(received, expected);
            let response = r#"{"choices":[{"message":{"content":"ok"},"finish_reason":"stop"}]}"#;
            reader.get_mut().write_all(format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",
                response.len(),
            ).as_bytes()).await.unwrap();
        });
        let response = handle_non_streaming_request(
            Client::new(),
            &endpoint,
            "",
            body,
            "message".into(),
            None,
            CancellationToken::new(),
            Arc::default(),
        )
        .await
        .unwrap();
        assert_eq!(response.full_content, "ok");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn prepared_upload_replays_the_same_body_after_307_and_308() {
        use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
        for status in [307, 308] {
            let value = json!({"messages": [{"role": "user", "content": "正文\n😀".repeat(2048)}]});
            let expected = serde_json::to_vec(&value).unwrap();
            let body = request_body::prepare(
                std::path::PathBuf::from("target/test-fixtures/http-bodies"),
                value,
                CancellationToken::new(),
                request_body::PREPARATION_SLOT.acquire().await.unwrap(),
            )
            .await
            .unwrap();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let endpoint = format!("http://{}", listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                for redirect in [true, false] {
                    let (socket, _) = listener.accept().await.unwrap();
                    let mut reader = tokio::io::BufReader::new(socket);
                    let mut length = None;
                    loop {
                        let mut line = String::new();
                        assert!(reader.read_line(&mut line).await.unwrap() > 0);
                        if line == "\r\n" {
                            break;
                        }
                        if let Some((name, value)) = line.split_once(':') {
                            if name.eq_ignore_ascii_case("content-length") {
                                length = Some(value.trim().parse::<usize>().unwrap());
                            }
                        }
                    }
                    assert_eq!(length, Some(expected.len()));
                    let mut received = vec![0; expected.len()];
                    reader.read_exact(&mut received).await.unwrap();
                    assert_eq!(received, expected);
                    let reply = if redirect {
                        format!("HTTP/1.1 {status} Redirect\r\nLocation: /destination\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    } else {
                        let content =
                            r#"{"choices":[{"message":{"content":"ok"},"finish_reason":"stop"}]}"#;
                        format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{content}", content.len())
                    };
                    reader.get_mut().write_all(reply.as_bytes()).await.unwrap();
                }
            });
            let result = tokio::time::timeout(
                Duration::from_secs(5),
                handle_non_streaming_request(
                    Client::new(),
                    &endpoint,
                    "",
                    body,
                    "message".into(),
                    None,
                    CancellationToken::new(),
                    Arc::default(),
                ),
            )
            .await;
            if !matches!(&result, Ok(Ok(_))) {
                server.abort();
            }
            assert_eq!(result.unwrap().unwrap().full_content, "ok", "HTTP {status}");
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn cancellation_during_non_streaming_body_does_not_wait_for_body_completion() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let (headers_sent, headers_ready) = tokio::sync::oneshot::channel();
        let release_server = CancellationToken::new();
        let server_release = release_server.clone();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            let received = socket.read(&mut request).await.unwrap();
            assert!(received > 0, "client closed before sending a request");
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 1000\r\n\r\n{").await.unwrap();
            let _ = headers_sent.send(());
            server_release.cancelled().await;
        });
        let cancellation = CancellationToken::new();
        let request_cancel = cancellation.clone();
        let request = tokio::spawn(async move {
            handle_non_streaming_request(
                Client::new(),
                &endpoint,
                "",
                empty_request_body().await.unwrap(),
                "message".into(),
                None,
                request_cancel,
                Arc::default(),
            )
            .await
        });
        tokio::time::timeout(Duration::from_secs(1), headers_ready)
            .await
            .unwrap()
            .unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        cancellation.cancel();
        let result = tokio::time::timeout(Duration::from_secs(1), request)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(result.is_aborted);
        assert_eq!(result.finish_reason.as_deref(), Some("cancelled_by_user"));
        release_server.cancel();
        server.await.unwrap();
    }

    fn message_key(message_id: &str) -> MessageKey {
        MessageKey::new(TopicKey::new("agent", "agent-a", "topic-a"), message_id)
    }

    #[tokio::test]
    async fn duplicate_attempt_is_rejected_and_cancel_reaches_every_step_clone() {
        let requests = ActiveRequests::default();
        let key = message_key("message-1");
        let (lease, first_step) = ActiveRequestLease::try_acquire(requests.0.clone(), key.clone())
            .expect("first attempt should register");
        let second_step = first_step.clone();

        assert!(ActiveRequestLease::try_acquire(requests.0.clone(), key.clone()).is_err());
        assert!(requests.cancel(&key).expect("cancel should succeed"));
        assert!(requests.0.contains_key(&key));
        first_step.cancelled().await;
        second_step.cancelled().await;
        assert!(first_step.is_cancelled());
        assert!(requests
            .cancel(&key)
            .expect("repeated cancel remains idempotent"));

        drop(lease);
        assert!(!requests.0.contains_key(&key));
    }

    #[tokio::test]
    async fn cancelled_voice_admission_cannot_start_http_after_late_registration() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let app = tauri::test::mock_app();
        app.manage(ActiveRequests::default());
        let requests = app.state::<ActiveRequests>();
        let key = message_key("late-voice-reply");
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let (lease, token) = ActiveRequestLease::try_acquire_with_cancellation(
            requests.0.clone(),
            key.clone(),
            cancellation,
        )
        .unwrap();
        lease.set_origin_user_message("voice-origin").unwrap();
        lease.bind_output(app.handle(), None).unwrap();
        {
            let output = lease.entry.output.state.lock().unwrap();
            assert!(output.channel.is_none());
            assert!(
                output.chat_feed.is_some(),
                "Rust feed binds without a WebView"
            );
        }
        let result = perform_vcp_request_registered(
            app.handle(),
            VcpRequestPayload {
                vcp_url: endpoint,
                vcp_api_key: String::new(),
                messages: vec![json!({"role": "user", "content": "voice"})],
                model_config: json!({"stream": true}),
                message_id: key.msg_id.clone(),
                context: Some(json!({
                    "ownerType": key.topic.owner_type,
                    "ownerId": key.topic.owner_id,
                    "topicId": key.topic.topic_id,
                })),
                transport_request_id: None,
            },
            None,
            token,
        )
        .await
        .unwrap();
        assert!(result.is_aborted);
        assert!(
            tokio::time::timeout(Duration::from_millis(30), listener.accept())
                .await
                .is_err()
        );
    }

    #[test]
    fn voice_group_without_webview_keeps_the_feed_and_group_cancellation() {
        let app = tauri::test::mock_app();
        app.manage(ActiveRequests::default());
        app.manage(ActiveGroupTurns::default());
        let turns = app.state::<ActiveGroupTurns>();
        let topic = TopicKey::new("group", "voice-group", "voice-topic");
        let turn = turns.try_acquire(topic.clone()).unwrap();
        turn.set_origin_user_message("voice-origin").unwrap();
        turn.bind_optional_output(app.handle(), None).unwrap();
        let events = Arc::new(std::sync::Mutex::new(Vec::<StreamEvent>::new()));
        {
            let entry = turns.0.get(&topic).unwrap();
            assert!(entry.channel.lock().unwrap().is_none());
            assert!(entry.chat_feed.lock().unwrap().is_some());
            let captured = events.clone();
            *entry.chat_feed.lock().unwrap() = Some(Arc::new(move |event| {
                captured.lock().unwrap().push(event.clone());
            }));
        }
        let speaker = turn.child_token();
        turn.cancel();
        assert!(turn.is_cancelled());
        assert!(speaker.is_cancelled());
        drop(turn);
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].r#type, "group_turn");
        assert_eq!(events[0].chunk.as_ref().unwrap()["active"], false);
        assert_eq!(
            events[0].context.as_ref().unwrap()["originUserMessageId"],
            "voice-origin"
        );
        let later = turns.try_acquire(topic).unwrap();
        assert!(
            !later.is_cancelled(),
            "a later unrelated group turn owns another token"
        );
    }

    #[tokio::test]
    async fn group_release_reaches_all_waiters_even_before_their_first_poll() {
        let turns = ActiveGroupTurns::default();
        let topic = TopicKey::new("group", "group", "topic");
        let turn = turns.try_acquire(topic.clone()).unwrap();
        let first = turns.try_acquire_or_wait(topic.clone()).err().unwrap();
        let second = turns.try_acquire_or_wait(topic.clone()).err().unwrap();
        let mut first = Box::pin(first);
        let mut second = Box::pin(second);
        drop(turn);
        assert!(futures_util::poll!(&mut first).is_ready());
        assert!(futures_util::poll!(&mut second).is_ready());
        assert!(turns.try_acquire(topic).is_ok());
    }

    #[test]
    fn stale_lease_cannot_remove_a_new_attempt() {
        let requests = ActiveRequests::default();
        let key = message_key("message-2");
        let (old_lease, _old_token) =
            ActiveRequestLease::try_acquire(requests.0.clone(), key.clone())
                .expect("old attempt should register");

        let new_attempt_id = uuid::Uuid::new_v4();
        requests.0.insert(
            key.clone(),
            Arc::new(ActiveRequestEntry {
                attempt_id: new_attempt_id,
                cancellation_token: CancellationToken::new(),
                output: Arc::default(),
                released: CancellationToken::new(),
            }),
        );

        drop(old_lease);
        assert_eq!(
            requests
                .0
                .get(&key)
                .expect("new attempt must remain")
                .attempt_id,
            new_attempt_id
        );
    }
}

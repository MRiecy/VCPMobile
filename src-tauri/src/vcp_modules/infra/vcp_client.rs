use crate::vcp_modules::media_processor::convert_local_image_for_multimodal;
use dashmap::mapref::entry::Entry;
use dashmap::DashMap;
use futures_util::StreamExt;
use futures_util::TryStreamExt;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{ipc::Channel, AppHandle, Manager, Runtime};
#[cfg(target_os = "android")]
use tokio_util::codec::LengthDelimitedCodec;
use tokio_util::codec::{FramedRead, LinesCodec};
use tokio_util::io::StreamReader;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::vcp_modules::aurora_pipeline::{AuroraBuffer, AuroraUpdate, AuroraUpdateKind};
use crate::vcp_modules::content_parser::ContentBlock;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::settings_manager::{
    read_settings, ChatEndpointMode, Settings, SettingsState,
};
use crate::vcp_modules::topic_types::{MessageKey, TopicKey};

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

const STANDARD_CHAT_SUFFIX: &str = "/v1/chat/completions";
const VCP_TOOLS_CHAT_SUFFIX: &str = "/v1/chatvcp/completions";
const MODELS_SUFFIX: &str = "/v1/models";
pub const MODEL_DISCOVERY_UNAVAILABLE: &str = "MODEL_DISCOVERY_UNAVAILABLE";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatRequestPurpose {
    Interactive,
    Auxiliary,
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

struct ValidatedHttpEndpoint {
    url: Url,
    explicit_default_port: Option<u16>,
}

fn explicit_port_from_raw(raw_url: &str) -> Option<u16> {
    let (_, after_scheme) = raw_url.split_once("://")?;
    let authority = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    let port = if authority.starts_with('[') {
        let closing_bracket = authority.find(']')?;
        authority.get(closing_bracket + 1..)?.strip_prefix(':')?
    } else {
        authority.rsplit_once(':')?.1
    };
    port.parse().ok()
}

fn validate_http_endpoint(raw_url: &str) -> Result<ValidatedHttpEndpoint, String> {
    if raw_url.chars().any(char::is_control) {
        return Err("URL 不能包含控制字符".to_string());
    }

    let url = Url::parse(raw_url).map_err(|error| format!("URL 解析失败: {error}"))?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err("URL 仅支持 HTTP 或 HTTPS".to_string());
    }
    if url.host_str().is_none() {
        return Err("URL 必须包含主机名".to_string());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("URL 不能包含用户名或密码".to_string());
    }
    if url.fragment().is_some() {
        return Err("URL 不能包含 fragment".to_string());
    }
    let explicit_default_port = explicit_port_from_raw(raw_url)
        .filter(|port| url.port().is_none() && url.port_or_known_default() == Some(*port));
    Ok(ValidatedHttpEndpoint {
        url,
        explicit_default_port,
    })
}

fn strip_known_chat_suffix(path: &str) -> Option<&str> {
    let path = path.trim_end_matches('/');
    [STANDARD_CHAT_SUFFIX, VCP_TOOLS_CHAT_SUFFIX]
        .into_iter()
        .find_map(|suffix| path.strip_suffix(suffix))
}

fn derive_known_api_endpoint(endpoint: ValidatedHttpEndpoint, suffix: &str) -> String {
    let ValidatedHttpEndpoint {
        mut url,
        explicit_default_port,
    } = endpoint;
    let path = url.path().trim_end_matches('/');
    let prefix = strip_known_chat_suffix(path)
        .or_else(|| path.strip_suffix("/v1"))
        .unwrap_or(path)
        .trim_end_matches('/');
    url.set_path(&format!("{prefix}{suffix}"));
    let mut derived = url.to_string();
    if let Some(port) = explicit_default_port {
        if let Some(after_scheme) = derived.find("://").map(|index| index + 3) {
            let authority_len = derived[after_scheme..]
                .find(['/', '?', '#'])
                .unwrap_or(derived.len() - after_scheme);
            derived.insert_str(after_scheme + authority_len, &format!(":{port}"));
        }
    }
    derived
}

pub fn resolve_chat_endpoint(
    raw_url: &str,
    mode: ChatEndpointMode,
    purpose: ChatRequestPurpose,
) -> Result<String, String> {
    let endpoint = validate_http_endpoint(raw_url)?;
    if mode == ChatEndpointMode::Raw {
        // Raw 模式只做安全校验，正式请求必须逐字复用用户输入。
        return Ok(raw_url.to_string());
    }

    let suffix = match (mode, purpose) {
        (ChatEndpointMode::VcpTools, ChatRequestPurpose::Interactive) => VCP_TOOLS_CHAT_SUFFIX,
        _ => STANDARD_CHAT_SUFFIX,
    };
    Ok(derive_known_api_endpoint(endpoint, suffix))
}

pub fn resolve_model_discovery_endpoint(
    raw_url: &str,
    mode: ChatEndpointMode,
) -> Result<Option<String>, String> {
    let endpoint = validate_http_endpoint(raw_url)?;
    if mode == ChatEndpointMode::Raw && strip_known_chat_suffix(endpoint.url.path()).is_none() {
        return Ok(None);
    }
    Ok(Some(derive_known_api_endpoint(endpoint, MODELS_SUFFIX)))
}

pub fn freeze_chat_connection(
    settings: &Settings,
    purpose: ChatRequestPurpose,
) -> Result<ChatConnectionSnapshot, String> {
    if settings.vcp_server_url.is_empty() {
        return Err("VCP Server URL is not configured.".to_string());
    }
    Ok(ChatConnectionSnapshot {
        endpoint_url: resolve_chat_endpoint(
            &settings.vcp_server_url,
            settings.chat_endpoint_mode,
            purpose,
        )?,
        api_key: settings.vcp_api_key.clone(),
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

#[cfg(any(target_os = "android", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HelperStartErrorDisposition {
    AdoptExistingSession,
    Fail,
}

#[cfg(any(target_os = "android", test))]
fn classify_helper_start_error(
    error: &str,
    observed_generation: Option<u64>,
) -> HelperStartErrorDisposition {
    if observed_generation.is_none() && error == "Session already exists" {
        HelperStartErrorDisposition::AdoptExistingSession
    } else {
        HelperStartErrorDisposition::Fail
    }
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
    pub r#type: String, // "end" 是唯一 durable 终态；"error" 仅表示终态提交失败、仍可恢复
    pub chunk: Option<Value>, // 数据块 (仅 type="data" 时有效)
    pub message_id: String, // 消息ID
    pub context: Option<Value>, // 透传的上下文信息
    pub finish_reason: Option<String>, // 结束原因
    pub error: Option<String>, // 错误信息 (仅 type="error" 时有效)
    pub content: Option<String>, // durable end / commit failure 的权威正文
    pub aurora: Option<AuroraUpdate>, // Aurora 语义沉淀更新 (type="aurora" 时有效)
    pub blocks: Option<Vec<ContentBlock>>, // 持久化后的预渲染块 (仅 type="end" 时有效)
    pub timestamp: Option<u64>, // ⚡ 新增物理落笔时间戳
    pub topic_updated_at: Option<i64>, // durable message bubble 后的话题列表权威时间
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

/// VCP 请求失败的内部类型。流式路径显式携带已经接收的完整 partial，
/// 防止上层只能从尚未写入正文的 pending 数据库行猜测内容。
#[derive(Debug, Clone)]
pub struct VcpRequestFailure {
    message: String,
    partial_content: Option<String>,
}

impl VcpRequestFailure {
    fn streaming(message: impl Into<String>, partial_content: String) -> Self {
        Self {
            message: message.into(),
            partial_content: Some(partial_content),
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
        }
    }
}

fn streaming_failure(
    buffer: &mut AuroraBuffer,
    pending_chunk: &mut String,
    message: impl Into<String>,
) -> VcpRequestFailure {
    if !pending_chunk.is_empty() {
        buffer.append_chunk(pending_chunk);
        pending_chunk.clear();
    }
    VcpRequestFailure::streaming(message, buffer.full_text.clone())
}

const AURORA_PARSE_INTERVAL: Duration = Duration::from_millis(33);

fn remaining_aurora_parse_delay(elapsed: Duration) -> Duration {
    AURORA_PARSE_INTERVAL.saturating_sub(elapsed)
}

#[cfg(any(target_os = "android", test))]
fn merge_recovery_partial(helper_content: &str, partial_content: Option<String>) -> String {
    let Some(partial_content) = partial_content else {
        return helper_content.to_string();
    };
    if partial_content.starts_with(helper_content) {
        partial_content
    } else if helper_content.starts_with(&partial_content) {
        helper_content.to_string()
    } else {
        log::warn!(
            "[VCPClient] Recovery partial diverged from helper snapshot; preserving helper-owned content"
        );
        helper_content.to_string()
    }
}

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
}

pub type ActiveRequestMap = Arc<DashMap<MessageKey, Arc<ActiveRequestEntry>>>;

/// 全局活跃请求管理器。相同 message id 只允许一个 live attempt。
pub struct ActiveRequests(pub ActiveRequestMap);

impl Default for ActiveRequests {
    fn default() -> Self {
        log::info!("[VCPClient] Initialized ActiveRequests successfully.");
        Self(Arc::new(DashMap::new()))
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
    attempt_id: uuid::Uuid,
}

impl ActiveRequestLease {
    pub fn try_acquire(
        requests: ActiveRequestMap,
        key: MessageKey,
    ) -> Result<(Self, CancellationToken), String> {
        let attempt_id = uuid::Uuid::new_v4();
        let cancellation_token = CancellationToken::new();
        let entry = Arc::new(ActiveRequestEntry {
            attempt_id,
            cancellation_token: cancellation_token.clone(),
        });

        match requests.entry(key.clone()) {
            Entry::Vacant(slot) => {
                slot.insert(entry);
                Ok((
                    Self {
                        requests: requests.clone(),
                        key,
                        attempt_id,
                    },
                    cancellation_token,
                ))
            }
            Entry::Occupied(_) => Err(format!(
                "Request {} is already active for {}/{}/{}; duplicate attempt rejected",
                key.msg_id, key.topic.owner_type, key.topic.owner_id, key.topic.topic_id
            )),
        }
    }
}

impl Drop for ActiveRequestLease {
    fn drop(&mut self) {
        if let Entry::Occupied(entry) = self.requests.entry(self.key.clone()) {
            if entry.get().attempt_id == self.attempt_id {
                entry.remove();
            }
        }
    }
}

type ActiveGroupTurnMap = Arc<DashMap<TopicKey, CancellationToken>>;

/// 每个 Topic 同时只允许一个群组回合，并由同一所有者承载回合取消状态。
pub struct ActiveGroupTurns(ActiveGroupTurnMap);

impl Default for ActiveGroupTurns {
    fn default() -> Self {
        log::info!("[VCPClient] Initialized ActiveGroupTurns successfully.");
        Self(Arc::new(DashMap::new()))
    }
}

impl ActiveGroupTurns {
    pub(crate) fn try_acquire(&self, key: TopicKey) -> Result<ActiveGroupTurnLease, String> {
        let cancellation_token = CancellationToken::new();

        match self.0.entry(key.clone()) {
            Entry::Vacant(slot) => {
                slot.insert(cancellation_token.clone());
                Ok(ActiveGroupTurnLease {
                    turns: self.0.clone(),
                    key,
                    cancellation_token,
                })
            }
            Entry::Occupied(_) => Err(format!(
                "Group turn is already active for {}/{}/{}",
                key.owner_type, key.owner_id, key.topic_id
            )),
        }
    }

    fn cancel(&self, key: &TopicKey) -> bool {
        let Some(entry) = self.0.get(key) else {
            return false;
        };
        entry.cancel();
        drop(entry);
        true
    }
}

pub(crate) struct ActiveGroupTurnLease {
    turns: ActiveGroupTurnMap,
    key: TopicKey,
    cancellation_token: CancellationToken,
}

impl ActiveGroupTurnLease {
    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancellation_token.is_cancelled()
    }
}

impl Drop for ActiveGroupTurnLease {
    fn drop(&mut self) {
        self.turns.remove(&self.key);
    }
}

/// 中止群组的整个接力赛回合
#[tauri::command]
#[allow(non_snake_case)]
pub fn interruptGroupTurn(
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
    let cancelled = state.cancel(&key);
    Ok(json!({"status": if cancelled { "cancelled" } else { "not_running" }}))
}

/// 核心请求函数：sendToVCP
/// 对应 JS 版的 sendToVCP。处理逻辑：
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

#[tauri::command]
#[allow(non_snake_case)]
pub async fn sendToVCP<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, ActiveRequests>,
    settings_state: tauri::State<'_, SettingsState>,
    mut payload: VcpRequestPayload,
    stream_channel: Channel<StreamEvent>,
) -> Result<Value, String> {
    let settings = read_settings(app.clone(), settings_state).await?;
    let connection = freeze_chat_connection(&settings, ChatRequestPurpose::Interactive)?;
    payload.vcp_url = connection.endpoint_url;
    payload.vcp_api_key = connection.api_key;

    let message_id = payload.message_id.clone();
    let context = payload.context.clone();
    let is_stream = payload.model_config["stream"].as_bool().unwrap_or(false);
    let request_key = message_key_from_context(&context, &message_id)?;
    if payload.transport_request_id.is_none() {
        payload.transport_request_id = Some(message_transport_request_id(&request_key));
    }

    let (lease, cancellation_token) =
        ActiveRequestLease::try_acquire(state.0.clone(), request_key.clone())?;
    let (res, is_aborted) = match perform_vcp_request_registered(
        &app,
        payload,
        Some(stream_channel.clone()),
        cancellation_token,
    )
    .await
    {
        Ok(val) => val,
        Err(failure) => {
            let (error, partial_content) = failure.into_parts();
            if is_stream {
                let pool = app
                    .state::<crate::vcp_modules::db_manager::DbState>()
                    .pool
                    .clone();
                let _ = finalize_stream_error(
                    &app,
                    &pool,
                    &request_key,
                    partial_content.unwrap_or_default(),
                    error.clone(),
                    Some(stream_channel.clone()),
                )
                .await?;
            }
            return Err(error);
        }
    };

    if is_stream {
        let finish_reason = if is_aborted {
            Some("cancelled_by_user".to_string())
        } else {
            res["finishReason"].as_str().map(|s| s.to_string())
        };

        let agent_id = context.as_ref().and_then(|value| value["agentId"].as_str());

        let pool = app
            .state::<crate::vcp_modules::db_manager::DbState>()
            .pool
            .clone();

        crate::vcp_modules::chat::message_service::finalize_stream_message(
            app.clone(),
            &pool,
            &request_key,
            res["fullContent"].as_str().unwrap_or("").to_string(),
            is_aborted,
            finish_reason,
            Some(stream_channel),
            agent_id.map(|s| s.to_string()),
        )
        .await?;
    }

    drop(lease);
    Ok(res)
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

/// 核心请求实现函数，可供 Tauri Command 或 内部 Rust 模块(如 GroupOrchestrator) 调用
/// 返回 Result<(全量内容/响应体, 是否被中止), 错误信息>
#[allow(dead_code)] // DORMANT ASSET: only the unregistered floating-assistant bridge uses this API.
pub async fn perform_vcp_request<R: Runtime>(
    app: &AppHandle<R>,
    active_requests: ActiveRequestMap,
    request_key: MessageKey,
    payload: VcpRequestPayload,
    stream_channel: Option<Channel<StreamEvent>>,
) -> Result<(Value, bool), VcpRequestFailure> {
    let (lease, cancellation_token) = ActiveRequestLease::try_acquire(active_requests, request_key)
        .map_err(VcpRequestFailure::from)?;
    let result =
        perform_vcp_request_registered(app, payload, stream_channel, cancellation_token).await;
    drop(lease);
    result
}

/// 执行一个已经注册的请求。调用方持有 lease，决定请求在 finalizer 提交后才退出 live 状态。
pub async fn perform_vcp_request_registered<R: Runtime>(
    app: &AppHandle<R>,
    payload: VcpRequestPayload,
    stream_channel: Option<Channel<StreamEvent>>,
    cancellation_token: CancellationToken,
) -> Result<(Value, bool), VcpRequestFailure> {
    log::info!(
        "[VCPClient] perform_vcp_request called for messageId: {}, context: {:?}",
        payload.message_id,
        payload.context
    );

    let message_id = payload.message_id.clone();
    let context = payload.context.clone();
    let transport_request_id = payload.effective_transport_request_id().to_string();

    // === 1. 数据验证和多模态资产转换 ===
    let mut messages = preprocess_multimodal_messages(app, payload.messages)
        .await
        .map_err(VcpRequestFailure::from)?;

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
    let mut request_body = payload.model_config.clone();
    if let Some(obj) = request_body.as_object_mut() {
        obj.insert("messages".to_string(), json!(messages));
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

    // === 6. 配置网络请求（共享 ChatStream 画像 Client，克隆仅复制 Arc 句柄、共享连接池） ===
    let client = super::http_clients::client(super::http_clients::HttpProfile::ChatStream).clone();

    // === 7. 分发至专职处理器执行请求 ===
    if is_stream {
        handle_streaming_request(
            app,
            client,
            &final_url,
            &payload.vcp_api_key,
            request_body,
            message_id,
            transport_request_id,
            context,
            cancellation_token,
            stream_channel,
            false,
            None,
            None,
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
            stream_channel,
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

/// 1. 抽离多模态消息预处理逻辑
async fn preprocess_multimodal_messages<R: Runtime>(
    app: &AppHandle<R>,
    raw_messages: Vec<Value>,
) -> Result<Vec<Value>, String> {
    let mut messages: Vec<Value> = Vec::new();
    for msg_val in raw_messages.into_iter() {
        if !msg_val.is_object() {
            messages.push(json!({"role": "system", "content": "[Invalid message]"}));
            continue;
        }

        let mut msg = msg_val.clone();
        remove_internal_local_attachment_metadata(&mut msg);
        let content = msg.get("content").cloned().unwrap_or(Value::Null);

        // 处理多模态或复杂内容数组
        if let Some(content_array) = content.as_array() {
            let mut new_parts = Vec::new();
            for part in content_array {
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
                                // 图片类型：长边 > 1120px 时缩放，避免多模态 payload 过大
                                let path_buf_clone = path_buf.clone();
                                let app_clone = app.clone();
                                match tokio::task::spawn_blocking(move || {
                                    convert_local_image_for_multimodal(&app_clone, &path_buf_clone)
                                })
                                .await
                                {
                                    Ok(Ok(data_url)) => {
                                        new_parts.push(json!({
                                            "type": "image_url",
                                            "image_url": { "url": data_url }
                                        }));
                                        converted = true;
                                    }
                                    Ok(Err(e)) => {
                                        log::warn!(
                                                "[VCPClient] Image conversion failed for attachment CAS {}: {}",
                                                short_hash(hash), e
                                            );
                                    }
                                    Err(e) => {
                                        log::warn!(
                                            "[VCPClient] Image conversion task panicked: {}",
                                            e
                                        );
                                    }
                                }
                            } else if media_kind == "video" {
                                // 视频：抽帧 → 每张帧作为 image_url
                                let path_clone = path_buf.clone();
                                let app_clone = app.clone();
                                match tokio::task::spawn_blocking(move || {
                                        crate::vcp_modules::media_processor::process_video_for_multimodal(&app_clone, &path_clone)
                                    }).await {
                                        Ok(Ok(frames)) => {
                                            for frame_url in frames {
                                                new_parts.push(json!({
                                                    "type": "image_url",
                                                    "image_url": { "url": frame_url }
                                                }));
                                            }
                                            converted = true;
                                        }
                                        Ok(Err(e)) => {
                                            log::warn!("[VCPClient] Video frame extraction failed for attachment CAS {}: {}", short_hash(hash), e);
                                        }
                                        Err(e) => {
                                            log::warn!("[VCPClient] Video processing task panicked: {}", e);
                                        }
                                    }
                            } else if media_kind == "audio" {
                                // 音频：提取为 MP3 (32kbps) 或 AAC (32kbps) -> input_audio
                                let path_clone = path_buf.clone();
                                let app_clone = app.clone();
                                match tokio::task::spawn_blocking(move || {
                                        crate::vcp_modules::media_processor::process_audio_for_multimodal(&app_clone, &path_clone)
                                    }).await {
                                        Ok(Ok(audio_url)) => {
                                            let format_str = if audio_url.starts_with("data:audio/aac") { "aac" } else { "mp3" };
                                            new_parts.push(json!({
                                                "type": "input_audio",
                                                "input_audio": { 
                                                    "data": audio_url, 
                                                    "format": format_str
                                                }
                                            }));
                                            converted = true;
                                        }
                                        Ok(Err(e)) => {
                                            log::warn!("[VCPClient] Audio extraction failed for attachment CAS {}: {}", short_hash(hash), e);
                                        }
                                        Err(e) => {
                                            log::warn!("[VCPClient] Audio processing task panicked: {}", e);
                                        }
                                    }
                            }
                        } else {
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
                        new_parts.push(part.clone());
                    }
                } else {
                    new_parts.push(part.clone());
                }
            }
            msg["content"] = json!(new_parts);
        } else if content.is_object() {
            if let Some(text) = content.get("text") {
                msg["content"] = text.clone();
            } else {
                msg["content"] = json!(content.to_string());
            }
        } else if !content.is_string() && !content.is_null() {
            msg["content"] = json!(content.to_string());
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
fn get_helper_port<R: Runtime>(app: &AppHandle<R>) -> Result<u16, String> {
    let cache_dir = app.path().app_cache_dir().map_err(|e| e.to_string())?;
    let port_file = cache_dir.join("sse_helper.port");
    if !port_file.exists() {
        return Err("sse_helper.port file not found. Is SseProxyService running?".to_string());
    }
    let content = std::fs::read_to_string(port_file).map_err(|e| e.to_string())?;
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
    if port_file.exists() {
        if let Ok(content) = std::fs::read_to_string(&port_file) {
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
    }

    // 2. 如果连接失败或文件不存在，启动/唤醒 helper 服务
    log::info!(
        "[VCPClient] Helper not responding or port file missing. Starting/Waking helper service..."
    );
    let _ = tauri_plugin_vcp_mobile::stream::start_helper_service(app.clone());

    // 3. 循环等待新端口文件并尝试连接（最多尝试 60 次，每次间隔 50ms，总计 3 秒超时）
    let mut last_err = String::new();
    let max_attempts = 60;
    let delay = Duration::from_millis(50);
    let ready_deadline = tokio::time::Instant::now() + Duration::from_secs(3);

    for attempt in 1..=max_attempts {
        if tokio::time::Instant::now() >= ready_deadline {
            break;
        }
        if !port_file.exists() {
            tokio::time::sleep(delay).await;
            continue;
        }

        let content = match std::fs::read_to_string(&port_file) {
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

    Err(format!(
        "Failed to connect to sse helper after {}s (last error: {})",
        (max_attempts as f32 * 0.05),
        last_err
    ))
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
    if let Some(params) = extra_params {
        if let Some(obj) = cmd.as_object_mut() {
            for (k, v) in params.as_object().unwrap() {
                obj.insert(k.clone(), v.clone());
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
async fn query_helper_generation<R: Runtime>(
    app: &AppHandle<R>,
    msg_id: &str,
) -> Result<u64, String> {
    let port = get_helper_port(app)?;
    let stream = connect_helper_port(port).await?;
    let stream = send_command_to_stream(stream, "query", msg_id, None).await?;
    let mut reader = FramedRead::new(stream, helper_frame_codec());
    let frame = tokio::time::timeout(HELPER_IO_TIMEOUT, reader.next())
        .await
        .map_err(|_| "Helper generation query timed out".to_string())?
        .ok_or_else(|| "Helper generation query ended at EOF".to_string())?
        .map_err(|e| format!("Helper generation query frame error: {}", e))?;
    let response = serde_json::from_slice::<Value>(&frame)
        .map_err(|e| format!("Invalid helper generation response: {}", e))?;
    if response.get("requestId").and_then(Value::as_str) != Some(msg_id) {
        return Err("Helper generation response identity mismatch".to_string());
    }
    response
        .get("generation")
        .and_then(Value::as_u64)
        .filter(|generation| *generation > 0)
        .ok_or_else(|| "Helper session has no active generation".to_string())
}

#[cfg(target_os = "android")]
async fn send_stop_to_helper<R: Runtime>(
    app: &AppHandle<R>,
    msg_id: &str,
    expected_generation: Option<u64>,
) -> Result<(), String> {
    let expected_generation = match expected_generation.filter(|generation| *generation > 0) {
        Some(generation) => generation,
        None => query_helper_generation(app, msg_id).await?,
    };
    let port = get_helper_port(app)?;
    let stream = connect_helper_port(port).await?;
    let stream = send_command_to_stream(
        stream,
        "stop",
        msg_id,
        Some(json!({ "generation": expected_generation })),
    )
    .await?;
    let mut reader = FramedRead::new(stream, helper_frame_codec());
    let frame = tokio::time::timeout(HELPER_IO_TIMEOUT, reader.next())
        .await
        .map_err(|_| "Helper stop ACK timed out".to_string())?
        .ok_or_else(|| "Helper stop ACK ended at EOF".to_string())?
        .map_err(|e| format!("Helper stop ACK frame error: {}", e))?;
    let ack = serde_json::from_slice::<Value>(&frame)
        .map_err(|e| format!("Invalid helper stop ACK: {}", e))?;
    validate_helper_stop_ack(&ack, msg_id, expected_generation)
}

#[cfg(any(target_os = "android", test))]
fn validate_helper_stop_ack(
    ack: &Value,
    msg_id: &str,
    expected_generation: u64,
) -> Result<(), String> {
    if ack.get("action").and_then(Value::as_str) == Some("stop_ack")
        && ack.get("requestId").and_then(Value::as_str) == Some(msg_id)
        && ack.get("generation").and_then(Value::as_u64) == Some(expected_generation)
        && ack.get("stopped").and_then(Value::as_bool) == Some(true)
    {
        return Ok(());
    }

    Err(format!(
        "Helper stop ACK rejected: expected requestId={} generation={}, got {}",
        msg_id, expected_generation, ack
    ))
}

/// 3. 流式请求循环（固定 33ms Aurora 解析门禁）
#[allow(clippy::too_many_arguments, unused_variables)]
async fn handle_streaming_request<R: Runtime>(
    _app: &AppHandle<R>,
    client: Client,
    final_url: &str,
    api_key: &str,
    request_body: Value,
    message_id: String,
    transport_request_id: String,
    context: Option<Value>,
    cancellation_token: CancellationToken,
    stream_channel: Option<Channel<StreamEvent>>,
    is_resume: bool,
    last_event_index: Option<i64>,
    initial_content: Option<String>,
) -> Result<(Value, bool), VcpRequestFailure> {
    let send_stream_event = |event: StreamEvent| -> bool {
        if let Some(ref ch) = stream_channel {
            if let Err(error) = ch.send(event) {
                log::error!(
                    "[VCPClient] Failed to send stream event for {}: {}",
                    message_id,
                    error
                );
                return false;
            }
        }
        true
    };

    let message_id_inner = message_id.clone();
    let context_inner = context.clone();

    let mut last_finish_reason: Option<String> = None;
    #[allow(unused_mut)]
    let mut last_received_index: Option<i64> = last_event_index;
    #[cfg(target_os = "android")]
    let mut helper_generation: Option<u64> = None;
    let mut aurora_buffer = AuroraBuffer::new();
    let mut pending_aurora_chunk = String::new();
    let mut last_aurora_parse = Instant::now() - Duration::from_millis(33);
    let mut retry_count = 0;
    let mut backoff = Duration::from_millis(500);

    let send_aurora_update = |buffer: &mut AuroraBuffer, finish_reason: Option<String>| {
        let is_final = finish_reason.is_some();
        let prepared = if is_final {
            Some(buffer.prepare_snapshot_update())
        } else {
            buffer.prepare_delta_update()
        };
        let Some((update, commit)) = prepared else {
            return;
        };
        let mut event =
            StreamEvent::aurora(message_id_inner.clone(), update, context_inner.clone());
        event.finish_reason = finish_reason;
        if send_stream_event(event) {
            buffer.commit_delivery(commit);
        }
    };

    let flush_aurora_parse = |buffer: &mut AuroraBuffer,
                              pending_chunk: &mut String,
                              last_parse: &mut Instant,
                              force: bool|
     -> (bool, bool) {
        if pending_chunk.is_empty() {
            return (false, false);
        }
        if !force && last_parse.elapsed() < AURORA_PARSE_INTERVAL {
            return (false, false);
        }

        buffer.append_chunk(pending_chunk);
        pending_chunk.clear();
        *last_parse = Instant::now();
        buffer.process_queue()
    };

    type BoxedLineStream =
        Box<dyn futures_util::Stream<Item = Result<String, std::io::Error>> + Unpin + Send>;

    let to_line_stream = |resp: reqwest::Response| -> BoxedLineStream {
        let stream = resp.bytes_stream().map_err(std::io::Error::other);
        let reader = StreamReader::new(stream);
        let framed = FramedRead::new(reader, LinesCodec::new_with_max_length(512 * 1024));
        let mapped = framed.map_err(std::io::Error::other);
        Box::new(mapped)
    };

    #[cfg(target_os = "android")]
    let mut tcp_reader: Option<FramedRead<tokio::net::TcpStream, LengthDelimitedCodec>> = None;

    #[cfg(not(target_os = "android"))]
    let mut lines: Option<BoxedLineStream> = None;

    // 1. 声明状态机的所有状态
    enum State {
        Init,
        Connecting,
        Resuming,
        Streaming,
        Aligning,
        Retrying,
    }

    let mut state = State::Init;

    // 2. 状态机驱动循环
    loop {
        match state {
            State::Init => {
                if let Some(ref content) = initial_content {
                    aurora_buffer.append_chunk(content);
                    let _ = aurora_buffer.process_queue();

                    // 暖接续只发送一次 helper 权威 Snapshot，并以 reset frame 建立新序列基线。
                    let (baseline, commit) = aurora_buffer.prepare_snapshot_update();
                    if send_stream_event(StreamEvent::aurora(
                        message_id_inner.clone(),
                        baseline,
                        context_inner.clone(),
                    )) {
                        aurora_buffer.commit_delivery(commit);
                    }
                }
                if is_resume {
                    state = State::Resuming;
                } else {
                    state = State::Connecting;
                }
            }
            State::Connecting => {
                #[cfg(target_os = "android")]
                {
                    let headers_json = json!({
                        "Authorization": format!("Bearer {}", api_key),
                        "Content-Type": "application/json"
                    });
                    let mut sse_context = json!({});
                    if let Some(ref ctx) = context_inner {
                        if let Some(agent_name) = ctx.get("agentName").and_then(|v| v.as_str()) {
                            sse_context["agentName"] = json!(agent_name);
                        }
                        if let Some(topic_id) = ctx.get("topicId").and_then(|v| v.as_str()) {
                            sse_context["topicId"] = json!(topic_id);
                        }
                        if let Some(owner_id) = ctx.get("ownerId").and_then(|v| v.as_str()) {
                            sse_context["ownerId"] = json!(owner_id);
                        }
                        if let Some(owner_type) = ctx.get("ownerType").and_then(|v| v.as_str()) {
                            sse_context["ownerType"] = json!(owner_type);
                        }
                    }

                    let params = json!({
                        "url": final_url,
                        "headers": headers_json.to_string(),
                        "body": request_body.to_string(),
                        "context": sse_context
                    });

                    match connect_to_helper(_app, "start", &transport_request_id, Some(params))
                        .await
                    {
                        Ok(stream) => {
                            tcp_reader = Some(FramedRead::new(stream, helper_frame_codec()));
                            state = State::Streaming;
                        }
                        Err(e) => {
                            log::error!("[VCPClient] connect_to_helper failed: {:?}", e);
                            return Err(streaming_failure(
                                &mut aurora_buffer,
                                &mut pending_aurora_chunk,
                                e,
                            ));
                        }
                    }
                }
                #[cfg(not(target_os = "android"))]
                {
                    let res_future = client
                        .post(final_url)
                        .header(AUTHORIZATION, format!("Bearer {}", api_key))
                        .header(CONTENT_TYPE, "application/json")
                        .json(&request_body)
                        .send();

                    tokio::select! {
                        _ = cancellation_token.cancelled() => {
                            log::warn!("[VCPClient] Request aborted during connection: {}", message_id_inner);
                            flush_aurora_parse(&mut aurora_buffer, &mut pending_aurora_chunk, &mut last_aurora_parse, true);
                            aurora_buffer.finalize();
                            send_aurora_update(
                                &mut aurora_buffer,
                                Some("cancelled_by_user".to_string()),
                            );
                            return Ok((json!({ "fullContent": aurora_buffer.full_text, "streamingStarted": false }), true));
                        }
                        response_res = res_future => {
                            match response_res {
                                Ok(resp) if resp.status().is_success() => {
                                    lines = Some(to_line_stream(resp));
                                    state = State::Streaming;
                                }
                                Ok(resp) => {
                                    let status = resp.status();
                                    let text = resp.text().await.unwrap_or_default();
                                    log::warn!("[VCPClient] VCP server rejected request: {} - {}", status, text);
                                    return Err(streaming_failure(
                                        &mut aurora_buffer,
                                        &mut pending_aurora_chunk,
                                        format!("VCP Error: {}", status),
                                    ));
                                }
                                Err(e) => {
                                    log::warn!("[VCPClient] Connection failed, transitioning to Retrying: {:?}", e);
                                    state = State::Retrying;
                                }
                            }
                        }
                    }
                }
            }
            State::Resuming => {
                while !crate::vcp_modules::infra::lifecycle_manager::is_app_in_foreground(_app) {
                    log::info!(
                        "[VCPClient] App is in background. Suspending reconnection for message: {}",
                        message_id_inner
                    );
                    tokio::select! {
                        _ = cancellation_token.cancelled() => {
                            #[cfg(target_os = "android")]
                            {
                                if let Err(error) = send_stop_to_helper(
                                    _app,
                                    &transport_request_id,
                                    helper_generation,
                                ).await {
                                    log::warn!("[VCPClient] Best-effort helper stop after cancellation failed: {}", error);
                                }
                            }
                            flush_aurora_parse(&mut aurora_buffer, &mut pending_aurora_chunk, &mut last_aurora_parse, true);
                            aurora_buffer.finalize();
                            send_aurora_update(
                                &mut aurora_buffer,
                                Some("cancelled_by_user".to_string()),
                            );
                            return Ok((json!({ "fullContent": aurora_buffer.full_text, "finishReason": Some("cancelled_by_user") }), true));
                        }
                        _ = tokio::time::sleep(Duration::from_secs(2)) => {}
                    }
                }

                #[cfg(target_os = "android")]
                {
                    log::info!(
                        "[VCPClient] Resuming SSE from local proxy socket for message: {}",
                        message_id_inner
                    );

                    let start_idx = last_received_index.map(|idx| idx + 1).unwrap_or(0);
                    let params = json!({
                        "startIndex": start_idx
                    });

                    match connect_to_helper(_app, "resume", &transport_request_id, Some(params))
                        .await
                    {
                        Ok(stream) => {
                            log::info!("[VCPClient] Successfully reconnected to sse helper socket");
                            tcp_reader = Some(FramedRead::new(stream, helper_frame_codec()));
                            retry_count = 0;
                            backoff = Duration::from_millis(500);
                            state = State::Streaming;
                        }
                        Err(e) => {
                            log::warn!("[VCPClient] Failed to reconnect to sse helper: {:?}", e);
                            state = State::Aligning;
                        }
                    }
                }
                #[cfg(not(target_os = "android"))]
                {
                    log::warn!("[VCPClient] Reconnection is only supported on Android via SSE proxy. Transitioning to Aligning.");
                    state = State::Aligning;
                }
            }
            State::Streaming => {
                let mut stream_ended_normally = false;

                #[cfg(target_os = "android")]
                {
                    if let Some(ref mut reader) = tcp_reader {
                        loop {
                            let pending_flush_delay =
                                remaining_aurora_parse_delay(last_aurora_parse.elapsed());
                            tokio::select! {
                                biased;
                                _ = cancellation_token.cancelled() => {
                                    log::warn!("[VCPClient] Request aborted during streaming: {}", message_id_inner);
                                    if let Err(error) = send_stop_to_helper(
                                        _app,
                                        &transport_request_id,
                                        helper_generation,
                                    ).await {
                                        log::warn!("[VCPClient] Best-effort helper stop after cancellation failed: {}", error);
                                    }
                                    flush_aurora_parse(&mut aurora_buffer, &mut pending_aurora_chunk, &mut last_aurora_parse, true);
                                    aurora_buffer.finalize();
                                    send_aurora_update(
                                        &mut aurora_buffer,
                                        Some("cancelled_by_user".to_string()),
                                    );
                                    return Ok((json!({ "fullContent": aurora_buffer.full_text, "finishReason": Some("cancelled_by_user") }), true));
                                }
                                _ = tokio::time::sleep(pending_flush_delay), if !pending_aurora_chunk.is_empty() => {
                                    flush_aurora_parse(
                                        &mut aurora_buffer,
                                        &mut pending_aurora_chunk,
                                        &mut last_aurora_parse,
                                        true,
                                    );
                                    send_aurora_update(&mut aurora_buffer, None);
                                }
                                next_line = reader.next() => {
                                    match next_line {
                                        Some(Ok(line)) => {
                                            if let Ok(event) = serde_json::from_slice::<Value>(&line) {
                                                if let Some(generation) = event.get("generation").and_then(Value::as_u64) {
                                                    if let Some(current) = helper_generation {
                                                        if current != generation {
                                                            return Err(streaming_failure(
                                                                &mut aurora_buffer,
                                                                &mut pending_aurora_chunk,
                                                                format!(
                                                                    "Helper generation changed during stream: expected {}, got {}",
                                                                    current, generation
                                                                ),
                                                            ));
                                                        }
                                                    } else {
                                                        helper_generation = Some(generation);
                                                    }
                                                }
                                                let event_type = event["eventType"].as_str().unwrap_or("");
                                                let event_data = event["eventData"].as_str().unwrap_or("");

                                                if let Some(idx) = event.get("index").and_then(|v| v.as_i64()) {
                                                    last_received_index = Some(idx);
                                                }

                                                if event_type == "message" {
                                                    if event_data == "[DONE]" {
                                                        stream_ended_normally = true;
                                                        break;
                                                    }
                                                    if let Ok(data_val) = serde_json::from_str::<Value>(event_data) {
                                                        if let Some(reason) = data_val.get("finish_reason").and_then(|r| r.as_str()) {
                                                            last_finish_reason = Some(reason.to_string());
                                                        }
                                                        if let Some(delta) = data_val.get("choices").and_then(|c| c.as_array()).and_then(|a| a.first()).and_then(|o| o.get("delta")).and_then(|d| d.get("content")).and_then(|s| s.as_str()) {
                                                            pending_aurora_chunk.push_str(delta);
                                                            let _ = flush_aurora_parse(
                                                                &mut aurora_buffer,
                                                                &mut pending_aurora_chunk,
                                                                &mut last_aurora_parse,
                                                                false,
                                                            );
                                                            send_aurora_update(&mut aurora_buffer, None);
                                                        }
                                                    }
                                                } else if event_type == "closed" {
                                                    stream_ended_normally = true;
                                                    break;
                                                } else if event_type == "error" {
                                                    let err_msg = if let Ok(err_val) = serde_json::from_str::<Value>(event_data) {
                                                        err_val["error"].as_str().unwrap_or("Unknown proxy error").to_string()
                                                    } else {
                                                        "Unknown proxy error".to_string()
                                                    };
                                                    if classify_helper_start_error(
                                                        &err_msg,
                                                        helper_generation,
                                                    ) == HelperStartErrorDisposition::AdoptExistingSession
                                                    {
                                                        let generation = match query_helper_generation(
                                                            _app,
                                                            &transport_request_id,
                                                        )
                                                        .await
                                                        {
                                                            Ok(generation) => generation,
                                                            Err(query_error) => {
                                                                return Err(streaming_failure(
                                                                    &mut aurora_buffer,
                                                                    &mut pending_aurora_chunk,
                                                                    format!(
                                                                        "Cannot adopt existing helper session: {query_error}"
                                                                    ),
                                                                ));
                                                            }
                                                        };
                                                        helper_generation = Some(generation);
                                                        last_received_index = None;
                                                        log::info!(
                                                            "[VCPClient] Adopting existing helper session for transport {}, generation {}",
                                                            transport_request_id,
                                                            generation
                                                        );
                                                        state = State::Resuming;
                                                        break;
                                                    }
                                                    log::warn!("[VCPClient] Stream proxy error: {}. Failing stream immediately.", err_msg);
                                                    if let Err(stop_error) = send_stop_to_helper(
                                                        _app,
                                                        &transport_request_id,
                                                        helper_generation,
                                                    )
                                                    .await
                                                    {
                                                        log::warn!(
                                                            "[VCPClient] Best-effort helper stop after proxy error failed: {}",
                                                            stop_error
                                                        );
                                                    }
                                                    return Err(streaming_failure(
                                                        &mut aurora_buffer,
                                                        &mut pending_aurora_chunk,
                                                        err_msg,
                                                    ));
                                                }
                                            }
                                        }
                                        Some(Err(e)) => {
                                            log::warn!("[VCPClient] TCP socket read error: {:?}, transitioning to Retrying", e);
                                            flush_aurora_parse(
                                                &mut aurora_buffer,
                                                &mut pending_aurora_chunk,
                                                &mut last_aurora_parse,
                                                true,
                                            );
                                            send_aurora_update(&mut aurora_buffer, None);
                                            state = State::Retrying;
                                            break;
                                        }
                                        None => {
                                            log::warn!("[VCPClient] TCP socket closed by server. Transitioning to Retrying.");
                                            flush_aurora_parse(
                                                &mut aurora_buffer,
                                                &mut pending_aurora_chunk,
                                                &mut last_aurora_parse,
                                                true,
                                            );
                                            send_aurora_update(&mut aurora_buffer, None);
                                            state = State::Retrying;
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                    } else {
                        log::error!("[VCPClient] Streaming state entered but tcp_reader is None. Transitioning to Retrying.");
                        state = State::Retrying;
                    }
                }

                #[cfg(not(target_os = "android"))]
                {
                    if let Some(ref mut line_stream) = lines {
                        loop {
                            let pending_flush_delay =
                                remaining_aurora_parse_delay(last_aurora_parse.elapsed());
                            tokio::select! {
                                biased;
                                _ = cancellation_token.cancelled() => {
                                    log::warn!("[VCPClient] Request aborted during streaming: {}", message_id_inner);
                                    flush_aurora_parse(&mut aurora_buffer, &mut pending_aurora_chunk, &mut last_aurora_parse, true);
                                    aurora_buffer.finalize();
                                    send_aurora_update(
                                        &mut aurora_buffer,
                                        Some("cancelled_by_user".to_string()),
                                    );
                                    return Ok((json!({ "fullContent": aurora_buffer.full_text, "finishReason": Some("cancelled_by_user") }), true));
                                }
                                _ = tokio::time::sleep(pending_flush_delay), if !pending_aurora_chunk.is_empty() => {
                                    flush_aurora_parse(
                                        &mut aurora_buffer,
                                        &mut pending_aurora_chunk,
                                        &mut last_aurora_parse,
                                        true,
                                    );
                                    send_aurora_update(&mut aurora_buffer, None);
                                }
                                next_line = line_stream.next() => {
                                    match next_line {
                                        Some(Ok(line)) => {
                                            if let Some(stripped) = line.strip_prefix("data:") {
                                                let data_content = stripped.trim();
                                                if data_content == "[DONE]" {
                                                    stream_ended_normally = true;
                                                    break;
                                                }
                                                if let Ok(val) = serde_json::from_str::<Value>(data_content) {
                                                    if let Some(reason) = val.get("finish_reason").and_then(|r| r.as_str()) {
                                                        last_finish_reason = Some(reason.to_string());
                                                    }
                                                    if let Some(delta) = val.get("choices").and_then(|c| c.as_array()).and_then(|a| a.first()).and_then(|o| o.get("delta")).and_then(|d| d.get("content")).and_then(|s| s.as_str()) {
                                                        pending_aurora_chunk.push_str(delta);
                                                        let _ = flush_aurora_parse(
                                                            &mut aurora_buffer,
                                                            &mut pending_aurora_chunk,
                                                            &mut last_aurora_parse,
                                                            false,
                                                        );
                                                        send_aurora_update(&mut aurora_buffer, None);
                                                    }
                                                }
                                            }
                                        }
                                        Some(Err(e)) => {
                                            log::warn!("[VCPClient] Stream read error: {:?}, transitioning to Retrying", e);
                                            flush_aurora_parse(
                                                &mut aurora_buffer,
                                                &mut pending_aurora_chunk,
                                                &mut last_aurora_parse,
                                                true,
                                            );
                                            send_aurora_update(&mut aurora_buffer, None);
                                            state = State::Retrying;
                                            break;
                                        }
                                        None => {
                                            if !aurora_buffer.full_text.is_empty() || last_finish_reason.is_some() {
                                                stream_ended_normally = true;
                                            } else {
                                                log::warn!("[VCPClient] Stream ended unexpectedly (None), transitioning to Retrying");
                                                flush_aurora_parse(
                                                    &mut aurora_buffer,
                                                    &mut pending_aurora_chunk,
                                                    &mut last_aurora_parse,
                                                    true,
                                                );
                                                send_aurora_update(&mut aurora_buffer, None);
                                                state = State::Retrying;
                                            }
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                    } else {
                        log::warn!("[VCPClient] Streaming state entered but lines is None. Transitioning to Retrying.");
                        state = State::Retrying;
                    }
                }

                if stream_ended_normally {
                    flush_aurora_parse(
                        &mut aurora_buffer,
                        &mut pending_aurora_chunk,
                        &mut last_aurora_parse,
                        true,
                    );
                    aurora_buffer.finalize();
                    send_aurora_update(&mut aurora_buffer, last_finish_reason.clone());
                    #[cfg(target_os = "android")]
                    {
                        if let Err(error) =
                            send_stop_to_helper(_app, &transport_request_id, helper_generation)
                                .await
                        {
                            log::warn!("[VCPClient] Best-effort helper cleanup after completed stream failed: {}", error);
                        }
                    }
                    return Ok((
                        json!({
                            "fullContent": aurora_buffer.full_text,
                            "streamingStarted": true,
                            "finishReason": last_finish_reason
                        }),
                        false,
                    ));
                }
            }
            State::Aligning => {
                log::warn!("[VCPClient] Stream alignment failed (cache was empty or errored). Failing stream.");
                let error = "流连接意外断开且本地缓存不可用".to_string();
                return Err(streaming_failure(
                    &mut aurora_buffer,
                    &mut pending_aurora_chunk,
                    error,
                ));
            }
            State::Retrying => {
                const MAX_RETRIES: u32 = 3;
                if retry_count >= MAX_RETRIES {
                    log::error!(
                        "[VCPClient] Max retries reached ({}) for message: {}",
                        MAX_RETRIES,
                        message_id_inner
                    );
                    return Err(streaming_failure(
                        &mut aurora_buffer,
                        &mut pending_aurora_chunk,
                        "Max retries reached".to_string(),
                    ));
                }

                retry_count += 1;
                log::info!(
                    "[VCPClient] Reconnecting {}/{} for message: {}",
                    retry_count,
                    MAX_RETRIES,
                    message_id_inner
                );

                send_stream_event(StreamEvent {
                    r#type: "reconnecting".into(),
                    message_id: message_id_inner.clone(),
                    context: context_inner.clone(),
                    ..Default::default()
                });

                tokio::select! {
                    _ = cancellation_token.cancelled() => {
                        log::warn!("[VCPClient] Aborted during retry backoff sleep");
                        #[cfg(target_os = "android")]
                        {
                            if let Err(error) = send_stop_to_helper(
                                _app,
                                &transport_request_id,
                                helper_generation,
                            ).await {
                                log::warn!("[VCPClient] Best-effort helper stop during retry cancellation failed: {}", error);
                            }
                        }
                        flush_aurora_parse(&mut aurora_buffer, &mut pending_aurora_chunk, &mut last_aurora_parse, true);
                        aurora_buffer.finalize();
                        send_aurora_update(
                            &mut aurora_buffer,
                            Some("cancelled_by_user".to_string()),
                        );
                        return Ok((json!({ "fullContent": aurora_buffer.full_text, "finishReason": Some("cancelled_by_user") }), true));
                    }
                    _ = tokio::time::sleep(backoff) => {}
                }
                backoff *= 2;
                state = State::Resuming;
            }
        }
    }
}

/// 4. 抽离非流式请求循环
#[allow(clippy::too_many_arguments)]
async fn handle_non_streaming_request(
    client: Client,
    final_url: &str,
    api_key: &str,
    request_body: Value,
    message_id: String,
    context: Option<Value>,
    cancellation_token: CancellationToken,
    stream_channel: Option<Channel<StreamEvent>>,
) -> Result<(Value, bool), String> {
    let send_stream_event = |event: StreamEvent| {
        if let Some(ref ch) = stream_channel {
            let _ = ch.send(event);
        }
    };

    let request_future = client
        .post(final_url)
        .header(AUTHORIZATION, format!("Bearer {}", api_key))
        .header(CONTENT_TYPE, "application/json")
        .json(&request_body)
        .send();

    let response = tokio::select! {
        _ = cancellation_token.cancelled() => {
            log::warn!("[VCPClient] Non-streaming request aborted before response for message: {}", message_id);
            // 用户取消由上层统一 Finalizer 持久化并发送唯一终态事件。
            return Ok((
                json!({
                    "response": serde_json::Value::Null,
                    "fullContent": "",
                    "finishReason": "cancelled_by_user",
                    "context": context
                }),
                true,
            ));
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
        let text = response.text().await.unwrap_or_default();
        let err_msg = format!("VCP服务器错误: {} - {}", status, text);
        return Err(err_msg);
    }

    let vcp_response = response
        .json::<Value>()
        .await
        .map_err(|error| format!("JSON解析失败: {error}"))?;

    // 从标准的 OpenAI 格式中提取文本和结束原因
    let choices = vcp_response["choices"].as_array();
    let first_choice = choices.and_then(|c| c.first());
    let full_content = first_choice
        .and_then(|choice| choice["message"]["content"].as_str())
        .unwrap_or("")
        .to_string();
    let finish_reason = first_choice
        .and_then(|choice| choice["finish_reason"].as_str())
        .map(|r| {
            if r == "stop" {
                "completed".to_string()
            } else {
                r.to_string()
            }
        });

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

    Ok((
        json!({
            "response": vcp_response,
            "fullContent": full_content,
            "finishReason": finish_reason,
            "context": context
        }),
        false,
    ))
}

/// 中止请求 Command: interruptRequest
/// 通过完整消息身份立即取消对应 outer request 的共享取消域
#[tauri::command]
#[allow(non_snake_case)]
pub fn interruptRequest(
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
    log::info!(
        "[VCPClient] interruptRequest called for messageId: {}. Active requests: {}",
        message_id,
        state.0.len()
    );
    if state.cancel(&key)? {
        log::info!(
            "[VCPClient] Found AbortController for messageId: {}, aborting...",
            message_id
        );
        log::info!(
            "[VCPClient] Request interrupted for messageId: {}. Active requests: {}",
            message_id,
            state.0.len()
        );
        Ok(json!({"success": true, "message": format!("Request {} interrupted", message_id)}))
    } else {
        log::warn!(
            "[VCPClient] No active request found for messageId: {}",
            message_id
        );
        Err(format!("Request {} not found", message_id))
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
}

#[tauri::command]
pub async fn get_active_generations(
    app: tauri::AppHandle,
    active_requests: tauri::State<'_, ActiveRequests>,
) -> Result<Vec<ActiveGeneration>, String> {
    let db = app.state::<DbState>();
    let rows = sqlx::query(
        "SELECT ag.msg_id, ag.topic_id, ag.owner_id, ag.owner_type, ag.created_at, \
                m.agent_id, COALESCE(m.name, a.name) AS agent_name \
         FROM active_generations ag \
         LEFT JOIN messages m ON m.owner_type = ag.owner_type \
            AND m.owner_id = ag.owner_id AND m.topic_id = ag.topic_id AND m.msg_id = ag.msg_id \
         LEFT JOIN agents a ON a.owner_type = 'agent' AND a.agent_id = m.agent_id \
         ORDER BY ag.created_at ASC",
    )
    .fetch_all(&db.pool)
    .await
    .map_err(|e| e.to_string())?;

    let mut list = Vec::new();
    for row in rows {
        use sqlx::Row;
        let msg_id: String = row.get("msg_id");
        let topic_id: String = row.get("topic_id");
        let owner_id: String = row.get("owner_id");
        let owner_type: String = row.get("owner_type");
        let key = MessageKey::new(TopicKey::new(&owner_type, &owner_id, &topic_id), &msg_id);
        // 过滤掉当前正在活跃运行的后台流式任务，它们由 sse helper 代理，并不是“被异常打断”的
        if active_requests.0.contains_key(&key) {
            continue;
        }
        list.push(ActiveGeneration {
            msg_id,
            topic_id,
            owner_id,
            owner_type,
            created_at: row.get("created_at"),
            agent_id: row.get("agent_id"),
            agent_name: row.get("agent_name"),
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
) -> Result<Option<String>, String> {
    use sqlx::Row;

    let pending = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(
            SELECT 1 FROM active_generations
            WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?
         )",
    )
    .bind(&key.topic.owner_type)
    .bind(&key.topic.owner_id)
    .bind(&key.topic.topic_id)
    .bind(&key.msg_id)
    .fetch_one(pool)
    .await
    .map_err(|e| e.to_string())?;

    if pending {
        let agent_id_row = sqlx::query(
            "SELECT agent_id FROM messages
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?",
        )
        .bind(&key.topic.owner_type)
        .bind(&key.topic.owner_id)
        .bind(&key.topic.topic_id)
        .bind(&key.msg_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| e.to_string())?;
        let agent_id = agent_id_row.and_then(|r| r.get::<Option<String>, _>("agent_id"));

        let error = if error.trim().is_empty() {
            "生成意外中断"
        } else {
            error.as_str()
        };
        let final_content = stream_error_content(&partial_content, error);

        crate::vcp_modules::chat::message_service::finalize_stream_message(
            app_handle.clone(),
            pool,
            key,
            final_content.clone(),
            false,
            Some("error".to_string()),
            stream_channel,
            agent_id,
        )
        .await?;
        return Ok(Some(final_content));
    } else {
        // Another owner may have committed the terminal message after recovery began.
        // Terminal rows are immutable: a late recovery must be an idempotent no-op.
        log::info!(
            "[VCPClient] Generation {} is no longer pending; skipping late error finalization.",
            key.msg_id
        );
    }
    Ok(None)
}

fn clean_old_cache_files(cache_dir: &std::path::Path) {
    let sse_cache_dir = cache_dir.join("sse_cache");
    if !sse_cache_dir.exists() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(sse_cache_dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() || path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        let Ok(elapsed) = modified.elapsed() else {
            continue;
        };
        if elapsed.as_secs() > 24 * 3600 {
            log::info!(
                "[VCPClient] Deleting orphaned cache file older than 24 hours: {:?}",
                path
            );
            let _ = std::fs::remove_file(path);
        }
    }
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn recover_active_generation<R: Runtime>(
    app: AppHandle<R>,
    active_requests: tauri::State<'_, ActiveRequests>,
    owner_id: String,
    owner_type: String,
    topic_id: String,
    msg_id: String,
    stream_channel: Channel<StreamEvent>,
    is_warm: bool,
) -> Result<Value, String> {
    log::info!(
        "[VCPClient] recover_active_generation called for {}/{}/{}/{}",
        owner_type,
        owner_id,
        topic_id,
        msg_id,
    );
    let key = MessageKey::new(TopicKey::new(&owner_type, &owner_id, &topic_id), &msg_id);
    let transport_request_id = message_transport_request_id(&key);

    // Recovery owns one atomic attempt from inspection through terminal commit.
    // This removes the old recover -> resume two-IPC claim gap.
    let (_recovery_lease, _recovery_cancellation_token) =
        match ActiveRequestLease::try_acquire(active_requests.0.clone(), key.clone()) {
            Ok(claim) => claim,
            Err(_) => {
                log::info!(
                    "[VCPClient] Active generation {} already has an owner.",
                    msg_id
                );
                return Ok(json!({ "status": "already_running" }));
            }
        };

    let db = app.state::<DbState>();
    let pending: Option<(Option<String>,)> = sqlx::query_as(
        "SELECT m.agent_id \
         FROM active_generations ag \
         LEFT JOIN messages m ON m.owner_type = ag.owner_type AND m.owner_id = ag.owner_id \
            AND m.topic_id = ag.topic_id AND m.msg_id = ag.msg_id \
         WHERE ag.owner_type = ? AND ag.owner_id = ? AND ag.topic_id = ? AND ag.msg_id = ?",
    )
    .bind(&owner_type)
    .bind(&owner_id)
    .bind(&topic_id)
    .bind(&msg_id)
    .fetch_optional(&db.pool)
    .await
    .map_err(|e| e.to_string())?;

    let Some((agent_id,)) = pending else {
        let terminal: Option<(String, Option<String>)> = sqlx::query_as(
            "SELECT content, finish_reason FROM messages
                 WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?",
        )
        .bind(&owner_type)
        .bind(&owner_id)
        .bind(&topic_id)
        .bind(&msg_id)
        .fetch_optional(&db.pool)
        .await
        .map_err(|e| e.to_string())?;
        return Ok(match terminal {
            Some((content, Some(finish_reason))) => json!({
                "status": "completed",
                "content": content,
                "finishReason": finish_reason,
            }),
            Some((content, None)) => json!({ "status": "failed", "content": content }),
            None => json!({ "status": "not_found", "content": "" }),
        });
    };

    #[cfg(not(target_os = "android"))]
    let _ = is_warm;

    let cache_dir = app.path().app_cache_dir().map_err(|e| e.to_string())?;

    // 异步清理超过 24 小时的孤立缓存文件
    let cache_dir_clone = cache_dir.clone();
    tokio::spawn(async move {
        clean_old_cache_files(&cache_dir_clone);
    });

    // 2. 检查是否存在 5 分钟超时后由助手转存的本地 JSON 恢复文件 (24小时内认领有效)
    let safe_msg_id =
        crate::vcp_modules::infra::utils::calculate_sha256(transport_request_id.as_bytes());
    let recovered_file = cache_dir
        .join("sse_cache")
        .join(format!("sse_recovered_{}.json", safe_msg_id));
    if recovered_file.exists() {
        log::info!(
            "[VCPClient] Found local sse_recovered JSON file for msg_id: {}. Recovering from disk.",
            msg_id
        );
        if let Ok(content_str) = std::fs::read_to_string(&recovered_file) {
            if let Ok(val) = serde_json::from_str::<Value>(&content_str) {
                let timestamp = val["timestamp"].as_i64().unwrap_or(0);
                let now = chrono::Utc::now().timestamp_millis();

                // 检查是否超过 24 小时 (24 * 3600 * 1000 ms)
                if now - timestamp > 24 * 3600 * 1000 {
                    log::warn!("[VCPClient] Recovered JSON file is older than 24 hours. Deleting and failing.");
                    let _ = std::fs::remove_file(&recovered_file);
                } else {
                    let content = val["content"].as_str().unwrap_or("").to_string();
                    let finish_reason = val["finishReason"].as_str().map(|s| s.to_string());

                    log::info!("[VCPClient] Successfully read recovered JSON: content_len={}, finish_reason={:?}", content.len(), finish_reason);

                    crate::vcp_modules::chat::message_service::finalize_stream_message(
                        app.clone(),
                        &db.pool,
                        &key,
                        content.clone(),
                        false,
                        finish_reason.or(Some("completed".to_string())),
                        Some(stream_channel.clone()),
                        agent_id.clone(),
                    )
                    .await?;

                    let _ = std::fs::remove_file(&recovered_file);
                    return Ok(json!({
                        "status": "completed",
                        "content": content
                    }));
                }
            }
        }
    }

    // 3. 在 Android 上通过 TCP 套接字向助手查询该会话状态 (5 分钟内的内存数据)
    #[cfg(target_os = "android")]
    {
        log::info!(
            "[VCPClient] Querying helper process via TCP for msg_id: {}",
            msg_id
        );
        let query_res = async {
            let port = get_helper_port(&app)?;
            log::info!("[VCPClient] Helper port discovered: {}", port);
            let stream = connect_helper_port(port).await?;

            let stream =
                send_command_to_stream(stream, "query", &transport_request_id, None).await?;

            log::info!("[VCPClient] Query command sent, waiting for response frame...");
            let mut reader = FramedRead::new(stream, helper_frame_codec());
            let line = tokio::time::timeout(HELPER_IO_TIMEOUT, reader.next())
                .await
                .map_err(|_| "Helper query response timed out".to_string())?
                .ok_or_else(|| "No query response received (EOF)".to_string())?
                .map_err(|e| format!("Helper query frame error: {}", e))?;
            let resp = serde_json::from_slice::<Value>(&line).map_err(|e| e.to_string())?;
            if resp.get("requestId").and_then(Value::as_str) != Some(transport_request_id.as_str())
            {
                return Err("Helper query response identity mismatch".to_string());
            }
            Ok::<Value, String>(resp)
        }
        .await;

        match query_res {
            Ok(resp) => {
                let status = resp["status"].as_str().unwrap_or("not_found");
                let content = resp["content"].as_str().unwrap_or("").to_string();
                let last_finish_reason = resp["lastFinishReason"].as_str().map(|s| s.to_string());

                log::info!(
                    "[VCPClient] Query response received: status={}, content_len={}, finish_reason={:?}",
                    status,
                    content.len(),
                    last_finish_reason
                );

                if status == "completed" {
                    let helper_generation = resp["generation"]
                        .as_u64()
                        .filter(|generation| *generation > 0)
                        .ok_or_else(|| {
                            "Completed helper session is missing generation".to_string()
                        })?;
                    log::info!("[VCPClient] Session completed in helper memory. Finalizing message in SQLite database.");
                    crate::vcp_modules::chat::message_service::finalize_stream_message(
                        app.clone(),
                        &db.pool,
                        &key,
                        content.clone(),
                        false,
                        last_finish_reason.or(Some("completed".to_string())),
                        Some(stream_channel.clone()),
                        agent_id.clone(),
                    )
                    .await?;

                    log::info!("[VCPClient] Finalization complete. Sending stop command to helper to release memory.");
                    if let Err(stop_error) =
                        send_stop_to_helper(&app, &transport_request_id, Some(helper_generation))
                            .await
                    {
                        log::warn!(
                            "[VCPClient] Best-effort helper cleanup after durable finalization failed: {}",
                            stop_error
                        );
                    }

                    return Ok(json!({
                        "status": "completed",
                        "content": content
                    }));
                } else if status == "streaming" {
                    log::info!("[VCPClient] Session is still streaming in helper. Claiming and resuming it atomically.");
                    let initial_content = is_warm.then(|| content.clone());
                    let last_event_index = if is_warm {
                        resp["lastEventIndex"].as_i64()
                    } else {
                        None
                    };
                    let resumed = resume_claimed_generation(
                        &app,
                        &key,
                        agent_id.clone(),
                        stream_channel.clone(),
                        initial_content,
                        content,
                        last_event_index,
                        _recovery_cancellation_token,
                    )
                    .await?;
                    let resumed_status = resumed["status"].as_str().unwrap_or("completed");
                    return Ok(json!({
                        "status": resumed_status,
                        "content": resumed["fullContent"],
                        "finishReason": resumed["finishReason"],
                    }));
                } else {
                    log::warn!("[VCPClient] Session status is 'not_found' in helper.");
                }
            }
            Err(e) => {
                log::warn!("[VCPClient] Failed to query helper via TCP socket: {}", e);
            }
        }
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
    )
    .await?;

    let content: Option<String> = sqlx::query_scalar(
        "SELECT content FROM messages
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?",
    )
    .bind(&owner_type)
    .bind(&owner_id)
    .bind(&topic_id)
    .bind(&msg_id)
    .fetch_optional(&db.pool)
    .await
    .map_err(|e| e.to_string())?;
    Ok(json!({ "status": "failed", "content": content.unwrap_or_default() }))
}

#[allow(clippy::too_many_arguments)]
#[cfg(target_os = "android")]
async fn resume_claimed_generation<R: Runtime>(
    app: &AppHandle<R>,
    key: &MessageKey,
    agent_id: Option<String>,
    stream_channel: Channel<StreamEvent>,
    initial_content: Option<String>,
    helper_content: String,
    last_event_index: Option<i64>,
    cancellation_token: CancellationToken,
) -> Result<Value, String> {
    let msg_id = key.msg_id.clone();
    let topic_id = key.topic.topic_id.clone();
    let owner_id = key.topic.owner_id.clone();
    let owner_type = key.topic.owner_type.clone();
    log::info!(
        "[VCPClient] resume_claimed_generation called for messageId: {}, topicId: {}, lastEventIndex: {:?}",
        msg_id,
        topic_id,
        last_event_index
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

    let (res, is_aborted) = match handle_streaming_request(
        app,
        client,
        "",
        "",
        Value::Null,
        msg_id.clone(),
        transport_request_id,
        Some(context.clone()),
        cancellation_token,
        Some(stream_channel.clone()),
        true,
        last_event_index,
        initial_content.clone(),
    )
    .await
    {
        Ok(val) => val,
        Err(failure) => {
            log::error!(
                "[VCPClient] Claimed recovery failed during handle_streaming_request: {}",
                failure
            );
            let (error, partial_content) = failure.into_parts();
            let partial_content = merge_recovery_partial(&helper_content, partial_content);
            let committed_content = finalize_stream_error(
                app,
                &pool,
                key,
                partial_content.clone(),
                format!("接续失败: {}", error),
                Some(stream_channel.clone()),
            )
            .await?;
            return Ok(json!({
                "status": "failed",
                "fullContent": committed_content.unwrap_or(partial_content),
                "finishReason": "error",
            }));
        }
    };

    let finish_reason = if is_aborted {
        Some("cancelled_by_user".to_string())
    } else {
        res["finishReason"].as_str().map(|s| s.to_string())
    };

    log::info!("[VCPClient] Claimed recovery completed. Finalizing message.");
    crate::vcp_modules::chat::message_service::finalize_stream_message(
        app.clone(),
        &pool,
        key,
        res["fullContent"].as_str().unwrap_or("").to_string(),
        is_aborted,
        finish_reason,
        Some(stream_channel),
        agent_id,
    )
    .await?;

    Ok(res)
}

#[cfg(test)]
mod active_request_tests {
    use super::*;

    fn message_key(message_id: &str) -> MessageKey {
        MessageKey::new(TopicKey::new("agent", "agent-a", "topic-a"), message_id)
    }

    #[test]
    fn endpoint_modes_cover_base_full_prefix_port_and_query_urls() {
        let cases = [
            (
                "https://example.invalid",
                "https://example.invalid/v1/chat/completions",
                "https://example.invalid/v1/chatvcp/completions",
            ),
            (
                "https://example.invalid/v1/chat/completions",
                "https://example.invalid/v1/chat/completions",
                "https://example.invalid/v1/chatvcp/completions",
            ),
            (
                "http://example.invalid:6005/proxy?tenant=mobile",
                "http://example.invalid:6005/proxy/v1/chat/completions?tenant=mobile",
                "http://example.invalid:6005/proxy/v1/chatvcp/completions?tenant=mobile",
            ),
            (
                "https://example.invalid:443/proxy",
                "https://example.invalid:443/proxy/v1/chat/completions",
                "https://example.invalid:443/proxy/v1/chatvcp/completions",
            ),
            (
                "https://example.invalid/proxy%20space?tenant=mobile",
                "https://example.invalid/proxy%20space/v1/chat/completions?tenant=mobile",
                "https://example.invalid/proxy%20space/v1/chatvcp/completions?tenant=mobile",
            ),
            (
                "https://example.invalid/proxy/v1",
                "https://example.invalid/proxy/v1/chat/completions",
                "https://example.invalid/proxy/v1/chatvcp/completions",
            ),
            (
                "https://example.invalid/proxy/v1/chatvcp/completions/?key=value",
                "https://example.invalid/proxy/v1/chat/completions?key=value",
                "https://example.invalid/proxy/v1/chatvcp/completions?key=value",
            ),
        ];

        for (raw, standard, vcp_tools) in cases {
            assert_eq!(
                resolve_chat_endpoint(
                    raw,
                    ChatEndpointMode::Standard,
                    ChatRequestPurpose::Interactive,
                )
                .expect("standard endpoint"),
                standard
            );
            assert_eq!(
                resolve_chat_endpoint(
                    raw,
                    ChatEndpointMode::VcpTools,
                    ChatRequestPurpose::Interactive,
                )
                .expect("VCP tools endpoint"),
                vcp_tools
            );
            assert_eq!(
                resolve_chat_endpoint(raw, ChatEndpointMode::Raw, ChatRequestPurpose::Interactive,)
                    .expect("raw endpoint"),
                raw
            );
        }
    }

    #[test]
    fn request_purpose_matrix_matches_vchat_semantics() {
        let raw = "https://example.invalid/proxy";
        let cases = [
            (
                ChatEndpointMode::Standard,
                ChatRequestPurpose::Interactive,
                "https://example.invalid/proxy/v1/chat/completions",
            ),
            (
                ChatEndpointMode::Standard,
                ChatRequestPurpose::Auxiliary,
                "https://example.invalid/proxy/v1/chat/completions",
            ),
            (
                ChatEndpointMode::VcpTools,
                ChatRequestPurpose::Interactive,
                "https://example.invalid/proxy/v1/chatvcp/completions",
            ),
            (
                ChatEndpointMode::VcpTools,
                ChatRequestPurpose::Auxiliary,
                "https://example.invalid/proxy/v1/chat/completions",
            ),
            (ChatEndpointMode::Raw, ChatRequestPurpose::Interactive, raw),
            (ChatEndpointMode::Raw, ChatRequestPurpose::Auxiliary, raw),
        ];

        for (mode, purpose, expected) in cases {
            assert_eq!(
                resolve_chat_endpoint(raw, mode, purpose).expect("purpose endpoint"),
                expected
            );
        }
    }

    #[test]
    fn raw_mode_is_byte_for_byte_unchanged() {
        let raw = "HTTP://Example.INVALID:8443/custom/%2f?signature=a%2Fb&x=1";
        assert_eq!(
            resolve_chat_endpoint(raw, ChatEndpointMode::Raw, ChatRequestPurpose::Interactive,)
                .expect("raw endpoint"),
            raw
        );
    }

    #[test]
    fn model_discovery_is_prefix_aware_and_conservative_in_raw_mode() {
        assert_eq!(
            resolve_model_discovery_endpoint(
                "https://example.invalid/proxy?tenant=mobile",
                ChatEndpointMode::VcpTools,
            )
            .expect("derived discovery"),
            Some("https://example.invalid/proxy/v1/models?tenant=mobile".to_string())
        );
        assert_eq!(
            resolve_model_discovery_endpoint(
                "https://example.invalid/proxy/v1/chat/completions?tenant=mobile",
                ChatEndpointMode::Raw,
            )
            .expect("safe raw discovery"),
            Some("https://example.invalid/proxy/v1/models?tenant=mobile".to_string())
        );
        assert_eq!(
            resolve_model_discovery_endpoint(
                "https://example.invalid/custom/gateway",
                ChatEndpointMode::Raw,
            )
            .expect("unsafe raw discovery"),
            None
        );
    }

    #[test]
    fn endpoint_validation_rejects_unsafe_or_non_http_urls() {
        for raw in [
            "ftp://example.invalid/v1/chat/completions",
            "https://user:secret@example.invalid/v1/chat/completions",
            "https://example.invalid/v1/chat/completions#fragment",
            "https://example.invalid/v1/chat/completions\n",
            "https://",
        ] {
            assert!(
                resolve_chat_endpoint(raw, ChatEndpointMode::Raw, ChatRequestPurpose::Interactive,)
                    .is_err(),
                "unexpectedly accepted {raw}"
            );
        }
    }

    #[test]
    fn streaming_failure_captures_buffer_and_unflushed_pending_content() {
        let mut buffer = AuroraBuffer::new();
        buffer.append_chunk("abc");
        let mut pending = "def".to_string();

        let failure = streaming_failure(&mut buffer, &mut pending, "network failed");
        let (message, partial_content) = failure.into_parts();
        assert_eq!(message, "network failed");
        assert_eq!(partial_content.as_deref(), Some("abcdef"));
        assert!(pending.is_empty());
    }

    #[test]
    fn aurora_parse_interval_is_fixed_at_30hz() {
        assert_eq!(AURORA_PARSE_INTERVAL, Duration::from_millis(33));
    }

    #[test]
    fn aurora_parse_deadline_is_anchored_to_the_last_parse() {
        assert_eq!(
            remaining_aurora_parse_delay(Duration::from_millis(10)),
            Duration::from_millis(23)
        );
        assert_eq!(
            remaining_aurora_parse_delay(Duration::from_millis(99)),
            Duration::ZERO
        );
    }

    #[test]
    fn recovery_partial_only_advances_along_the_helper_prefix() {
        assert_eq!(
            merge_recovery_partial("abc", Some("abcdef".to_string())),
            "abcdef"
        );
        assert_eq!(
            merge_recovery_partial("abcdef", Some("abc".to_string())),
            "abcdef"
        );
        assert_eq!(
            merge_recovery_partial("helper", Some("diverged".to_string())),
            "helper"
        );
    }

    #[test]
    fn stream_error_content_appends_one_terminal_suffix() {
        let final_content = stream_error_content("partial", "network failed");
        assert_eq!(final_content, "partial\n\n> VCP流式错误: network failed");
        assert_eq!(
            stream_error_content(&final_content, "network failed"),
            final_content
        );
        assert_eq!(
            stream_error_content("", "network failed"),
            "\n\n> VCP流式错误: network failed"
        );
    }

    #[test]
    fn durable_end_serializes_the_committed_content() {
        let event = StreamEvent::end(
            "message-4".to_string(),
            None,
            Some("error".to_string()),
            Some("committed".to_string()),
            Some(Vec::new()),
            Some(123),
            Some(456),
        );
        let wire = serde_json::to_value(event).expect("serialize durable end");
        assert_eq!(wire["type"], "end");
        assert_eq!(wire["content"], "committed");
        assert_eq!(wire["finishReason"], "error");
        assert_eq!(wire["timestamp"], 123);
        assert_eq!(wire["topicUpdatedAt"], 456);
    }

    #[test]
    fn local_attachment_metadata_is_internal_and_labels_never_reveal_parent_paths() {
        let mut message = json!({
            "role": "user",
            "content": "hello",
            "__vcpLocalAttachments": [{
                "sha256": "a".repeat(64),
                "hostPath": "/data/user/0/private/secret.png"
            }]
        });
        remove_internal_local_attachment_metadata(&mut message);
        let wire = serde_json::to_string(&message).expect("serialize model message");
        assert!(!wire.contains("__vcpLocalAttachments"));
        assert!(!wire.contains("/data/user/0/private"));
        assert_eq!(
            bounded_attachment_label("/data/user/0/private/\nphoto.png"),
            "photo.png"
        );
        assert_eq!(
            short_hash(&format!("{}not-a-hash", "a".repeat(64))),
            "a".repeat(12)
        );
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

    #[test]
    fn helper_stop_ack_requires_stopped_and_exact_generation() {
        let valid = json!({
            "action": "stop_ack",
            "requestId": "message-3",
            "generation": 17,
            "stopped": true,
        });
        assert!(validate_helper_stop_ack(&valid, "message-3", 17).is_ok());

        let stale_generation = json!({
            "action": "stop_ack",
            "requestId": "message-3",
            "generation": 16,
            "stopped": true,
        });
        assert!(validate_helper_stop_ack(&stale_generation, "message-3", 17).is_err());

        let not_stopped = json!({
            "action": "stop_ack",
            "requestId": "message-3",
            "generation": 17,
            "stopped": false,
        });
        assert!(validate_helper_stop_ack(&not_stopped, "message-3", 17).is_err());
    }

    #[test]
    fn helper_collision_adopts_only_the_exact_unversioned_start_error() {
        assert_eq!(
            classify_helper_start_error("Session already exists", None),
            HelperStartErrorDisposition::AdoptExistingSession
        );
        assert_eq!(
            classify_helper_start_error("Session already exists", Some(7)),
            HelperStartErrorDisposition::Fail
        );
        assert_eq!(
            classify_helper_start_error("Session not found", None),
            HelperStartErrorDisposition::Fail
        );
    }
}

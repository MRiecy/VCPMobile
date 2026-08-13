use crate::vcp_modules::infra::utils::normalize_vcp_url;
use crate::vcp_modules::media_processor::convert_local_image_for_multimodal;
use dashmap::mapref::entry::Entry;
use dashmap::{DashMap, DashSet};
use futures_util::StreamExt;
use futures_util::TryStreamExt;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use std::sync::Arc;
use std::time::Duration;
use tauri::{ipc::Channel, AppHandle, Manager, Runtime};
#[cfg(target_os = "android")]
use tokio_util::codec::LengthDelimitedCodec;
use tokio_util::codec::{FramedRead, LinesCodec};
use tokio_util::io::StreamReader;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::vcp_modules::aurora_pipeline::{AuroraBuffer, AuroraUpdate};
use crate::vcp_modules::content_parser::ContentBlock;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::settings_manager::{
    read_settings, MobileCliAgentRoute, Settings, SettingsState,
};
use crate::vcp_modules::stream_block_parser::{StreamBlock, StreamBlockParser};

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
    pub vcp_url: String,        // VCP服务器URL
    pub vcp_api_key: String,    // API密钥
    pub messages: Vec<Value>,   // 消息数组
    pub model_config: Value,    // 模型配置 (包含 model, stream, temperature 等)
    pub message_id: String,     // 消息ID (用于跟踪和中止)
    pub context: Option<Value>, // 上下文信息 (agentId, topicId等)
    /// 每个模型 step 的内部网络/helper 身份；不进入 StreamEvent 或 DB 可见身份。
    #[serde(default)]
    pub transport_request_id: Option<String>,
    /// 本地多 step turn 的事件投影；旧调用省略时保持现有 wire。
    #[serde(default)]
    pub turn_attempt: Option<String>,
    #[serde(default)]
    pub step_index: Option<u32>,
    #[serde(default)]
    pub projection_reset: Option<bool>,
    /// Coordinator 在 turn 开始时冻结；`None` 仅保留旧调用的设置兼容路径。
    #[serde(default)]
    pub mobile_cli_agent_route: Option<MobileCliAgentRoute>,
    /// 仅供 Rust coordinator 投影已持久化的工具结果；不接受 IPC/wire 输入，也不进入模型请求。
    #[serde(skip)]
    pub local_cli_projection_prefix: Option<String>,
}

const TYPED_NON_STREAM_BODY_OVERHEAD_BYTES: usize = 64 * 1024;
pub(crate) const TYPED_ASSISTANT_BUDGET_ERROR: &str = "模型单步输出超过 512 KiB 安全上限";
const VCP_TOOL_USE_FORBIDDEN_SENTINEL: &str = "[[VCPToolUse=Forbidden]]";

pub(crate) fn is_typed_assistant_budget_error(error: &str) -> bool {
    error.starts_with(TYPED_ASSISTANT_BUDGET_ERROR)
}

fn inject_local_loopback_transport_guard(messages: &mut Vec<Value>) {
    if messages
        .first()
        .and_then(|message| message["role"].as_str())
        != Some("system")
    {
        messages.insert(
            0,
            json!({
                "role": "system",
                "content": VCP_TOOL_USE_FORBIDDEN_SENTINEL,
            }),
        );
        return;
    }

    let Some(content) = messages[0].get_mut("content") else {
        messages[0]["content"] = json!(VCP_TOOL_USE_FORBIDDEN_SENTINEL);
        return;
    };
    match content {
        Value::String(text) => {
            if !text.contains(VCP_TOOL_USE_FORBIDDEN_SENTINEL) {
                if text.is_empty() {
                    *text = VCP_TOOL_USE_FORBIDDEN_SENTINEL.to_string();
                } else {
                    *text = format!("{VCP_TOOL_USE_FORBIDDEN_SENTINEL}\n{text}");
                }
            }
        }
        Value::Array(parts) => {
            let already_present = parts.iter().any(|part| {
                part.get("type").and_then(Value::as_str) == Some("text")
                    && part
                        .get("text")
                        .and_then(Value::as_str)
                        .is_some_and(|text| text.contains(VCP_TOOL_USE_FORBIDDEN_SENTINEL))
            });
            if !already_present {
                parts.insert(
                    0,
                    json!({"type": "text", "text": VCP_TOOL_USE_FORBIDDEN_SENTINEL}),
                );
            }
        }
        _ => *content = json!(VCP_TOOL_USE_FORBIDDEN_SENTINEL),
    }
}

fn should_inject_local_loopback_transport_guard(
    route: MobileCliAgentRoute,
    metadata: &StreamTurnMetadata,
) -> bool {
    route == MobileCliAgentRoute::LocalLoopback && metadata.turn_attempt.is_some()
}

#[derive(Debug, Clone, Default)]
struct LocalCliAuroraProjection {
    prefix: String,
    stable_blocks: Vec<StreamBlock>,
}

impl LocalCliAuroraProjection {
    fn from_prefix(prefix: Option<String>) -> Self {
        let prefix = prefix.filter(|value| !value.is_empty()).unwrap_or_default();
        let stable_blocks = if prefix.is_empty() {
            Vec::new()
        } else {
            StreamBlockParser::new().finalize(&prefix)
        };
        Self {
            prefix,
            stable_blocks,
        }
    }

    fn apply(&self, update: &mut AuroraUpdate) {
        if self.prefix.is_empty() {
            return;
        }
        if let Some(content) = update.content.take() {
            update.content = Some(self.full_content(&content));
        }
        if let Some(model_blocks) = update.stable_blocks.take() {
            let mut projected = self.stable_blocks.clone();
            projected.extend(model_blocks);
            update.stable_blocks = Some(projected);
        }
    }

    fn full_content(&self, model_content: &str) -> String {
        format!("{}{}", self.prefix, model_content)
    }
}

fn enforce_typed_assistant_budget(
    metadata: &StreamTurnMetadata,
    buffered_bytes: usize,
    pending_bytes: usize,
    incoming_bytes: usize,
) -> Result<(), String> {
    if metadata.turn_attempt.is_none() {
        return Ok(());
    }
    let projected = buffered_bytes
        .checked_add(pending_bytes)
        .and_then(|value| value.checked_add(incoming_bytes));
    if projected
        .is_none_or(|value| value > crate::vcp_modules::cli::turn_types::MAX_ASSISTANT_STEP_BYTES)
    {
        return Err(TYPED_ASSISTANT_BUDGET_ERROR.to_string());
    }
    Ok(())
}

fn append_bounded_response_chunk(
    body: &mut Vec<u8>,
    chunk: &[u8],
    max_bytes: usize,
) -> Result<(), String> {
    if body
        .len()
        .checked_add(chunk.len())
        .is_none_or(|value| value > max_bytes)
    {
        return Err(format!("模型响应体超过 {} 字节安全上限", max_bytes));
    }
    body.extend_from_slice(chunk);
    Ok(())
}

async fn read_response_body_bounded(
    response: reqwest::Response,
    max_bytes: usize,
) -> Result<Vec<u8>, String> {
    let mut body = Vec::with_capacity(max_bytes.min(64 * 1024));
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| format!("读取模型响应失败: {error}"))?;
        append_bounded_response_chunk(&mut body, &chunk, max_bytes)?;
    }
    Ok(body)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct StreamTurnMetadata {
    turn_attempt: Option<String>,
    step_index: Option<u32>,
    projection_reset: Option<bool>,
}

fn project_aurora_finish_reason(
    metadata: &StreamTurnMetadata,
    finish_reason: Option<String>,
) -> Option<String> {
    if metadata.turn_attempt.is_some() {
        None
    } else {
        finish_reason
    }
}

fn apply_stream_turn_metadata(event: StreamEvent, metadata: &StreamTurnMetadata) -> StreamEvent {
    event.with_turn_metadata(metadata)
}

fn project_model_step_event(
    event: StreamEvent,
    metadata: &StreamTurnMetadata,
) -> Option<StreamEvent> {
    // Inner model-step failures return through Result to the coordinator. Emitting an `error`
    // frame here would create a false outer terminal in the UI before continuation policy runs.
    if metadata.turn_attempt.is_some() && event.r#type == "error" {
        None
    } else {
        Some(apply_stream_turn_metadata(event, metadata))
    }
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

    fn stream_turn_metadata(&self) -> StreamTurnMetadata {
        StreamTurnMetadata {
            turn_attempt: self.turn_attempt.clone(),
            step_index: self.step_index,
            projection_reset: self.projection_reset,
        }
    }
}

fn resolve_vcp_endpoint(raw_url: &str, frozen_route: MobileCliAgentRoute) -> String {
    if frozen_route == MobileCliAgentRoute::VcpPlugin {
        let mut final_url = raw_url.to_string();
        if let Ok(mut url) = Url::parse(raw_url) {
            url.set_path("/v1/chatvcp/completions");
            final_url = url.to_string();
        }
        final_url
    } else {
        normalize_vcp_url(raw_url)
    }
}

fn resolve_request_route(
    explicit_route: Option<MobileCliAgentRoute>,
    settings: Option<&Settings>,
) -> MobileCliAgentRoute {
    explicit_route
        .or_else(|| settings.map(|settings| settings.mobile_cli_agent_route))
        .unwrap_or_default()
}

/// 流式事件结构体，用于向前端发送数据
#[derive(Debug, Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct StreamEvent {
    pub r#type: String, // 事件类型: "data", "aurora", "end", "error", "reconnecting"
    pub chunk: Option<Value>, // 数据块 (仅 type="data" 时有效)
    pub message_id: String, // 消息ID
    pub context: Option<Value>, // 透传的上下文信息
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_attempt: Option<String>, // 本地工具回环 attempt；旧请求不发送
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_index: Option<u32>, // attempt 内模型 step
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection_reset: Option<bool>, // 当前帧是否重置上一 step 的 UI 投影
    pub finish_reason: Option<String>, // 结束原因
    pub error: Option<String>, // 错误信息 (仅 type="error" 时有效)
    pub aurora: Option<AuroraUpdate>, // Aurora 语义沉淀更新 (type="aurora" 时有效)
    pub blocks: Option<Vec<ContentBlock>>, // 持久化后的预渲染块 (仅 type="end" 时有效)
    pub timestamp: Option<u64>, // ⚡ 新增物理落笔时间戳
}

impl StreamEvent {
    fn with_turn_metadata(mut self, metadata: &StreamTurnMetadata) -> Self {
        self.turn_attempt = metadata.turn_attempt.clone();
        self.step_index = metadata.step_index;
        self.projection_reset = metadata.projection_reset;
        self
    }

    /// Coordinator 的唯一 outer terminal 也必须保留最后一个 step 的 typed wire 身份。
    pub fn with_turn_projection(
        self,
        turn_attempt: String,
        step_index: u32,
        projection_reset: bool,
    ) -> Self {
        self.with_turn_metadata(&StreamTurnMetadata {
            turn_attempt: Some(turn_attempt),
            step_index: Some(step_index),
            projection_reset: Some(projection_reset),
        })
    }

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
        blocks: Option<Vec<ContentBlock>>,
        timestamp: Option<u64>,
    ) -> Self {
        Self {
            r#type: "end".into(),
            message_id,
            context,
            finish_reason,
            blocks,
            timestamp,
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

/// 单次请求注册。`attempt_id` 使迟到任务无法删除同 message id 的新请求。
pub struct ActiveRequestEntry {
    attempt_id: uuid::Uuid,
    cancellation_token: CancellationToken,
}

pub type ActiveRequestMap = Arc<DashMap<String, Arc<ActiveRequestEntry>>>;

/// 全局活跃请求管理器。相同 message id 只允许一个 live attempt。
pub struct ActiveRequests(pub ActiveRequestMap);

impl Default for ActiveRequests {
    fn default() -> Self {
        log::info!("[VCPClient] Initialized ActiveRequests successfully.");
        Self(Arc::new(DashMap::new()))
    }
}

impl ActiveRequests {
    pub(crate) fn cancel(&self, message_id: &str) -> Result<bool, String> {
        let Some(entry) = self.0.get(message_id) else {
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
    message_id: String,
    attempt_id: uuid::Uuid,
}

impl ActiveRequestLease {
    pub fn try_acquire(
        requests: ActiveRequestMap,
        message_id: String,
    ) -> Result<(Self, CancellationToken), String> {
        let attempt_id = uuid::Uuid::new_v4();
        let cancellation_token = CancellationToken::new();
        let entry = Arc::new(ActiveRequestEntry {
            attempt_id,
            cancellation_token: cancellation_token.clone(),
        });

        match requests.entry(message_id.clone()) {
            Entry::Vacant(slot) => {
                slot.insert(entry);
                Ok((
                    Self {
                        requests: requests.clone(),
                        message_id,
                        attempt_id,
                    },
                    cancellation_token,
                ))
            }
            Entry::Occupied(_) => Err(format!(
                "Request {} is already active; duplicate attempt rejected",
                message_id
            )),
        }
    }
}

impl Drop for ActiveRequestLease {
    fn drop(&mut self) {
        if let Entry::Occupied(entry) = self.requests.entry(self.message_id.clone()) {
            if entry.get().attempt_id == self.attempt_id {
                entry.remove();
            }
        }
    }
}

/// 群组回合取消令牌，用于标记需要中断接力赛的话题
/// topicId -> true (存在即代表已取消)
pub struct CancelledGroupTurns(pub Arc<DashSet<String>>);

impl Default for CancelledGroupTurns {
    fn default() -> Self {
        log::info!("[VCPClient] Initialized CancelledGroupTurns successfully.");
        Self(Arc::new(DashSet::new()))
    }
}

/// 中止群组的整个接力赛回合
#[tauri::command]
#[allow(non_snake_case)]
pub fn interruptGroupTurn(
    state: tauri::State<'_, CancelledGroupTurns>,
    topic_id: String,
) -> Result<Value, String> {
    log::info!(
        "[VCPClient] interruptGroupTurn called for topicId: {}",
        topic_id
    );
    state.0.insert(topic_id);
    Ok(json!({"status": "cancelled"}))
}

/// 核心请求函数：sendToVCP
/// 对应 JS 版的 sendToVCP。处理逻辑：
/// 1. 数据验证与规范化 (通过 Rust 类型系统自动处理部分)
/// 2. 按 turn 起点冻结的 typed route 选择普通 completions 或 /v1/chatvcp/completions
/// 3. 上下文注入 (音乐信息、UI 规范要求)
/// 4. 发起 HTTP 请求 (支持流式和非流式)
/// 5. 注册 AbortController 实现中止机制
#[tauri::command]
#[allow(non_snake_case)]
pub async fn sendToVCP<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, ActiveRequests>,
    payload: VcpRequestPayload,
    stream_channel: Channel<StreamEvent>,
) -> Result<Value, String> {
    let message_id = payload.message_id.clone();
    let context = payload.context.clone();
    let is_stream = payload.model_config["stream"].as_bool().unwrap_or(false);

    let (lease, cancellation_token) =
        ActiveRequestLease::try_acquire(state.0.clone(), message_id.clone())?;
    let (res, is_aborted) = match perform_vcp_request_registered(
        &app,
        payload,
        Some(stream_channel.clone()),
        cancellation_token,
    )
    .await
    {
        Ok(val) => val,
        Err(e) => {
            return Err(e);
        }
    };

    if is_stream {
        let finish_reason = if is_aborted {
            Some("cancelled_by_user".to_string())
        } else {
            res["finishReason"].as_str().map(|s| s.to_string())
        };

        // 从 context 解出 owner_id, owner_type, topic_id 并委派统一终结器
        let ctx = context.as_ref();
        let group_id = ctx.and_then(|c| c["groupId"].as_str());
        let agent_id = ctx.and_then(|c| c["agentId"].as_str());
        let topic_id = ctx
            .and_then(|c| c["topicId"].as_str())
            .unwrap_or("")
            .to_string();

        let (owner_id, owner_type) = if let Some(gid) = group_id {
            (gid, "group")
        } else if let Some(aid) = agent_id {
            (aid, "agent")
        } else {
            ("", "agent")
        };

        let pool = app
            .state::<crate::vcp_modules::db_manager::DbState>()
            .pool
            .clone();

        crate::vcp_modules::chat::message_service::finalize_stream_message(
            app.clone(),
            &pool,
            owner_id,
            owner_type,
            topic_id,
            message_id,
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
    payload: VcpRequestPayload,
    stream_channel: Option<Channel<StreamEvent>>,
) -> Result<(Value, bool), String> {
    let (lease, cancellation_token) =
        ActiveRequestLease::try_acquire(active_requests, payload.message_id.clone())?;
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
) -> Result<(Value, bool), String> {
    log::info!(
        "[VCPClient] perform_vcp_request called for messageId: {}, context: {:?}",
        payload.message_id,
        payload.context
    );

    let message_id = payload.message_id.clone();
    let context = payload.context.clone();
    let transport_request_id = payload.effective_transport_request_id().to_string();
    let stream_turn_metadata = payload.stream_turn_metadata();
    let local_cli_projection =
        LocalCliAuroraProjection::from_prefix(payload.local_cli_projection_prefix);

    // === 1. 数据验证和多模态资产转换 ===
    let mut messages = preprocess_multimodal_messages(app, payload.messages).await?;

    // === 2. 读取设置与动态路由切换 ===
    let settings_snapshot = if payload.mobile_cli_agent_route.is_none() {
        load_app_settings(app).await.ok()
    } else {
        None
    };
    let frozen_route =
        resolve_request_route(payload.mobile_cli_agent_route, settings_snapshot.as_ref());
    let final_url = resolve_vcp_endpoint(&payload.vcp_url, frozen_route);

    // === 3. 补充 System 提示词首部 ===
    let has_system = messages.iter().any(|m| m["role"] == "system");
    if !has_system {
        messages.insert(0, json!({"role": "system", "content": ""}));
    }
    // VCPToolBox 的普通 `/v1/chat/completions` 仍会解析并执行显式 TOOL_REQUEST。
    // 本地 coordinator 必须成为唯一工具 owner；该请求级 sentinel 会被 VCPToolBox
    // 在转发给模型前剥离，不写回 Agent prompt，也不用于 vcpPlugin 路由。
    if should_inject_local_loopback_transport_guard(frozen_route, &stream_turn_metadata) {
        inject_local_loopback_transport_guard(&mut messages);
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

    // === 6. 配置网络请求 ===
    let client = Client::builder()
        .tcp_keepalive(Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())?;

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
            stream_turn_metadata,
            local_cli_projection,
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
            stream_turn_metadata,
            local_cli_projection,
            stream_channel,
        )
        .await
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

/// 3. 抽离自适应降帧流式请求循环
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
    stream_turn_metadata: StreamTurnMetadata,
    local_cli_projection: LocalCliAuroraProjection,
    stream_channel: Option<Channel<StreamEvent>>,
    is_resume: bool,
    last_event_index: Option<i64>,
    initial_content: Option<String>,
) -> Result<(Value, bool), String> {
    let send_stream_event = |event: StreamEvent| {
        if let Some(ref ch) = stream_channel {
            if let Some(event) = project_model_step_event(event, &stream_turn_metadata) {
                let _ = ch.send(event);
            }
        }
    };

    let message_id_inner = message_id.clone();
    let context_inner = context.clone();
    // Coordinator 的每个模型 step 都只是 outer turn 的中间帧；唯一终态由 outer
    // finalizer 发出。旧单 step 路径没有 turnAttempt，保留既有 Aurora finishReason。

    let mut last_finish_reason: Option<String> = None;
    #[allow(unused_mut)]
    let mut last_received_index: Option<i64> = last_event_index;
    #[cfg(target_os = "android")]
    let mut helper_generation: Option<u64> = None;
    let mut aurora_buffer = AuroraBuffer::new();
    let mut pending_aurora_chunk = String::new();
    let mut last_aurora_parse = std::time::Instant::now() - Duration::from_millis(33);
    let mut retry_count = 0;
    let mut backoff = Duration::from_millis(500);

    fn adaptive_parse_interval_ms(tail_len: usize) -> u128 {
        match tail_len {
            0..=8_191 => 33,
            8_192..=24_575 => 100,
            _ => 200,
        }
    }
    fn adaptive_force_bytes(tail_len: usize) -> usize {
        match tail_len {
            0..=8_191 => 1024,
            8_192..=24_575 => 4096,
            _ => 8192,
        }
    }

    let send_aurora_update = |buffer: &mut AuroraBuffer,
                              stable_changed: bool,
                              tail_changed: bool,
                              finish_reason: Option<String>,
                              error: Option<String>| {
        let is_final = finish_reason.is_some() || error.is_some();
        let chunk = buffer.take_chunk();
        let tail_frame = buffer.take_tail_frame();
        let tail_snapshot = tail_frame.as_ref().and_then(|frame| frame.snapshot.clone());
        let mut update = AuroraUpdate {
            stable_blocks: if stable_changed {
                Some(buffer.stable_blocks.clone())
            } else {
                None
            },
            stable_changed,
            tail_block: if tail_changed {
                buffer.tail_block.clone()
            } else {
                None
            },
            tail: if tail_changed {
                Some(buffer.tail_content.clone())
            } else {
                None
            },
            tail_changed,
            tail_frame,
            tail_snapshot,
            content: if is_final {
                Some(buffer.full_text.clone())
            } else {
                None
            },
            chunk,
        };
        local_cli_projection.apply(&mut update);
        let mut event =
            StreamEvent::aurora(message_id_inner.clone(), update, context_inner.clone());
        event.finish_reason = project_aurora_finish_reason(&stream_turn_metadata, finish_reason);
        event.error = error;
        send_stream_event(event);
    };

    let flush_aurora_parse = |buffer: &mut AuroraBuffer,
                              pending_chunk: &mut String,
                              last_parse: &mut std::time::Instant,
                              force: bool|
     -> (bool, bool) {
        if pending_chunk.is_empty() {
            return (false, false);
        }
        let projected_tail_len = buffer.tail_content.len() + pending_chunk.len();
        if !force
            && last_parse.elapsed().as_millis() < adaptive_parse_interval_ms(projected_tail_len)
            && pending_chunk.len() < adaptive_force_bytes(projected_tail_len)
        {
            return (false, false);
        }

        buffer.append_chunk(pending_chunk);
        pending_chunk.clear();
        *last_parse = std::time::Instant::now();
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
                    enforce_typed_assistant_budget(&stream_turn_metadata, 0, 0, content.len())?;
                    aurora_buffer.append_chunk(content);
                    let _ = aurora_buffer.process_queue();
                    aurora_buffer.pushed_len = content.len();
                    let _ = aurora_buffer.take_chunk();
                    let _ = aurora_buffer.take_tail_frame();
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
                        let owner_id = ctx
                            .get("groupId")
                            .and_then(|v| v.as_str())
                            .or_else(|| ctx.get("agentId").and_then(|v| v.as_str()));
                        if let Some(oid) = owner_id {
                            sse_context["ownerId"] = json!(oid);
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
                            send_stream_event(StreamEvent::error(
                                message_id_inner.clone(),
                                context_inner.clone(),
                                format!("启动本地代理失败: {}", e),
                            ));
                            return Err(e);
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
                            send_aurora_update(&mut aurora_buffer, true, true, Some("cancelled_by_user".to_string()), Some("请求已中止".to_string()));
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
                                    send_stream_event(StreamEvent::error(
                                        message_id_inner.clone(),
                                        context_inner.clone(),
                                        format!("VCP服务器错误: {} - {}", status, text),
                                    ));
                                    return Err(format!("VCP Error: {}", status));
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
                                send_stop_to_helper(_app, &transport_request_id, helper_generation)
                                    .await?;
                            }
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
                            tokio::select! {
                                _ = cancellation_token.cancelled() => {
                                    log::warn!("[VCPClient] Request aborted during streaming: {}", message_id_inner);
                                    send_stop_to_helper(_app, &transport_request_id, helper_generation)
                                        .await?;
                                    flush_aurora_parse(&mut aurora_buffer, &mut pending_aurora_chunk, &mut last_aurora_parse, true);
                                    aurora_buffer.finalize();
                                    send_aurora_update(&mut aurora_buffer, true, true, Some("cancelled_by_user".to_string()), Some("请求已中止".to_string()));
                                    return Ok((json!({ "fullContent": aurora_buffer.full_text, "finishReason": Some("cancelled_by_user") }), true));
                                }
                                next_line = reader.next() => {
                                    match next_line {
                                        Some(Ok(line)) => {
                                            if let Ok(event) = serde_json::from_slice::<Value>(&line) {
                                                if let Some(generation) = event.get("generation").and_then(Value::as_u64) {
                                                    if let Some(current) = helper_generation {
                                                        if current != generation {
                                                            return Err(format!(
                                                                "Helper generation changed during stream: expected {}, got {}",
                                                                current, generation
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
                                                            if let Err(limit_error) = enforce_typed_assistant_budget(
                                                                &stream_turn_metadata,
                                                                aurora_buffer.full_text.len(),
                                                                pending_aurora_chunk.len(),
                                                                delta.len(),
                                                            ) {
                                                                if let Err(stop_error) = send_stop_to_helper(
                                                                    _app,
                                                                    &transport_request_id,
                                                                    helper_generation,
                                                                )
                                                                .await
                                                                {
                                                                    return Err(format!(
                                                                        "{limit_error}; helper cleanup failed: {stop_error}"
                                                                    ));
                                                                }
                                                                return Err(limit_error);
                                                            }
                                                            pending_aurora_chunk.push_str(delta);
                                                            let (stable_changed, tail_changed) = flush_aurora_parse(
                                                                &mut aurora_buffer,
                                                                &mut pending_aurora_chunk,
                                                                &mut last_aurora_parse,
                                                                false,
                                                            );
                                                            let has_mutations = !aurora_buffer.pending_mutations.is_empty();
                                                            if stable_changed || tail_changed || has_mutations {
                                                                send_aurora_update(&mut aurora_buffer, stable_changed, tail_changed, None, None);
                                                            }
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
                                                        let generation = query_helper_generation(
                                                            _app,
                                                            &transport_request_id,
                                                        )
                                                        .await
                                                        .map_err(|query_error| {
                                                            format!(
                                                                "Cannot adopt existing helper session: {query_error}"
                                                            )
                                                        })?;
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
                                                    send_stream_event(StreamEvent::error(
                                                        message_id_inner.clone(),
                                                        context_inner.clone(),
                                                        err_msg.clone(),
                                                    ));
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
                                                    return Err(err_msg);
                                                }
                                            }
                                        }
                                        Some(Err(e)) => {
                                            log::warn!("[VCPClient] TCP socket read error: {:?}, transitioning to Retrying", e);
                                            state = State::Retrying;
                                            break;
                                        }
                                        None => {
                                            log::warn!("[VCPClient] TCP socket closed by server. Transitioning to Retrying.");
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
                            tokio::select! {
                                _ = cancellation_token.cancelled() => {
                                    log::warn!("[VCPClient] Request aborted during streaming: {}", message_id_inner);
                                    flush_aurora_parse(&mut aurora_buffer, &mut pending_aurora_chunk, &mut last_aurora_parse, true);
                                    aurora_buffer.finalize();
                                    send_aurora_update(&mut aurora_buffer, true, true, Some("cancelled_by_user".to_string()), Some("请求已中止".to_string()));
                                    return Ok((json!({ "fullContent": aurora_buffer.full_text, "finishReason": Some("cancelled_by_user") }), true));
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
                                                        enforce_typed_assistant_budget(
                                                            &stream_turn_metadata,
                                                            aurora_buffer.full_text.len(),
                                                            pending_aurora_chunk.len(),
                                                            delta.len(),
                                                        )?;
                                                        pending_aurora_chunk.push_str(delta);
                                                        let (stable_changed, tail_changed) = flush_aurora_parse(
                                                            &mut aurora_buffer,
                                                            &mut pending_aurora_chunk,
                                                            &mut last_aurora_parse,
                                                            false,
                                                        );
                                                        let has_mutations = !aurora_buffer.pending_mutations.is_empty();
                                                        if stable_changed || tail_changed || has_mutations {
                                                            send_aurora_update(&mut aurora_buffer, stable_changed, tail_changed, None, None);
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                        Some(Err(e)) => {
                                            log::warn!("[VCPClient] Stream read error: {:?}, transitioning to Retrying", e);
                                            state = State::Retrying;
                                            break;
                                        }
                                        None => {
                                            if !aurora_buffer.full_text.is_empty() || last_finish_reason.is_some() {
                                                stream_ended_normally = true;
                                            } else {
                                                log::warn!("[VCPClient] Stream ended unexpectedly (None), transitioning to Retrying");
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
                    send_aurora_update(
                        &mut aurora_buffer,
                        true,
                        true,
                        last_finish_reason.clone(),
                        None,
                    );
                    #[cfg(target_os = "android")]
                    {
                        send_stop_to_helper(_app, &transport_request_id, helper_generation).await?;
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
                send_stream_event(StreamEvent::error(
                    message_id_inner.clone(),
                    context_inner.clone(),
                    error.clone(),
                ));
                return Err(error);
            }
            State::Retrying => {
                const MAX_RETRIES: u32 = 3;
                if retry_count >= MAX_RETRIES {
                    log::error!(
                        "[VCPClient] Max retries reached ({}) for message: {}",
                        MAX_RETRIES,
                        message_id_inner
                    );
                    send_stream_event(StreamEvent::error(
                        message_id_inner.clone(),
                        context_inner.clone(),
                        "网络连接意外断开，重连失败".to_string(),
                    ));
                    return Err("Max retries reached".to_string());
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
                            send_stop_to_helper(_app, &transport_request_id, helper_generation).await?;
                        }
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
    stream_turn_metadata: StreamTurnMetadata,
    local_cli_projection: LocalCliAuroraProjection,
    stream_channel: Option<Channel<StreamEvent>>,
) -> Result<(Value, bool), String> {
    let is_typed_local_step = stream_turn_metadata.turn_attempt.is_some();
    let send_stream_event = |event: StreamEvent| {
        if let Some(ref ch) = stream_channel {
            if let Some(event) = project_model_step_event(event, &stream_turn_metadata) {
                let _ = ch.send(event);
            }
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
            send_stream_event(StreamEvent::error(
                message_id.clone(),
                context.clone(),
                "请求已中止".to_string(),
            ));
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
                    send_stream_event(StreamEvent::error(
                        message_id.clone(),
                        context.clone(),
                        err_msg.clone(),
                    ));
                    return Err(err_msg);
                }
            }
        }
    };

    if !response.status().is_success() {
        let status = response.status();
        let text = if is_typed_local_step {
            let bytes = read_response_body_bounded(
                response,
                crate::vcp_modules::cli::turn_types::MAX_ASSISTANT_STEP_BYTES
                    + TYPED_NON_STREAM_BODY_OVERHEAD_BYTES,
            )
            .await?;
            String::from_utf8_lossy(&bytes).into_owned()
        } else {
            response.text().await.unwrap_or_default()
        };
        let err_msg = format!("VCP服务器错误: {} - {}", status, text);
        send_stream_event(StreamEvent::error(
            message_id.clone(),
            context.clone(),
            err_msg.clone(),
        ));
        return Err(err_msg);
    }

    let vcp_response_result = if is_typed_local_step {
        let bytes = read_response_body_bounded(
            response,
            crate::vcp_modules::cli::turn_types::MAX_ASSISTANT_STEP_BYTES
                + TYPED_NON_STREAM_BODY_OVERHEAD_BYTES,
        )
        .await;
        bytes.and_then(|bytes| {
            serde_json::from_slice::<Value>(&bytes)
                .map_err(|error| format!("JSON解析失败: {error}"))
        })
    } else {
        response
            .json::<Value>()
            .await
            .map_err(|error| format!("JSON解析失败: {error}"))
    };
    let vcp_response = match vcp_response_result {
        Ok(value) => value,
        Err(err_msg) => {
            send_stream_event(StreamEvent::error(
                message_id.clone(),
                context.clone(),
                err_msg.clone(),
            ));
            return Err(err_msg);
        }
    };

    // 从标准的 OpenAI 格式中提取文本和结束原因
    let choices = vcp_response["choices"].as_array();
    let first_choice = choices.and_then(|c| c.first());
    let full_content = first_choice
        .and_then(|choice| choice["message"]["content"].as_str())
        .unwrap_or("")
        .to_string();
    enforce_typed_assistant_budget(&stream_turn_metadata, 0, 0, full_content.len())?;
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
    let mut update = AuroraUpdate {
        stable_blocks: if local_cli_projection.prefix.is_empty() {
            None
        } else {
            Some(StreamBlockParser::new().finalize(&full_content))
        },
        stable_changed: !local_cli_projection.prefix.is_empty(),
        tail_block: None,
        tail: None,
        tail_changed: false,
        tail_frame: None,
        tail_snapshot: None,
        content: Some(full_content.clone()),
        chunk: None,
    };
    local_cli_projection.apply(&mut update);
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

async fn load_app_settings<R: Runtime>(app: &AppHandle<R>) -> Result<Settings, String> {
    let settings_state = app.state::<SettingsState>();
    read_settings(app.clone(), settings_state).await
}

/// 中止请求 Command: interruptRequest
/// 通过 messageId 立即取消对应 outer request 的共享取消域
#[tauri::command]
#[allow(non_snake_case)]
pub fn interruptRequest(
    state: tauri::State<'_, ActiveRequests>,
    message_id: String,
) -> Result<Value, String> {
    log::info!(
        "[VCPClient] interruptRequest called for messageId: {}. Active requests: {}",
        message_id,
        state.0.len()
    );
    if state.cancel(&message_id)? {
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

/// 测试 VCP 后端连接状态并获取模型列表 (对齐桌面端 main.js fetchAndCacheModels 逻辑)
#[tauri::command]
pub async fn test_vcp_connection(vcp_url: String, vcp_api_key: String) -> Result<Value, String> {
    log::info!(
        "[VCPClient] test_vcp_connection called for URL: {}",
        vcp_url
    );

    // 对齐桌面端原汁原味的逻辑：
    // const urlObject = new URL(vcpServerUrl);
    // const baseUrl = `${urlObject.protocol}//${urlObject.host}`;
    // const modelsUrl = new URL('/v1/models', baseUrl).toString();

    let url_object = match Url::parse(&vcp_url) {
        Ok(url) => url,
        Err(e) => return Err(format!("URL 解析失败: {}", e)),
    };

    // 对齐 JS 的 urlObject.host (包含端口号)
    let port_str = match url_object.port() {
        Some(p) => format!(":{}", p),
        None => "".to_string(),
    };
    let host_with_port = format!("{}{}", url_object.host_str().unwrap_or(""), port_str);
    let base_url = format!("{}://{}", url_object.scheme(), host_with_port);

    let models_url = if base_url.ends_with('/') {
        format!("{}v1/models", base_url)
    } else {
        format!("{}/v1/models", base_url)
    };

    log::info!(
        "[VCPClient] Testing connection to (Original Logic): {}",
        models_url
    );

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
            "models": json_res
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
         LEFT JOIN messages m ON m.topic_id = ag.topic_id AND m.msg_id = ag.msg_id \
         LEFT JOIN agents a ON a.agent_id = m.agent_id \
         ORDER BY ag.created_at ASC",
    )
    .fetch_all(&db.pool)
    .await
    .map_err(|e| e.to_string())?;

    let mut list = Vec::new();
    for row in rows {
        use sqlx::Row;
        let msg_id: String = row.get("msg_id");
        // 过滤掉当前正在活跃运行的后台流式任务，它们由 sse helper 代理，并不是“被异常打断”的
        if active_requests.0.contains_key(&msg_id) {
            continue;
        }
        list.push(ActiveGeneration {
            msg_id,
            topic_id: row.get("topic_id"),
            owner_id: row.get("owner_id"),
            owner_type: row.get("owner_type"),
            created_at: row.get("created_at"),
            agent_id: row.get("agent_id"),
            agent_name: row.get("agent_name"),
        });
    }
    Ok(list)
}

async fn mark_message_as_error<R: Runtime>(
    app_handle: &AppHandle<R>,
    pool: &sqlx::Pool<sqlx::Sqlite>,
    msg_id: &str,
    custom_error: Option<String>,
) -> Result<(), String> {
    use sqlx::Row;

    // 先获取已有的正文内容进行挽留保留
    let existing_content_row = sqlx::query("SELECT content FROM messages WHERE msg_id = ?")
        .bind(msg_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| e.to_string())?;
    let existing_content = existing_content_row
        .and_then(|r| r.get::<Option<String>, _>("content"))
        .unwrap_or_default();

    let row = sqlx::query(
        "SELECT topic_id, owner_id, owner_type FROM active_generations WHERE msg_id = ?",
    )
    .bind(msg_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| e.to_string())?;

    if let Some(r) = row {
        let topic_id: String = r.get("topic_id");
        let owner_id: String = r.get("owner_id");
        let owner_type: String = r.get("owner_type");

        let agent_id_row = sqlx::query("SELECT agent_id FROM messages WHERE msg_id = ?")
            .bind(msg_id)
            .fetch_optional(pool)
            .await
            .map_err(|e| e.to_string())?;
        let agent_id = agent_id_row.and_then(|r| r.get::<Option<String>, _>("agent_id"));

        let error_suffix = match custom_error {
            Some(err) => format!("\n\n> VCP流式错误: {}", err),
            None => "\n\n> VCP流式错误: 生成意外中断".to_string(),
        };
        let final_content = if existing_content.is_empty() {
            error_suffix
        } else {
            format!("{}{}", existing_content, error_suffix)
        };

        crate::vcp_modules::chat::message_service::finalize_stream_message(
            app_handle.clone(),
            pool,
            &owner_id,
            &owner_type,
            topic_id,
            msg_id.to_string(),
            final_content,
            false,
            Some("error".to_string()),
            None,
            agent_id,
        )
        .await?;
    } else {
        // Another owner may have committed the terminal message after recovery began.
        // Terminal rows are immutable: a late recovery must be an idempotent no-op.
        log::info!(
            "[VCPClient] Generation {} is no longer pending; skipping late error finalization.",
            msg_id
        );
    }
    Ok(())
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
pub async fn recover_active_generation<R: Runtime>(
    app: AppHandle<R>,
    active_requests: tauri::State<'_, ActiveRequests>,
    msg_id: String,
    stream_channel: Channel<StreamEvent>,
    is_warm: bool,
) -> Result<Value, String> {
    log::info!(
        "[VCPClient] recover_active_generation called for msg_id: {}",
        msg_id
    );

    // Recovery owns one atomic attempt from inspection through terminal commit.
    // This removes the old recover -> resume two-IPC claim gap.
    let (_recovery_lease, recovery_cancellation_token) =
        match ActiveRequestLease::try_acquire(active_requests.0.clone(), msg_id.clone()) {
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
    let pending: Option<(String, String, String, Option<String>)> = sqlx::query_as(
        "SELECT ag.topic_id, ag.owner_id, ag.owner_type, m.agent_id \
         FROM active_generations ag \
         LEFT JOIN messages m ON m.msg_id = ag.msg_id AND m.topic_id = ag.topic_id \
         WHERE ag.msg_id = ?",
    )
    .bind(&msg_id)
    .fetch_optional(&db.pool)
    .await
    .map_err(|e| e.to_string())?;

    let Some((topic_id, owner_id, owner_type, agent_id)) = pending else {
        let terminal: Option<(String, Option<String>)> =
            sqlx::query_as("SELECT content, finish_reason FROM messages WHERE msg_id = ?")
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

    // A durable local-loop turn owns recovery before any legacy disk/SSE-helper probe.
    // The outer lease remains live across this await; `None` alone authorizes the old path.
    if let Some(recovered) = crate::vcp_modules::cli::turn_coordinator::recover_local_cli_turn(
        &app,
        &db.pool,
        &msg_id,
        stream_channel.clone(),
        recovery_cancellation_token.clone(),
    )
    .await?
    {
        return Ok(recovered);
    }

    #[cfg(not(target_os = "android"))]
    let _ = is_warm;

    let cache_dir = app.path().app_cache_dir().map_err(|e| e.to_string())?;

    // 异步清理超过 24 小时的孤立缓存文件
    let cache_dir_clone = cache_dir.clone();
    tokio::spawn(async move {
        clean_old_cache_files(&cache_dir_clone);
    });

    // 2. 检查是否存在 5 分钟超时后由助手转存的本地 JSON 恢复文件 (24小时内认领有效)
    let safe_msg_id = crate::vcp_modules::infra::utils::calculate_sha256(msg_id.as_bytes());
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
                        &owner_id,
                        &owner_type,
                        topic_id.clone(),
                        msg_id.clone(),
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

            let stream = send_command_to_stream(stream, "query", &msg_id, None).await?;

            log::info!("[VCPClient] Query command sent, waiting for response frame...");
            let mut reader = FramedRead::new(stream, helper_frame_codec());
            let line = tokio::time::timeout(HELPER_IO_TIMEOUT, reader.next())
                .await
                .map_err(|_| "Helper query response timed out".to_string())?
                .ok_or_else(|| "No query response received (EOF)".to_string())?
                .map_err(|e| format!("Helper query frame error: {}", e))?;
            let resp = serde_json::from_slice::<Value>(&line).map_err(|e| e.to_string())?;
            if resp.get("requestId").and_then(Value::as_str) != Some(msg_id.as_str()) {
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
                        &owner_id,
                        &owner_type,
                        topic_id.clone(),
                        msg_id.clone(),
                        content.clone(),
                        false,
                        last_finish_reason.or(Some("completed".to_string())),
                        Some(stream_channel.clone()),
                        agent_id.clone(),
                    )
                    .await?;

                    log::info!("[VCPClient] Finalization complete. Sending stop command to helper to release memory.");
                    if let Err(stop_error) =
                        send_stop_to_helper(&app, &msg_id, Some(helper_generation)).await
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
                    let initial_content = is_warm.then_some(content);
                    let last_event_index = if is_warm {
                        resp["lastEventIndex"].as_i64()
                    } else {
                        None
                    };
                    let resumed = resume_claimed_generation(
                        &app,
                        msg_id.clone(),
                        topic_id.clone(),
                        owner_id.clone(),
                        owner_type.clone(),
                        agent_id.clone(),
                        stream_channel.clone(),
                        initial_content,
                        last_event_index,
                        recovery_cancellation_token,
                    )
                    .await?;
                    return Ok(json!({
                        "status": "completed",
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

    mark_message_as_error(
        &app,
        &db.pool,
        &msg_id,
        Some("后台进程已被系统销毁，流式对话中断".to_string()),
    )
    .await?;

    let content: Option<String> =
        sqlx::query_scalar("SELECT content FROM messages WHERE msg_id = ?")
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
    msg_id: String,
    topic_id: String,
    owner_id: String,
    owner_type: String,
    agent_id: Option<String>,
    stream_channel: Channel<StreamEvent>,
    initial_content: Option<String>,
    last_event_index: Option<i64>,
    cancellation_token: CancellationToken,
) -> Result<Value, String> {
    log::info!(
        "[VCPClient] resume_claimed_generation called for messageId: {}, topicId: {}, lastEventIndex: {:?}",
        msg_id,
        topic_id,
        last_event_index
    );

    let client = Client::builder().build().map_err(|e| e.to_string())?;

    let pool = app.state::<DbState>().pool.clone();

    if let Some(ref content) = initial_content {
        let _ = sqlx::query(
            "UPDATE messages SET content = ?, updated_at = ? \
             WHERE msg_id = ? AND topic_id = ? AND finish_reason IS NULL \
             AND EXISTS(SELECT 1 FROM active_generations WHERE msg_id = ?)",
        )
        .bind(content)
        .bind(crate::vcp_modules::infra::utils::now_millis())
        .bind(&msg_id)
        .bind(&topic_id)
        .bind(&msg_id)
        .execute(&pool)
        .await;
    }

    let context = json!({
        "topicId": topic_id,
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
        msg_id.clone(),
        Some(context.clone()),
        cancellation_token,
        StreamTurnMetadata::default(),
        LocalCliAuroraProjection::default(),
        Some(stream_channel.clone()),
        true,
        last_event_index,
        initial_content.clone(),
    )
    .await
    {
        Ok(val) => val,
        Err(e) => {
            log::error!(
                "[VCPClient] Claimed recovery failed during handle_streaming_request: {}",
                e
            );
            let _ =
                mark_message_as_error(app, &pool, &msg_id, Some(format!("接续失败: {}", e))).await;
            return Err(e);
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
        &owner_id,
        &owner_type,
        topic_id,
        msg_id.clone(),
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
        let (lease, first_step) =
            ActiveRequestLease::try_acquire(requests.0.clone(), "message-1".to_string())
                .expect("first attempt should register");
        let second_step = first_step.clone();

        assert!(
            ActiveRequestLease::try_acquire(requests.0.clone(), "message-1".to_string()).is_err()
        );
        assert!(requests.cancel("message-1").expect("cancel should succeed"));
        assert!(requests.0.contains_key("message-1"));
        first_step.cancelled().await;
        second_step.cancelled().await;
        assert!(first_step.is_cancelled());
        assert!(requests
            .cancel("message-1")
            .expect("repeated cancel remains idempotent"));

        drop(lease);
        assert!(!requests.0.contains_key("message-1"));
    }

    #[test]
    fn stale_lease_cannot_remove_a_new_attempt() {
        let requests = ActiveRequests::default();
        let (old_lease, _old_token) =
            ActiveRequestLease::try_acquire(requests.0.clone(), "message-2".to_string())
                .expect("old attempt should register");

        let new_attempt_id = uuid::Uuid::new_v4();
        requests.0.insert(
            "message-2".to_string(),
            Arc::new(ActiveRequestEntry {
                attempt_id: new_attempt_id,
                cancellation_token: CancellationToken::new(),
            }),
        );

        drop(old_lease);
        assert_eq!(
            requests
                .0
                .get("message-2")
                .expect("new attempt must remain")
                .attempt_id,
            new_attempt_id
        );
    }

    #[test]
    fn transport_identity_and_turn_event_wire_keep_visible_message_id_stable() {
        let payload: VcpRequestPayload = serde_json::from_value(json!({
            "vcpUrl": "https://example.invalid/v1/chat/completions",
            "vcpApiKey": "secret",
            "messages": [],
            "modelConfig": {"stream": true},
            "messageId": "outer-message",
            "context": null,
            "transportRequestId": "outer-message:step:2:attempt-a",
            "turnAttempt": "attempt-a",
            "stepIndex": 2,
            "projectionReset": true,
            "mobileCliAgentRoute": "localLoopback"
        }))
        .expect("typed request payload");

        assert_eq!(
            payload.effective_transport_request_id(),
            "outer-message:step:2:attempt-a"
        );
        let event = StreamEvent::thinking(payload.message_id.clone(), None)
            .with_turn_metadata(&payload.stream_turn_metadata());
        let wire = serde_json::to_value(event).expect("serialize stream event");
        assert_eq!(wire["messageId"], "outer-message");
        assert_eq!(wire["turnAttempt"], "attempt-a");
        assert_eq!(wire["stepIndex"], 2);
        assert_eq!(wire["projectionReset"], true);
        assert!(wire.get("transportRequestId").is_none());
        assert_eq!(
            payload.mobile_cli_agent_route,
            Some(MobileCliAgentRoute::LocalLoopback)
        );
        assert_eq!(
            project_aurora_finish_reason(
                &payload.stream_turn_metadata(),
                Some("completed".to_string())
            ),
            None
        );
        assert!(payload.local_cli_projection_prefix.is_none());
    }

    #[test]
    fn durable_prefix_is_internal_and_survives_every_stable_block_replacement() {
        let payload: VcpRequestPayload = serde_json::from_value(json!({
            "vcpUrl": "https://example.invalid/v1/chat/completions",
            "vcpApiKey": "secret",
            "messages": [],
            "modelConfig": {"stream": true},
            "messageId": "outer-message",
            "context": null,
            "localCliProjectionPrefix": "untrusted wire prefix"
        }))
        .expect("request payload");
        assert!(payload.local_cli_projection_prefix.is_none());

        let prefix = "[[VCP调用结果信息汇总:\n- 工具名称: VCPMobileCLI\n- 执行状态: success\nVCP调用结果结束]]\n\n";
        let projection = LocalCliAuroraProjection::from_prefix(Some(prefix.to_string()));

        for model_content in ["first model step", "next model step"] {
            let mut update = AuroraUpdate {
                stable_blocks: Some(StreamBlockParser::new().finalize(model_content)),
                stable_changed: true,
                tail_block: None,
                tail: None,
                tail_changed: false,
                tail_frame: None,
                tail_snapshot: None,
                content: Some(model_content.to_string()),
                chunk: None,
            };
            projection.apply(&mut update);

            assert_eq!(
                update.content.as_deref(),
                Some(format!("{prefix}{model_content}").as_str())
            );
            let blocks = serde_json::to_value(
                update
                    .stable_blocks
                    .as_ref()
                    .expect("blocks-first projection must be present"),
            )
            .expect("serialize projected blocks");
            assert_eq!(blocks[0]["type"], "tool-result");
            assert_eq!(blocks[1]["type"], "markdown");
        }
    }

    #[test]
    fn typed_assistant_and_raw_response_budgets_accept_exact_limit_only() {
        let typed = StreamTurnMetadata {
            turn_attempt: Some("attempt-budget".to_string()),
            step_index: Some(1),
            projection_reset: Some(false),
        };
        let max = crate::vcp_modules::cli::turn_types::MAX_ASSISTANT_STEP_BYTES;
        assert!(enforce_typed_assistant_budget(&typed, max - 1, 0, 1).is_ok());
        assert!(enforce_typed_assistant_budget(&typed, max, 0, 1).is_err());
        assert!(enforce_typed_assistant_budget(
            &StreamTurnMetadata::default(),
            usize::MAX,
            usize::MAX,
            usize::MAX,
        )
        .is_ok());

        let mut body = Vec::new();
        assert!(append_bounded_response_chunk(&mut body, b"1234", 4).is_ok());
        assert!(append_bounded_response_chunk(&mut body, b"5", 4).is_err());
        assert_eq!(body, b"1234");
        assert!(is_typed_assistant_budget_error(
            TYPED_ASSISTANT_BUDGET_ERROR
        ));
        assert!(is_typed_assistant_budget_error(&format!(
            "{TYPED_ASSISTANT_BUDGET_ERROR}; helper cleanup failed: timeout"
        )));
        assert!(!is_typed_assistant_budget_error("network unavailable"));
    }

    #[test]
    fn nonterminal_step_events_use_metadata_and_inner_error_is_suppressed() {
        let metadata = StreamTurnMetadata {
            turn_attempt: Some("attempt-b".to_string()),
            step_index: Some(4),
            projection_reset: Some(false),
        };
        let events = [
            StreamEvent::thinking("visible".to_string(), None),
            StreamEvent::aurora(
                "visible".to_string(),
                AuroraUpdate {
                    stable_blocks: None,
                    stable_changed: false,
                    tail_block: None,
                    tail: None,
                    tail_changed: false,
                    tail_frame: None,
                    tail_snapshot: None,
                    content: None,
                    chunk: None,
                },
                None,
            ),
            StreamEvent {
                r#type: "reconnecting".to_string(),
                message_id: "visible".to_string(),
                ..Default::default()
            },
        ];

        for event in events {
            let projected = project_model_step_event(event, &metadata)
                .expect("nonterminal step event remains visible");
            let wire = serde_json::to_value(projected).expect("serialize projected event");
            assert_eq!(wire["messageId"], "visible");
            assert_eq!(wire["turnAttempt"], "attempt-b");
            assert_eq!(wire["stepIndex"], 4);
            assert_eq!(wire["projectionReset"], false);
        }

        assert!(project_model_step_event(
            StreamEvent::error("visible".to_string(), None, "failed".to_string()),
            &metadata,
        )
        .is_none());
        assert!(project_model_step_event(
            StreamEvent::error("legacy".to_string(), None, "failed".to_string()),
            &StreamTurnMetadata::default(),
        )
        .is_some());

        let terminal = StreamEvent::end(
            "visible".to_string(),
            None,
            Some("completed".to_string()),
            None,
            None,
        )
        .with_turn_projection("attempt-b".to_string(), 4, false);
        let wire = serde_json::to_value(terminal).expect("serialize terminal projection");
        assert_eq!(wire["messageId"], "visible");
        assert_eq!(wire["turnAttempt"], "attempt-b");
        assert_eq!(wire["stepIndex"], 4);
        assert_eq!(wire["projectionReset"], false);
        assert_eq!(wire["finishReason"], "completed");
    }

    #[test]
    fn legacy_request_falls_back_to_visible_transport_and_omits_turn_wire() {
        let payload: VcpRequestPayload = serde_json::from_value(json!({
            "vcpUrl": "https://example.invalid/v1/chat/completions",
            "vcpApiKey": "secret",
            "messages": [],
            "modelConfig": {"stream": true},
            "messageId": "legacy-message",
            "context": null
        }))
        .expect("legacy request payload");

        assert_eq!(payload.effective_transport_request_id(), "legacy-message");
        let wire = serde_json::to_value(
            StreamEvent::thinking(payload.message_id.clone(), None)
                .with_turn_metadata(&payload.stream_turn_metadata()),
        )
        .expect("serialize legacy stream event");
        assert!(wire.get("turnAttempt").is_none());
        assert!(wire.get("stepIndex").is_none());
        assert!(wire.get("projectionReset").is_none());
        assert_eq!(
            project_aurora_finish_reason(
                &payload.stream_turn_metadata(),
                Some("completed".to_string())
            ),
            Some("completed".to_string())
        );
    }

    #[test]
    fn frozen_route_overrides_legacy_endpoint_flag_for_every_step() {
        let raw = "https://example.invalid/proxy/v1/chat/completions";
        let settings = crate::vcp_modules::settings_manager::create_default_settings();
        assert_eq!(
            resolve_vcp_endpoint(raw, MobileCliAgentRoute::LocalLoopback),
            "https://example.invalid/proxy/v1/chat/completions"
        );
        let frozen_remote =
            resolve_request_route(Some(MobileCliAgentRoute::VcpPlugin), Some(&settings));
        assert_eq!(
            resolve_vcp_endpoint(raw, frozen_remote),
            "https://example.invalid/v1/chatvcp/completions"
        );
    }

    #[test]
    fn local_loopback_transport_guard_is_first_system_only_and_idempotent() {
        let mut messages = vec![
            json!({"role":"system", "content":"user-owned prompt"}),
            json!({"role":"user", "content":"hello"}),
        ];
        inject_local_loopback_transport_guard(&mut messages);
        inject_local_loopback_transport_guard(&mut messages);
        assert_eq!(messages[0]["role"], "system");
        let content = messages[0]["content"].as_str().expect("system text");
        assert!(content.starts_with(VCP_TOOL_USE_FORBIDDEN_SENTINEL));
        assert_eq!(content.matches(VCP_TOOL_USE_FORBIDDEN_SENTINEL).count(), 1);
        assert!(content.ends_with("user-owned prompt"));

        let mut user_first = vec![json!({"role":"user", "content":"hello"})];
        inject_local_loopback_transport_guard(&mut user_first);
        assert_eq!(user_first[0]["role"], "system");
        assert_eq!(user_first[0]["content"], VCP_TOOL_USE_FORBIDDEN_SENTINEL);
        assert_eq!(user_first[1]["role"], "user");
    }

    #[test]
    fn local_loopback_transport_guard_preserves_multimodal_system_content() {
        let mut messages = vec![json!({
            "role":"system",
            "content":[{"type":"text", "text":"prompt"}]
        })];
        inject_local_loopback_transport_guard(&mut messages);
        let parts = messages[0]["content"].as_array().expect("system parts");
        assert_eq!(parts[0]["type"], "text");
        assert_eq!(parts[0]["text"], VCP_TOOL_USE_FORBIDDEN_SENTINEL);
        assert_eq!(parts[1]["text"], "prompt");
    }

    #[test]
    fn transport_guard_is_scoped_to_typed_local_loopback_steps() {
        let typed = StreamTurnMetadata {
            turn_attempt: Some("attempt-1".to_string()),
            ..StreamTurnMetadata::default()
        };
        assert!(should_inject_local_loopback_transport_guard(
            MobileCliAgentRoute::LocalLoopback,
            &typed
        ));
        assert!(!should_inject_local_loopback_transport_guard(
            MobileCliAgentRoute::VcpPlugin,
            &typed
        ));
        assert!(!should_inject_local_loopback_transport_guard(
            MobileCliAgentRoute::LocalLoopback,
            &StreamTurnMetadata::default()
        ));
    }

    #[test]
    fn legacy_extra_flag_never_selects_vcp_plugin_endpoint() {
        let mut settings = crate::vcp_modules::settings_manager::create_default_settings();
        settings.extra = json!({ "enableVcpToolInjection": true });
        let raw = "https://example.invalid/proxy/v1/chat/completions";
        let route = resolve_request_route(None, Some(&settings));

        assert_eq!(route, MobileCliAgentRoute::LocalLoopback);
        assert_eq!(
            resolve_vcp_endpoint(raw, route),
            "https://example.invalid/proxy/v1/chat/completions"
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

    #[test]
    fn helper_adoption_is_generation_fenced_and_alignment_never_returns_partial_success() {
        let source = include_str!("vcp_client.rs");
        let adoption_start = source
            .find("== HelperStartErrorDisposition::AdoptExistingSession")
            .expect("helper adoption branch");
        let adoption_end = source[adoption_start..]
            .find("Stream proxy error")
            .map(|offset| adoption_start + offset)
            .expect("helper adoption boundary");
        let adoption = &source[adoption_start..adoption_end];
        assert!(adoption.contains("query_helper_generation"));
        assert!(adoption.contains("helper_generation = Some(generation)"));
        assert!(adoption.contains("state = State::Resuming"));

        let aligning_start = source
            .find("State::Aligning =>")
            .expect("alignment failure branch");
        let aligning_end = source[aligning_start..]
            .find("State::Retrying =>")
            .map(|offset| aligning_start + offset)
            .expect("alignment failure boundary");
        let aligning = &source[aligning_start..aligning_end];
        assert!(aligning.contains("return Err(error);"));
        assert!(!aligning.contains("break 'main_loop"));
    }

    #[test]
    fn local_turn_recovery_claim_precedes_every_legacy_cache_or_helper_branch() {
        let source = include_str!("vcp_client.rs");
        let function_start = source
            .find("pub async fn recover_active_generation")
            .expect("recovery entrypoint");
        let function_end = source[function_start..]
            .find("async fn resume_claimed_generation")
            .map(|offset| function_start + offset)
            .expect("recovery function boundary");
        let recovery = &source[function_start..function_end];

        let local_probe = recovery
            .find("turn_coordinator::recover_local_cli_turn")
            .expect("local ledger probe");
        let legacy_cache = recovery
            .find("app.path().app_cache_dir()")
            .expect("legacy cache branch");
        assert!(local_probe < legacy_cache);
        assert!(recovery.contains("recovery_cancellation_token.clone()"));
        assert!(recovery.contains("return Ok(recovered);"));
    }
}

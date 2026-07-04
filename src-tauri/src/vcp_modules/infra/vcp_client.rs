use crate::vcp_modules::infra::utils::normalize_vcp_url;
use crate::vcp_modules::media_processor::convert_local_image_for_multimodal;
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
use tokio::sync::oneshot;
#[cfg(target_os = "android")]
use tokio_util::codec::LengthDelimitedCodec;
use tokio_util::codec::{FramedRead, LinesCodec};
use tokio_util::io::StreamReader;
use url::Url;

use crate::vcp_modules::aurora_pipeline::{AuroraBuffer, AuroraUpdate};
use crate::vcp_modules::content_parser::ContentBlock;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::settings_manager::{create_default_settings, Settings};

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
}

/// 流式事件结构体，用于向前端发送数据
#[derive(Debug, Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct StreamEvent {
    pub r#type: String, // 事件类型: "data", "aurora", "end", "error", "reconnecting"
    pub chunk: Option<Value>, // 数据块 (仅 type="data" 时有效)
    pub message_id: String, // 消息ID
    pub context: Option<Value>, // 透传的上下文信息
    pub finish_reason: Option<String>, // 结束原因
    pub error: Option<String>, // 错误信息 (仅 type="error" 时有效)
    pub aurora: Option<AuroraUpdate>, // Aurora 语义沉淀更新 (type="aurora" 时有效)
    pub blocks: Option<Vec<ContentBlock>>, // 持久化后的预渲染块 (仅 type="end" 时有效)
    pub timestamp: Option<u64>, // ⚡ 新增物理落笔时间戳
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

/// 全局活跃请求管理器，使用 DashMap 存储中止信号发送端
/// messageId -> oneshot::Sender
pub struct ActiveRequests(pub Arc<DashMap<String, oneshot::Sender<()>>>);

impl Default for ActiveRequests {
    fn default() -> Self {
        log::info!("[VCPClient] Initialized ActiveRequests successfully.");
        Self(Arc::new(DashMap::new()))
    }
}

/// RAII guard：在 Drop 时自动从 ActiveRequests 中移除对应条目，防止 panic 导致泄漏
pub struct ActiveRequestGuard {
    requests: Arc<DashMap<String, oneshot::Sender<()>>>,
    message_id: String,
}

impl ActiveRequestGuard {
    pub fn new(requests: Arc<DashMap<String, oneshot::Sender<()>>>, message_id: String) -> Self {
        Self {
            requests,
            message_id,
        }
    }
}

impl Drop for ActiveRequestGuard {
    fn drop(&mut self) {
        self.requests.remove(&self.message_id);
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
/// 2. 动态路由切换 (根据设置注入 /v1/chatvcp/completions)
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

    let (res, is_aborted) =
        match perform_vcp_request(&app, state.0.clone(), payload, Some(stream_channel.clone()))
            .await
        {
            Ok(val) => val,
            Err(e) => {
                if is_stream {
                    let pool = app
                        .state::<crate::vcp_modules::db_manager::DbState>()
                        .pool
                        .clone();
                    let _ = sqlx::query("DELETE FROM active_generations WHERE msg_id = ?")
                        .bind(&message_id)
                        .execute(&pool)
                        .await;
                }
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
pub async fn perform_vcp_request<R: Runtime>(
    app: &AppHandle<R>,
    active_requests: Arc<DashMap<String, oneshot::Sender<()>>>,
    payload: VcpRequestPayload,
    stream_channel: Option<Channel<StreamEvent>>,
) -> Result<(Value, bool), String> {
    log::info!(
        "[VCPClient] perform_vcp_request called for messageId: {}, context: {:?}",
        payload.message_id,
        payload.context
    );

    let message_id = payload.message_id.clone();
    let context = payload.context.clone();

    // === 1. 数据验证和多模态资产转换 ===
    let mut messages = preprocess_multimodal_messages(app, payload.messages).await?;

    // === 2. 读取设置与动态路由切换 ===
    let mut enable_vcp_tool_injection = false;

    if let Ok(settings) = load_app_settings(app).await {
        if let Some(extra) = settings.extra.as_object() {
            enable_vcp_tool_injection = extra
                .get("enableVcpToolInjection")
                .and_then(|v: &Value| v.as_bool())
                .unwrap_or(false);
        }
    }

    let mut final_url = payload.vcp_url.clone();
    if enable_vcp_tool_injection {
        if let Ok(mut url) = Url::parse(&final_url) {
            url.set_path("/v1/chatvcp/completions");
            final_url = url.to_string();
        }
    } else {
        final_url = normalize_vcp_url(&final_url);
    }

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
        obj.insert("requestId".to_string(), json!(payload.message_id));
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

    // 创建并注册中止信号
    let (abort_tx, abort_rx) = oneshot::channel();
    active_requests.insert(payload.message_id.clone(), abort_tx);
    let _guard = ActiveRequestGuard::new(active_requests.clone(), payload.message_id.clone());

    // === 7. 分发至专职处理器执行请求 ===
    if is_stream {
        handle_streaming_request(
            app,
            client,
            &final_url,
            &payload.vcp_api_key,
            request_body,
            message_id,
            context,
            abort_rx,
            active_requests,
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
            abort_rx,
            active_requests,
            stream_channel,
        )
        .await
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
        let content = msg.get("content").cloned().unwrap_or(Value::Null);

        // 处理多模态或复杂内容数组
        if let Some(content_array) = content.as_array() {
            let mut new_parts = Vec::new();
            for part in content_array {
                if let Some(obj) = part.as_object() {
                    // 识别自定义的 local_file 类型并进行路径还原与编码
                    if obj.get("type").and_then(|t| t.as_str()) == Some("local_file") {
                        if let Some(path_str) = obj.get("path").and_then(|p| p.as_str()) {
                            let clean_path = path_str.replace("file://", "");
                            let path_buf = std::path::PathBuf::from(&clean_path);

                            let mut converted = false;
                            // 提取扩展名决定 mime_type（在文件存在判断之前，确保降级提示也能按类型区分）
                            let ext = path_buf
                                .extension()
                                .and_then(|e| e.to_str())
                                .unwrap_or("")
                                .to_lowercase();
                            let (mime, part_type) = match ext.as_str() {
                                "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "heic"
                                | "heif" | "avif" => ("image", "image_url"),
                                "mp3" | "wav" | "ogg" | "flac" | "aac" | "m4a" | "opus" | "amr" => {
                                    ("audio", "input_audio")
                                }
                                "mp4" | "webm" | "3gp" | "3g2" | "mov" => ("video", "image_url"),
                                _ => ("application", "file_url"), // 非支持多模态格式退化回退
                            };

                            if path_buf.exists() {
                                if mime == "image" {
                                    // 图片类型：长边 > 1120px 时缩放，避免多模态 payload 过大
                                    let path_buf_clone = path_buf.clone();
                                    let app_clone = app.clone();
                                    match tokio::task::spawn_blocking(move || {
                                        convert_local_image_for_multimodal(
                                            &app_clone,
                                            &path_buf_clone,
                                        )
                                    })
                                    .await
                                    {
                                        Ok(Ok(data_url)) => {
                                            new_parts.push(json!({
                                                "type": part_type,
                                                part_type: { "url": data_url }
                                            }));
                                            converted = true;
                                        }
                                        Ok(Err(e)) => {
                                            log::warn!(
                                                "[VCPClient] Image conversion failed for {:?}: {}",
                                                path_buf,
                                                e
                                            );
                                        }
                                        Err(e) => {
                                            log::warn!(
                                                "[VCPClient] Image conversion task panicked: {}",
                                                e
                                            );
                                        }
                                    }
                                } else if mime == "video" {
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
                                            log::warn!("[VCPClient] Video frame extraction failed for {:?}: {}", path_buf, e);
                                        }
                                        Err(e) => {
                                            log::warn!("[VCPClient] Video processing task panicked: {}", e);
                                        }
                                    }
                                } else if mime == "audio" {
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
                                            log::warn!("[VCPClient] Audio extraction failed for {:?}: {}", path_buf, e);
                                        }
                                        Err(e) => {
                                            log::warn!("[VCPClient] Audio processing task panicked: {}", e);
                                        }
                                    }
                                }
                            }

                            // 若文件不存在或读取失败，至少保留文本描述，避免内容静默丢失
                            if !converted {
                                let mut warn_msg = format!("[附件文件: {}]", clean_path);
                                if mime == "image" {
                                    warn_msg = format!("[附件文件: {}]\n<system_meta>[系统提示]：由于硬件环境限制或原图过大，该图片的视觉信息提取失败，已转为纯文本占位符，请提醒用户注意。</system_meta>", clean_path);
                                }
                                new_parts.push(json!({
                                    "type": "text",
                                    "text": warn_msg
                                }));
                            }
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
                if let Ok(stream) =
                    tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port)).await
                {
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

    for attempt in 1..=max_attempts {
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
        match tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port)).await {
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
    Ok(stream)
}

#[cfg(target_os = "android")]
async fn send_stop_to_helper<R: Runtime>(app: &AppHandle<R>, msg_id: &str) -> Result<(), String> {
    let port = get_helper_port(app)?;
    let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
        .await
        .map_err(|e| e.to_string())?;

    let cmd = json!({
        "action": "stop",
        "requestId": msg_id
    });

    use tokio::io::AsyncWriteExt;
    let cmd_str = cmd.to_string();
    let cmd_bytes = cmd_str.as_bytes();
    let len = cmd_bytes.len() as u32;
    stream
        .write_all(&len.to_be_bytes())
        .await
        .map_err(|e| e.to_string())?;
    stream
        .write_all(cmd_bytes)
        .await
        .map_err(|e| e.to_string())?;
    stream.flush().await.map_err(|e| e.to_string())?;
    Ok(())
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
    context: Option<Value>,
    mut abort_rx: tokio::sync::oneshot::Receiver<()>,
    active_requests: Arc<DashMap<String, tokio::sync::oneshot::Sender<()>>>,
    stream_channel: Option<Channel<StreamEvent>>,
    is_resume: bool,
    last_event_index: Option<i64>,
    initial_content: Option<String>,
) -> Result<(Value, bool), String> {
    let send_stream_event = |event: StreamEvent| {
        if let Some(ref ch) = stream_channel {
            let _ = ch.send(event);
        }
    };

    let message_id_inner = message_id.clone();
    let context_inner = context.clone();
    let active_requests_inner = active_requests.clone();

    let mut full_content = String::new();
    let mut last_finish_reason: Option<String> = None;
    #[allow(unused_mut)]
    let mut last_received_index: Option<i64> = last_event_index;
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
        let mut event = StreamEvent::aurora(
            message_id_inner.clone(),
            AuroraUpdate {
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
            },
            context_inner.clone(),
        );
        event.finish_reason = finish_reason;
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
    'main_loop: loop {
        match state {
            State::Init => {
                if let Some(ref content) = initial_content {
                    aurora_buffer.append_chunk(content);
                    let _ = aurora_buffer.process_queue();
                    aurora_buffer.pushed_len = content.len();
                    let _ = aurora_buffer.take_chunk();
                    let _ = aurora_buffer.take_tail_frame();
                    full_content.push_str(content);
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

                    match connect_to_helper(_app, "start", &message_id_inner, Some(params)).await {
                        Ok(stream) => {
                            tcp_reader = Some(FramedRead::new(stream, LengthDelimitedCodec::new()));
                            state = State::Streaming;
                        }
                        Err(e) => {
                            log::error!("[VCPClient] connect_to_helper failed: {:?}", e);
                            send_stream_event(StreamEvent::error(
                                message_id_inner.clone(),
                                context_inner.clone(),
                                format!("启动本地代理失败: {}", e),
                            ));
                            active_requests_inner.remove(&message_id_inner);
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
                        _ = &mut abort_rx => {
                            log::warn!("[VCPClient] Request aborted during connection: {}", message_id_inner);
                            flush_aurora_parse(&mut aurora_buffer, &mut pending_aurora_chunk, &mut last_aurora_parse, true);
                            aurora_buffer.finalize();
                            send_aurora_update(&mut aurora_buffer, true, true, Some("cancelled_by_user".to_string()), Some("请求已中止".to_string()));
                            active_requests_inner.remove(&message_id_inner);
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
                                    active_requests_inner.remove(&message_id_inner);
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
                        _ = &mut abort_rx => {
                            #[cfg(target_os = "android")]
                            {
                                let _ = send_stop_to_helper(_app, &message_id_inner).await;
                            }
                            active_requests_inner.remove(&message_id_inner);
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

                    match connect_to_helper(_app, "resume", &message_id_inner, Some(params)).await {
                        Ok(stream) => {
                            log::info!("[VCPClient] Successfully reconnected to sse helper socket");
                            tcp_reader = Some(FramedRead::new(stream, LengthDelimitedCodec::new()));
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
                                _ = &mut abort_rx => {
                                    log::warn!("[VCPClient] Request aborted during streaming: {}", message_id_inner);
                                    let _ = send_stop_to_helper(_app, &message_id_inner).await;
                                    flush_aurora_parse(&mut aurora_buffer, &mut pending_aurora_chunk, &mut last_aurora_parse, true);
                                    aurora_buffer.finalize();
                                    send_aurora_update(&mut aurora_buffer, true, true, Some("cancelled_by_user".to_string()), Some("请求已中止".to_string()));
                                    active_requests_inner.remove(&message_id_inner);
                                    return Ok((json!({ "fullContent": aurora_buffer.full_text, "finishReason": Some("cancelled_by_user") }), true));
                                }
                                next_line = reader.next() => {
                                    match next_line {
                                        Some(Ok(line)) => {
                                            if let Ok(event) = serde_json::from_slice::<Value>(&line) {
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
                                                    log::warn!("[VCPClient] Stream proxy error: {}. Failing stream immediately.", err_msg);
                                                    send_stream_event(StreamEvent::error(
                                                        message_id_inner.clone(),
                                                        context_inner.clone(),
                                                        err_msg.clone(),
                                                    ));
                                                    let _ = send_stop_to_helper(_app, &message_id_inner).await;
                                                    active_requests_inner.remove(&message_id_inner);
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
                                _ = &mut abort_rx => {
                                    log::warn!("[VCPClient] Request aborted during streaming: {}", message_id_inner);
                                    flush_aurora_parse(&mut aurora_buffer, &mut pending_aurora_chunk, &mut last_aurora_parse, true);
                                    aurora_buffer.finalize();
                                    send_aurora_update(&mut aurora_buffer, true, true, Some("cancelled_by_user".to_string()), Some("请求已中止".to_string()));
                                    active_requests_inner.remove(&message_id_inner);
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
                        let _ = send_stop_to_helper(_app, &message_id_inner).await;
                    }
                    active_requests_inner.remove(&message_id_inner);
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
                send_stream_event(StreamEvent::error(
                    message_id_inner.clone(),
                    context_inner.clone(),
                    "流连接意外断开且本地缓存不可用".to_string(),
                ));
                break 'main_loop;
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
                    active_requests_inner.remove(&message_id_inner);
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
                    _ = &mut abort_rx => {
                        log::warn!("[VCPClient] Aborted during retry backoff sleep");
                        #[cfg(target_os = "android")]
                        {
                            let _ = send_stop_to_helper(_app, &message_id_inner).await;
                        }
                        active_requests_inner.remove(&message_id_inner);
                        return Ok((json!({ "fullContent": aurora_buffer.full_text, "finishReason": Some("cancelled_by_user") }), true));
                    }
                    _ = tokio::time::sleep(backoff) => {}
                }
                backoff *= 2;
                state = State::Resuming;
            }
        }
    }

    active_requests_inner.remove(&message_id_inner);
    Ok((
        json!({
            "fullContent": aurora_buffer.full_text,
            "streamingStarted": true,
            "finishReason": last_finish_reason
        }),
        false,
    ))
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
    mut abort_rx: tokio::sync::oneshot::Receiver<()>,
    active_requests: Arc<DashMap<String, tokio::sync::oneshot::Sender<()>>>,
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
        _ = &mut abort_rx => {
            log::warn!("[VCPClient] Non-streaming request aborted before response for message: {}", message_id);
            send_stream_event(StreamEvent::error(
                message_id.clone(),
                context.clone(),
                "请求已中止".to_string(),
            ));
            active_requests.remove(&message_id);
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
                    active_requests.remove(&message_id);
                    return Err(err_msg);
                }
            }
        }
    };

    active_requests.remove(&message_id);

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        let err_msg = format!("VCP服务器错误: {} - {}", status, text);
        send_stream_event(StreamEvent::error(
            message_id.clone(),
            context.clone(),
            err_msg.clone(),
        ));
        return Err(err_msg);
    }

    let vcp_response = match response.json::<Value>().await {
        Ok(json) => json,
        Err(e) => {
            let err_msg = format!("JSON解析失败: {}", e);
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
    send_stream_event(StreamEvent::aurora(
        message_id.clone(),
        AuroraUpdate {
            stable_blocks: None,
            stable_changed: false,
            tail_block: None,
            tail: None,
            tail_changed: false,
            tail_frame: None,
            tail_snapshot: None,
            content: Some(full_content.clone()),
            chunk: None,
        },
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
    let db_state = app.state::<DbState>();
    let pool = &db_state.pool;

    let row = sqlx::query("SELECT value FROM settings WHERE key = 'global'")
        .fetch_optional(pool)
        .await
        .map_err(|e| e.to_string())?;

    if let Some(row) = row {
        use sqlx::Row;
        let content: String = row.get("value");
        let settings = serde_json::from_str::<Settings>(&content)
            .unwrap_or_else(|_| create_default_settings());
        Ok(settings)
    } else {
        Ok(create_default_settings())
    }
}

/// 中止请求 Command: interruptRequest
/// 通过 messageId 立即触发对应的 oneshot 信号
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
    if let Some((_, sender)) = state.0.remove(&message_id) {
        log::info!(
            "[VCPClient] Found AbortController for messageId: {}, aborting...",
            message_id
        );
        let _ = sender.send(());
        log::info!(
            "[VCPClient] Request interrupted for messageId: {}. Remaining active requests: {}",
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
}

#[tauri::command]
pub async fn get_active_generations(
    app: tauri::AppHandle,
    active_requests: tauri::State<'_, ActiveRequests>,
) -> Result<Vec<ActiveGeneration>, String> {
    let db = app.state::<DbState>();
    let rows = sqlx::query(
        "SELECT msg_id, topic_id, owner_id, owner_type, created_at FROM active_generations ORDER BY created_at ASC"
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
        let error_suffix = match custom_error {
            Some(err) => format!("\n\n> VCP流式错误: {}", err),
            None => "\n\n> VCP流式错误: 生成意外中断".to_string(),
        };
        let final_content = if existing_content.is_empty() {
            error_suffix
        } else {
            format!("{}{}", existing_content, error_suffix)
        };

        sqlx::query(
            "UPDATE messages SET content = ?, finish_reason = 'error', is_thinking = 0 WHERE msg_id = ?",
        )
        .bind(final_content)
        .bind(msg_id)
        .execute(pool)
        .await
        .map_err(|e| e.to_string())?;

        sqlx::query("DELETE FROM active_generations WHERE msg_id = ?")
            .bind(msg_id)
            .execute(pool)
            .await
            .map_err(|e| e.to_string())?;
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
) -> Result<Value, String> {
    log::info!(
        "[VCPClient] recover_active_generation called for msg_id: {}",
        msg_id
    );

    // 1. 如果此消息在当前 active_requests 中，说明后台流式任务仍在正常进行/重连接续中
    if active_requests.0.contains_key(&msg_id) {
        log::info!(
            "[VCPClient] Active generation {} is running in background. Returning streaming status.",
            msg_id
        );
        return Ok(json!({ "status": "streaming" }));
    }

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

                    let db = app.state::<DbState>();
                    let row = sqlx::query(
                        "SELECT topic_id, owner_id, owner_type FROM active_generations WHERE msg_id = ?",
                    )
                    .bind(&msg_id)
                    .fetch_optional(&db.pool)
                    .await
                    .map_err(|e| e.to_string())?;

                    if let Some(r) = row {
                        use sqlx::Row;
                        let topic_id: String = r.get("topic_id");
                        let owner_id: String = r.get("owner_id");
                        let owner_type: String = r.get("owner_type");

                        let agent_id_row =
                            sqlx::query("SELECT agent_id FROM messages WHERE msg_id = ?")
                                .bind(&msg_id)
                                .fetch_optional(&db.pool)
                                .await
                                .map_err(|e| e.to_string())?;
                        let agent_id =
                            agent_id_row.and_then(|r| r.get::<Option<String>, _>("agent_id"));

                        crate::vcp_modules::chat::message_service::finalize_stream_message(
                            app.clone(),
                            &db.pool,
                            &owner_id,
                            &owner_type,
                            topic_id,
                            msg_id.clone(),
                            content.clone(),
                            false,
                            finish_reason.or(Some("completed".to_string())),
                            None,
                            agent_id,
                        )
                        .await?;
                    }

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
            let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
                .await
                .map_err(|e| format!("TCP connection failed: {}", e))?;

            let cmd = json!({
                "action": "query",
                "requestId": msg_id
            });

            use tokio::io::AsyncWriteExt;
            let cmd_str = cmd.to_string();
            let cmd_bytes = cmd_str.as_bytes();
            let len = cmd_bytes.len() as u32;
            stream
                .write_all(&len.to_be_bytes())
                .await
                .map_err(|e| e.to_string())?;
            stream
                .write_all(cmd_bytes)
                .await
                .map_err(|e| e.to_string())?;
            stream.flush().await.map_err(|e| e.to_string())?;

            log::info!("[VCPClient] Query command sent, waiting for response frame...");
            let mut reader = FramedRead::new(stream, LengthDelimitedCodec::new());
            if let Some(Ok(line)) = reader.next().await {
                let resp = serde_json::from_slice::<Value>(&line).map_err(|e| e.to_string())?;
                return Ok::<Value, String>(resp);
            }
            Err("No query response received (EOF)".to_string())
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
                    log::info!("[VCPClient] Session completed in helper memory. Finalizing message in SQLite database.");
                    let db = app.state::<DbState>();
                    let row = sqlx::query(
                        "SELECT topic_id, owner_id, owner_type FROM active_generations WHERE msg_id = ?",
                    )
                    .bind(&msg_id)
                    .fetch_optional(&db.pool)
                    .await
                    .map_err(|e| e.to_string())?;

                    if let Some(r) = row {
                        use sqlx::Row;
                        let topic_id: String = r.get("topic_id");
                        let owner_id: String = r.get("owner_id");
                        let owner_type: String = r.get("owner_type");

                        let agent_id_row =
                            sqlx::query("SELECT agent_id FROM messages WHERE msg_id = ?")
                                .bind(&msg_id)
                                .fetch_optional(&db.pool)
                                .await
                                .map_err(|e| e.to_string())?;
                        let agent_id =
                            agent_id_row.and_then(|r| r.get::<Option<String>, _>("agent_id"));

                        crate::vcp_modules::chat::message_service::finalize_stream_message(
                            app.clone(),
                            &db.pool,
                            &owner_id,
                            &owner_type,
                            topic_id,
                            msg_id.clone(),
                            content.clone(),
                            false,
                            last_finish_reason.or(Some("completed".to_string())),
                            None,
                            agent_id,
                        )
                        .await?;
                    }

                    log::info!("[VCPClient] Finalization complete. Sending stop command to helper to release memory.");
                    let _ = send_stop_to_helper(&app, &msg_id).await;

                    return Ok(json!({
                        "status": "completed",
                        "content": content
                    }));
                } else if status == "streaming" {
                    log::info!("[VCPClient] Session is still streaming in helper. Returning status and content to frontend.");
                    return Ok(json!({
                        "status": "streaming",
                        "content": content,
                        "lastEventIndex": resp["lastEventIndex"]
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

    let db = app.state::<DbState>();
    mark_message_as_error(
        &app,
        &db.pool,
        &msg_id,
        Some("后台进程已被系统销毁，流式对话中断".to_string()),
    )
    .await?;

    Ok(json!({ "status": "failed" }))
}

#[tauri::command]
#[allow(non_snake_case)]
#[allow(clippy::too_many_arguments)]
pub async fn resume_stream<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, ActiveRequests>,
    msg_id: String,
    topic_id: String,
    owner_id: String,
    owner_type: String,
    stream_channel: Channel<StreamEvent>,
    initial_content: Option<String>,
    last_event_index: Option<i64>,
) -> Result<Value, String> {
    log::info!(
        "[VCPClient] resume_stream called for messageId: {}, topicId: {}, lastEventIndex: {:?}",
        msg_id,
        topic_id,
        last_event_index
    );

    let client = Client::builder().build().map_err(|e| e.to_string())?;

    let pool = app.state::<DbState>().pool.clone();

    if let Some(ref content) = initial_content {
        let _ = sqlx::query("UPDATE messages SET content = ? WHERE msg_id = ?")
            .bind(content)
            .bind(&msg_id)
            .execute(&pool)
            .await;
    }

    let (abort_tx, abort_rx) = oneshot::channel();
    state.0.insert(msg_id.clone(), abort_tx);
    let _guard = ActiveRequestGuard::new(state.0.clone(), msg_id.clone());

    let context = json!({
        "topicId": topic_id,
        "groupId": if owner_type == "group" { Some(&owner_id) } else { None },
        "agentId": if owner_type == "agent" { Some(&owner_id) } else { None },
    });

    let (res, is_aborted) = match handle_streaming_request(
        &app,
        client,
        "",
        "",
        Value::Null,
        msg_id.clone(),
        Some(context.clone()),
        abort_rx,
        state.0.clone(),
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
                "[VCPClient] resume_stream failed during handle_streaming_request: {}",
                e
            );
            let _ =
                mark_message_as_error(&app, &pool, &msg_id, Some(format!("接续失败: {}", e))).await;
            return Err(e);
        }
    };

    let finish_reason = if is_aborted {
        Some("cancelled_by_user".to_string())
    } else {
        res["finishReason"].as_str().map(|s| s.to_string())
    };

    let pool = app.state::<DbState>().pool.clone();

    log::info!("[VCPClient] resume_stream completed. Finalizing message.");
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
        if owner_type == "agent" {
            Some(owner_id.clone())
        } else {
            None
        },
    )
    .await?;

    Ok(res)
}

use crate::vcp_modules::media_processor::convert_local_image_for_multimodal;
use crate::vcp_modules::infra::utils::normalize_vcp_url;
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

    let (res, is_aborted) = match perform_vcp_request(&app, state.0.clone(), payload, Some(stream_channel.clone())).await {
        Ok(val) => val,
        Err(e) => {
            if is_stream {
                let pool = app.state::<crate::vcp_modules::db_manager::DbState>().pool.clone();
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
                            if path_buf.exists() {
                                // 提取扩展名决定 mime_type
                                let ext = path_buf
                                    .extension()
                                    .and_then(|e| e.to_str())
                                    .unwrap_or("")
                                    .to_lowercase();
                                let (mime, part_type) = match ext.as_str() {
                                    "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "heic"
                                    | "heif" | "avif" => ("image", "image_url"),
                                    "mp3" | "wav" | "ogg" | "flac" | "aac" | "m4a" | "opus"
                                    | "amr" => ("audio", "input_audio"),
                                    "mp4" | "webm" | "3gp" | "3g2" | "mov" => {
                                        ("video", "image_url")
                                    }
                                    _ => ("application", "file_url"), // 非支持多模态格式退化回退
                                };

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
                                new_parts.push(json!({
                                    "type": "text",
                                    "text": format!("[附件文件: {}]", clean_path)
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

/// 3. 抽离自适应降帧流式请求循环
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
    let mut is_aborted = false;
    let mut aurora_buffer = AuroraBuffer::new();
    let mut pending_aurora_chunk = String::new();
    let mut last_aurora_parse = std::time::Instant::now() - Duration::from_millis(33);

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

    // 定义线读取的通用装箱流类型，用以抹平 reqwest 底层繁琐的泛型定义
    type BoxedLineStream = Box<dyn futures_util::Stream<Item = Result<String, std::io::Error>> + Unpin + Send>;
    let to_line_stream = |resp: reqwest::Response| -> BoxedLineStream {
        let stream = resp.bytes_stream().map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e));
        let reader = StreamReader::new(stream);
        let framed = FramedRead::new(reader, LinesCodec::new_with_max_length(512 * 1024));
        let mapped = framed.map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e));
        Box::new(mapped)
    };

    // 1. 发起初始请求
    let res_future = client
        .post(final_url)
        .header(AUTHORIZATION, format!("Bearer {}", api_key))
        .header(CONTENT_TYPE, "application/json")
        .json(&request_body)
        .send();

    let mut lines: Option<BoxedLineStream>;

    tokio::select! {
        _ = &mut abort_rx => {
            log::warn!("[VCPClient] Request aborted before response for message: {}", message_id_inner);
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
                    send_stream_event(StreamEvent::error(
                        message_id_inner.clone(),
                        context_inner.clone(),
                        format!("网络请求异常: {}", e),
                    ));
                    active_requests_inner.remove(&message_id_inner);
                    return Err(e.to_string());
                }
            }
        }
    }

    // 2. 主循环与断点续传重连状态机
    let mut retry_count = 0;
    const MAX_RETRIES: u32 = 5;
    let mut backoff = Duration::from_millis(500);

    'main_loop: loop {
        if is_aborted {
            break 'main_loop;
        }

        if let Some(ref mut line_stream) = lines {
            loop {
                tokio::select! {
                    _ = &mut abort_rx => {
                        is_aborted = true;
                        log::warn!("[VCPClient] Stream deep-polling detected abort for message: {}", message_id_inner);
                        aurora_buffer.finalize();
                        send_aurora_update(&mut aurora_buffer, true, true, Some("cancelled_by_user".to_string()), Some("请求已中止".to_string()));
                        active_requests_inner.remove(&message_id_inner);
                        break 'main_loop;
                    }
                    line_res = line_stream.next() => {
                        match line_res {
                            Some(Ok(line)) => {
                                if line.trim().is_empty() { continue; }
                                if line.starts_with("data: ") {
                                    let data = line.trim_start_matches("data: ").trim();
                                    if data == "[DONE]" {
                                        log::debug!("[VCPClient] Stream finished normally with [DONE] for message: {}", message_id_inner);
                                        flush_aurora_parse(&mut aurora_buffer, &mut pending_aurora_chunk, &mut last_aurora_parse, true);
                                        aurora_buffer.finalize();
                                        send_aurora_update(&mut aurora_buffer, true, true, last_finish_reason.clone(), None);
                                        break 'main_loop;
                                    }
                                    if let Ok(chunk) = serde_json::from_str::<Value>(data) {
                                        let mut text_chunk = String::new();
                                        if let Some(choice) = chunk["choices"].as_array().and_then(|a| a.first()) {
                                            if let Some(text) = choice["delta"]["content"].as_str() {
                                                full_content.push_str(text);
                                                text_chunk.push_str(text);
                                            }
                                            if let Some(reason) = choice["finish_reason"].as_str() {
                                                last_finish_reason = Some(
                                                    if reason == "stop" { "completed".to_string() } else { reason.to_string() }
                                                );
                                            }
                                        }

                                        if !text_chunk.is_empty() {
                                            pending_aurora_chunk.push_str(&text_chunk);
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
                                log::warn!("[VCPClient] Stream read error, entering reconnect: {:?}", e);
                                break; // 跳出内层流式循环，触发断点重连
                            }
                            None => {
                                // TCP 连接被服务器关闭
                                if !full_content.is_empty() || last_finish_reason.is_some() {
                                    log::debug!("[VCPClient] Stream ended without [DONE] but content was received. Treating as normal end.");
                                    flush_aurora_parse(&mut aurora_buffer, &mut pending_aurora_chunk, &mut last_aurora_parse, true);
                                    aurora_buffer.finalize();
                                    send_aurora_update(&mut aurora_buffer, true, true, last_finish_reason.clone(), None);
                                    break 'main_loop;
                                } else {
                                    log::warn!("[VCPClient] Stream ended unexpectedly (None), entering reconnect");
                                    break; // 跳出内层流式循环，触发断点重连
                                }
                            }
                        }
                    }
                }
            }
        }

        // 3. 断点续传与重连状态机逻辑
        lines = None; // 丢弃旧的线读取流

        // 🌟 修复温回归/后台恢复：若 App 处于后台，则挂起重连循环，等待 App 回到前台后再试，防止在后台耗尽重试次数
        while !crate::vcp_modules::infra::vcp_log_service::APP_IN_FOREGROUND.load(std::sync::atomic::Ordering::SeqCst) {
            log::info!("[VCPClient] App is in background. Suspending reconnection for message: {}", message_id_inner);
            tokio::select! {
                _ = &mut abort_rx => {
                    is_aborted = true;
                    break 'main_loop;
                }
                _ = tokio::time::sleep(Duration::from_secs(2)) => {}
            }
        }

        if retry_count >= MAX_RETRIES {
            log::error!("[VCPClient] Max retries reached ({}) for message: {}", MAX_RETRIES, message_id_inner);
            send_stream_event(StreamEvent::error(
                message_id_inner.clone(),
                context_inner.clone(),
                "网络连接意外断开，重连失败".to_string(),
            ));
            break 'main_loop;
        }

        retry_count += 1;
        log::info!("[VCPClient] Reconnecting {}/{} for message: {}", retry_count, MAX_RETRIES, message_id_inner);

        // 发射 reconnecting 事件通知前端展示重连状态
        send_stream_event(StreamEvent {
            r#type: "reconnecting".into(),
            message_id: message_id_inner.clone(),
            context: context_inner.clone(),
            ..Default::default()
        });

        // 进行指数退避等待，且支持响应前端的主止信号
        tokio::select! {
            _ = &mut abort_rx => {
                is_aborted = true;
                log::warn!("[VCPClient] Aborted during reconnection backoff sleep");
                break 'main_loop;
            }
            _ = tokio::time::sleep(backoff) => {}
        }
        backoff *= 2;

        // 构造断点流式接续 URL: GET /api/chat/stream?msg_id={message_id}
        let reconnect_url = match Url::parse(final_url) {
            Ok(mut url) => {
                url.set_path("/api/chat/stream");
                url.set_query(Some(&format!("msg_id={}", message_id_inner)));
                url.to_string()
            }
            Err(_) => {
                log::error!("[VCPClient] Failed to parse final_url for reconnection");
                break 'main_loop;
            }
        };

        // 计算当前已接收的文本字符数作为 Last-Event-ID
        let offset = aurora_buffer.full_text.chars().count();
        log::info!("[VCPClient] Re-establishing stream connection to {} with Last-Event-ID: {}", reconnect_url, offset);

        let req_res = client
            .get(&reconnect_url)
            .header(AUTHORIZATION, format!("Bearer {}", api_key))
            .header("Last-Event-ID", offset.to_string())
            .send()
            .await;

        match req_res {
            Ok(resp) if resp.status().is_success() => {
                log::info!("[VCPClient] Reconnection successful for message: {}", message_id_inner);
                retry_count = 0; // 重置重试计数器
                backoff = Duration::from_millis(500); // 重置退避延迟
                lines = Some(to_line_stream(resp));
            }
            Ok(resp) => {
                log::warn!("[VCPClient] Reconnection returned non-success status: {}", resp.status());
            }
            Err(e) => {
                log::warn!("[VCPClient] Reconnection request failed: {:?}", e);
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
        is_aborted,
    ))
}


/// 4. 抽离非流式请求循环
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
        .map(|r| if r == "stop" { "completed".to_string() } else { r.to_string() });

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



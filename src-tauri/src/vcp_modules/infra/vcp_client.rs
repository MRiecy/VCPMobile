use crate::vcp_modules::media_processor::convert_local_image_for_multimodal;
use dashmap::{DashMap, DashSet};
use futures_util::StreamExt;
use futures_util::TryStreamExt;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::Error as IoError;

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
    pub fn data(message_id: String, chunk: Value, context: Option<Value>) -> Self {
        Self {
            r#type: "data".into(),
            chunk: Some(chunk),
            message_id,
            context,
            ..Default::default()
        }
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
        perform_vcp_request(&app, state.0.clone(), payload, Some(stream_channel.clone())).await?;

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

fn get_or_calculate_message_hash(content_hash_opt: Option<&str>, content: &Value) -> String {
    use crate::vcp_modules::infra::utils::{calculate_sha256, is_valid_cas_hash};

    if let Some(raw_hash) = content_hash_opt {
        let clean_hash = raw_hash.trim_start_matches("sha256:");
        if is_valid_cas_hash(clean_hash) {
            return format!("sha256:{}", clean_hash);
        }
    }

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

    let send_stream_event = |event: StreamEvent| {
        if let Some(ref ch) = stream_channel {
            let _ = ch.send(event);
        }
    };

    // === 0. 数据验证和规范化 ===
    let mut message_timestamp_bindings = Vec::new();
    let mut messages: Vec<Value> = Vec::new();
    for (index, msg_val) in payload.messages.into_iter().enumerate() {
        if !msg_val.is_object() {
            messages.push(json!({"role": "system", "content": "[Invalid message]"}));
            continue;
        }

        let mut msg = msg_val.clone();
        let mut timestamp_meta = None;
        if let Some(obj) = msg.as_object_mut() {
            if let Some(meta) = obj.remove("__vcpchatTimestampMeta") {
                timestamp_meta = Some(meta);
            }
        }
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
                                    "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "tiff"
                                    | "tif" | "heic" | "heif" | "avif" | "ico" => {
                                        ("image", "image_url")
                                    }
                                    "mp3" | "wav" | "ogg" | "flac" | "aac" | "m4a" | "opus"
                                    | "wma" | "amr" | "aiff" | "aif" => ("audio", "input_audio"),
                                    "mp4" | "mkv" | "webm" | "avi" | "mov" | "flv" | "m4v"
                                    | "3gp" | "3g2" | "wmv" | "ts" | "mts" | "m2ts" | "qt" => {
                                        ("video", "image_url")
                                    }
                                    _ => ("application", "file_url"), // 非多模态文件回退
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

                            // 修复：若文件不存在或读取失败，至少保留文本描述，避免内容静默丢失
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

                let content_hash_opt = meta.get("contentHash").and_then(|h| h.as_str());
                let final_content_val = msg.get("content").unwrap_or(&Value::Null);
                let sent_message_hash =
                    get_or_calculate_message_hash(content_hash_opt, final_content_val);

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

        messages.push(msg);
    }

    // === 1. 读取设置与动态路由切换 ===
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

    // === 2. 上下文注入 ===
    let has_system = messages.iter().any(|m| m["role"] == "system");
    let system_inserted = !has_system;
    if !has_system {
        messages.insert(0, json!({"role": "system", "content": ""}));
    }

    // === 4. 准备请求体 ===
    let is_stream = payload.model_config["stream"].as_bool().unwrap_or(false);
    let mut request_body = payload.model_config.clone();
    if let Some(obj) = request_body.as_object_mut() {
        obj.insert("messages".to_string(), json!(messages));
        obj.insert("requestId".to_string(), json!(payload.message_id));
        obj.insert("stream".to_string(), json!(is_stream));
        if !message_timestamp_bindings.is_empty() {
            let mut final_bindings = message_timestamp_bindings.clone();
            if system_inserted {
                for binding in final_bindings.iter_mut() {
                    if let Some(binding_obj) = binding.as_object_mut() {
                        if let Some(idx_val) = binding_obj.get_mut("sentMessageIndex") {
                            if let Some(idx) = idx_val.as_u64() {
                                *idx_val = json!(idx + 1);
                            }
                        }
                    }
                }
            }
            obj.insert(
                "vcpchatExtensions".to_string(),
                json!({
                    "schemaVersion": 1,
                    "messageMetadataMode": "hash_only",
                    "messageTimestampBindings": final_bindings
                }),
            );
        }
    }

    // === 5. 配置网络请求 ===
    let client = Client::builder()
        // 不设 read_timeout：数小时自循环中，任何 read_timeout 都是定时炸弹
        // tcp_keepalive(60s) 维持 TCP 层活性，防止 NAT/防火墙静默丢弃空闲连接
        .tcp_keepalive(Duration::from_secs(60))
        .build()
        .map_err(|e| e.to_string())?;

    // 创建并注册中止信号
    let (abort_tx, abort_rx) = oneshot::channel();
    active_requests.insert(payload.message_id.clone(), abort_tx);
    let _guard = ActiveRequestGuard::new(active_requests.clone(), payload.message_id.clone());

    let message_id = payload.message_id.clone();
    let context = payload.context.clone();
    let api_key = payload.vcp_api_key.clone();

    if is_stream {
        // === 6. 流式处理模式 (同步等待，以便串行调用) ===
        let _app_handle = app.clone();
        let message_id_inner = message_id.clone();
        let context_inner = context.clone();
        let active_requests_inner = active_requests.clone();

        let mut full_content = String::new();
        let mut last_finish_reason: Option<String> = None;
        let mut is_aborted = false;
        let mut abort_rx = abort_rx; // 取得所有权进入循环
        let mut aurora_buffer = AuroraBuffer::new();
        let mut last_aurora_send = std::time::Instant::now();

        // 辅助闭包：发送 Aurora 更新事件（稀疏序列化：只发送有变化的字段）
        let send_aurora_update = |buffer: &AuroraBuffer,
                                  stable_changed: bool,
                                  tail_changed: bool,
                                  finish_reason: Option<String>,
                                  error: Option<String>| {
            let is_final = finish_reason.is_some() || error.is_some();
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
                    content: if is_final {
                        Some(buffer.full_text.clone())
                    } else {
                        None
                    },
                },
                context_inner.clone(),
            );
            event.finish_reason = finish_reason;
            event.error = error;
            send_stream_event(event);
        };

        let res_future = client
            .post(&final_url)
            .header(AUTHORIZATION, format!("Bearer {}", api_key))
            .header(CONTENT_TYPE, "application/json")
            .json(&request_body)
            .send();

        tokio::select! {
            _ = &mut abort_rx => {
                                log::warn!("[VCPClient] Request aborted before response for message: {}", message_id_inner);
                                aurora_buffer.finalize();
                send_aurora_update(&aurora_buffer, true, true, Some("cancelled_by_user".to_string()), Some("请求已中止".to_string()));
                active_requests_inner.remove(&message_id_inner);
                return Ok((json!({ "fullContent": aurora_buffer.full_text, "streamingStarted": false }), true));
            }
            response_res = res_future => {
                match response_res {
                    Ok(resp) if resp.status().is_success() => {
                        let stream = resp.bytes_stream().map_err(IoError::other);
                        let reader = StreamReader::new(stream);
                        let mut lines = FramedRead::new(reader, LinesCodec::new_with_max_length(512 * 1024));

                        loop {
                            tokio::select! {
                                // 核心修复：即使在等待数据的间隙，也能捕获中断信号
                                _ = &mut abort_rx => {
                                    is_aborted = true;
                                    log::warn!("[VCPClient] Stream deep-polling detected abort for message: {}", message_id_inner);
                                    aurora_buffer.finalize();
                                    send_aurora_update(&aurora_buffer, true, true, Some("cancelled_by_user".to_string()), Some("请求已中止".to_string()));

                                    // 显式清理，防止 race
                                    active_requests_inner.remove(&message_id_inner);
                                    break;
                                }
                                line_res = lines.next() => {
                                    match line_res {
                                        Some(Ok(line)) => {
                                            if line.trim().is_empty() { continue; }
                                            if line.starts_with("data: ") {
                                                let data = line.trim_start_matches("data: ").trim();
                                                if data == "[DONE]" {
                                                    log::debug!("[VCPClient] Stream finished normally with [DONE] for message: {}", message_id_inner);
                                                    aurora_buffer.finalize();
                                                    send_aurora_update(&aurora_buffer, true, true, last_finish_reason.clone(), None);
                                                    break;
                                                }
                                                if let Ok(chunk) = serde_json::from_str::<Value>(data) {
                                                    // 累加全量内容并驱动 Aurora 沉淀
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
                                                        aurora_buffer.append_chunk(&text_chunk);
                                                        let (stable_changed, tail_changed) = aurora_buffer.process_queue();
                                                        if stable_changed || (tail_changed && last_aurora_send.elapsed().as_millis() >= 33) {
                                                            send_aurora_update(&aurora_buffer, stable_changed, tail_changed, None, None);
                                                            last_aurora_send = std::time::Instant::now();
                                                        }
                                                    }

                                                    // 保留原始 data 事件以保证兼容性
                                                    send_stream_event(StreamEvent::data(
                                                        message_id_inner.clone(),
                                                        chunk,
                                                        context_inner.clone(),
                                                    ));

                                                }
                                            }
                                        }
                                        Some(Err(e)) => {
                                            log::error!("[VCPClient] Stream read error: {:?}", e);
                                            aurora_buffer.finalize();
                                            send_aurora_update(&aurora_buffer, true, true, Some("error".to_string()), Some(format!("流读取错误: {}", e)));
                                            send_stream_event(StreamEvent::error(
                                                message_id_inner.clone(),
                                                context_inner.clone(),
                                                format!("流读取错误: {}", e),
                                            ));

                                            break;
                                        }
                                        None => {
                                            // 修复：若此前已收到有效 chunk，则视为正常结束（对齐桌面端行为）
                                            aurora_buffer.finalize();
                                            if !full_content.is_empty() || last_finish_reason.is_some() {
                                                log::debug!("[VCPClient] Stream ended without [DONE] but content was received. Treating as normal end.");
                                                send_aurora_update(&aurora_buffer, true, true, last_finish_reason.clone(), None);
                                            } else {
                                                log::warn!("[VCPClient] Stream ended unexpectedly (None)");
                                                send_aurora_update(&aurora_buffer, true, true, Some("error".to_string()), Some("网络连接意外断开".to_string()));
                                                send_stream_event(StreamEvent::error(
                                                    message_id_inner.clone(),
                                                    context_inner.clone(),
                                                    "网络连接意外断开".to_string(),
                                                ));

                                            }
                                            break;
                                        }
                                    }
                                }
                            }
                        }
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

        active_requests_inner.remove(&message_id_inner);
        Ok((
            json!({
                "fullContent": aurora_buffer.full_text,
                "streamingStarted": true,
                "finishReason": last_finish_reason
            }),
            is_aborted,
        ))
    } else {
        // === 7. 非流式响应模式 ===
        let response = client
            .post(&final_url)
            .header(AUTHORIZATION, format!("Bearer {}", api_key))
            .header(CONTENT_TYPE, "application/json")
            .json(&request_body)
            .send()
            .await
            .map_err(|e| format!("VCP请求失败: {}", e))?;

        active_requests.remove(&message_id);

        if !response.status().is_success() {
            let status = response.status();
            return Err(format!("VCP响应错误: {}", status));
        }

        let vcp_response = response
            .json::<Value>()
            .await
            .map_err(|e| format!("JSON解析失败: {}", e))?;
        Ok((json!({"response": vcp_response, "context": context}), false))
    }
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

/// Normalize a VCP server URL by appending `/v1/chat/completions` if missing.
/// Handles URLs with or without trailing slashes in the existing path.
pub fn normalize_vcp_url(url_str: &str) -> String {
    if let Ok(url) = Url::parse(url_str) {
        if !url.path().ends_with("/chat/completions") {
            let mut url = url;
            let new_path = if url.path().ends_with('/') {
                format!("{}v1/chat/completions", url.path())
            } else {
                format!("{}/v1/chat/completions", url.path())
            };
            url.set_path(&new_path);
            return url.to_string();
        }
    }
    url_str.to_string()
}

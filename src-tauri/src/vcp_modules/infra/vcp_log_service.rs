use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tokio::sync::{mpsc, watch};
use tokio::time::{sleep, Duration};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use url::Url;

static HEARTBEAT_INTERVAL_MS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(15000);

lazy_static::lazy_static! {
static ref LOG_CONNECTION_ACTIVE: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
static ref LOG_SENDER: Arc<tokio::sync::Mutex<Option<mpsc::UnboundedSender<Value>>>> = Arc::new(tokio::sync::Mutex::new(None));
// 关键修复：保持 Sender 和一个 Receiver 都在生命周期内，防止通道因无接收者而被视为关闭
static ref WS_URL_CHANNEL: (watch::Sender<Option<Url>>, watch::Receiver<Option<Url>>) = watch::channel(None);
static ref CURRENT_LOG_STATUS: Arc<tokio::sync::RwLock<String>> = Arc::new(tokio::sync::RwLock::new("closed".to_string()));
static ref HEARTBEAT_RESET_TX: Arc<tokio::sync::Mutex<Option<mpsc::Sender<()>>>> = Arc::new(tokio::sync::Mutex::new(None));
}

#[tauri::command]
pub async fn set_vcp_log_heartbeat(interval_ms: u64) -> Result<(), String> {
    HEARTBEAT_INTERVAL_MS.store(interval_ms, Ordering::SeqCst);
    let tx_lock = HEARTBEAT_RESET_TX.lock().await;
    if let Some(tx) = tx_lock.as_ref() {
        let _ = tx.send(()).await;
    }
    Ok(())
}

pub async fn get_vcp_log_status_internal() -> String {
    CURRENT_LOG_STATUS.read().await.clone()
}

#[tauri::command]
pub async fn send_vcp_log_message(payload: serde_json::Value) -> Result<(), String> {
    let sender_lock = LOG_SENDER.lock().await;
    if let Some(sender) = sender_lock.as_ref() {
        sender
            .send(payload)
            .map_err(|e| format!("Failed to send message to VCPLog: {}", e))?;
        Ok(())
    } else {
        Err("VCPLog connection is not active".to_string())
    }
}

fn parse_log_url(url: &str, key: &str) -> Result<Url, String> {
    let mut base_url = url.trim_end_matches('/').to_string();
    if !base_url.contains("/VCPlog") {
        base_url.push_str("/VCPlog");
    }

    let url_with_key = if base_url.contains("VCP_Key=") {
        base_url
    } else {
        if !base_url.ends_with('/') {
            base_url.push('/');
        }
        format!("{}VCP_Key={}", base_url, key)
    };

    Url::parse(&url_with_key).map_err(|e| format!("Invalid URL: {}", e))
}

#[tauri::command]
pub async fn init_vcp_log_connection(
    app: AppHandle,
    url: String,
    key: String,
) -> Result<(), String> {
    init_vcp_log_connection_internal(app, url, key).await
}

pub async fn init_vcp_log_connection_internal<R: tauri::Runtime>(
    app: AppHandle<R>,
    url: String,
    key: String,
) -> Result<(), String> {
    // 如果 URL 或 Key 为空，发送 None 以停止现有连接并进入静默等待
    if url.trim().is_empty() || key.trim().is_empty() {
        let _ = WS_URL_CHANNEL.0.send(None);
        return Ok(());
    }

    let ws_url = parse_log_url(&url, &key)?;

    // Always send the new URL to the watch channel
    let _ = WS_URL_CHANNEL.0.send(Some(ws_url.clone()));

    if LOG_CONNECTION_ACTIVE.swap(true, Ordering::SeqCst) {
        return Ok(());
    }

    let h = app.clone();
    tauri::async_runtime::spawn(async move {
        start_vcp_log_listener(h).await;
    });

    Ok(())
}

async fn start_vcp_log_listener<R: tauri::Runtime>(app_handle: AppHandle<R>) {
    let mut url_rx = WS_URL_CHANNEL.0.subscribe();

    // 创建 mpsc 通道用于回传消息
    let (tx, mut rx) = mpsc::unbounded_channel::<Value>();

    // 将发送端存储在全局静态变量中供 send_vcp_log_message 使用
    {
        let mut sender_lock = LOG_SENDER.lock().await;
        *sender_lock = Some(tx);
    }

    let mut retry_delay = Duration::from_millis(1000);
    loop {
        // 获取当前 URL
        let ws_url = {
            let val = url_rx.borrow().clone();
            match val {
                Some(u) => u,
                None => {
                    if url_rx.changed().await.is_err() {
                        break;
                    }
                    continue;
                }
            }
        };

        let masked_url = if ws_url.as_str().contains("VCP_Key=") {
            let parts: Vec<&str> = ws_url.as_str().split("VCP_Key=").collect();
            format!("{}VCP_Key=********", parts[0])
        } else {
            ws_url.to_string()
        };
        log::info!("[VCPLog] Attempting to connect to {}...", masked_url);

        {
            *CURRENT_LOG_STATUS.write().await = "connecting".to_string();
        }

        let _ = app_handle.emit(
            "vcp-system-event",
            serde_json::json!({
                "type": "vcp-log-status",
                "status": "connecting",
                "message": "连接中...",
                "source": "VCPLog"
            }),
        );

        let mut request = match ws_url.as_str().into_client_request() {
            Ok(req) => req,
            Err(e) => {
                {
                    *CURRENT_LOG_STATUS.write().await = "error".to_string();
                }
                log::error!(
                    "[VCPLog] Failed to build request: {}. Retrying in 5 seconds...",
                    e
                );
                let _ = app_handle.emit(
                    "vcp-system-event",
                    serde_json::json!({
                        "type": "vcp-log-status",
                        "status": "error",
                        "message": "连接错误",
                        "source": "VCPLog"
                    }),
                );

                // 错误卡片 1：请求构建失败 (例如 URL 格式错误)
                let _ = app_handle.emit(
                    "vcp-system-event",
                    serde_json::json!({
                        "type": "vcp-log-message",
                        "data": {
                            "id": "vcp_log_connection_status",
                            "status": "error",
                            "tool_name": "VCPLog 请求异常",
                            "content": format!("❌ 无法构造请求: {}\n\n提示：请检查配置的 URL 格式是否正确。", e),
                            "source": "VCPLog"
                        }
                    }),
                );

                tokio::select! {
                    _ = url_rx.changed() => {},
                    _ = sleep(retry_delay) => {},
                }
                retry_delay = (retry_delay * 2).min(Duration::from_secs(60));
                continue;
            }
        };

        if let Some(host) = ws_url.host_str() {
            let host_with_port = if let Some(port) = ws_url.port() {
                format!("{}:{}", host, port)
            } else {
                host.to_string()
            };
            if let Ok(val) = host_with_port.parse() {
                request.headers_mut().insert("Host", val);
            }

            let origin_scheme = match ws_url.scheme() {
                "wss" => "https",
                _ => "http",
            };
            let origin = if let Some(port) = ws_url.port() {
                format!("{}://{}:{}", origin_scheme, host, port)
            } else {
                format!("{}://{}", origin_scheme, host)
            };
            if let Ok(val) = origin.parse() {
                request.headers_mut().insert("Origin", val);
            }
        }

        request.headers_mut().insert(
            "User-Agent",
            "Mozilla/5.0 (Linux; Android 10; K) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Mobile Safari/537.36".parse().unwrap()
        );

        match tokio::time::timeout(Duration::from_secs(10), connect_async(request)).await {
            Ok(connection_result) => match connection_result {
                Ok((ws_stream, _)) => {
                    retry_delay = Duration::from_millis(1000);
                    {
                        *CURRENT_LOG_STATUS.write().await = "connected".to_string();
                    }
                    log::info!("[VCPLog] Connected successfully to {}", masked_url);

                    let (mut ws_write, mut ws_read) = ws_stream.split();

                    let _ = app_handle.emit(
                        "vcp-system-event",
                        serde_json::json!({
                            "type": "vcp-log-status",
                            "status": "connected",
                            "message": "已连接",
                            "source": "VCPLog"
                        }),
                    );

                    // 额外发送一条连接成功的通知卡片
                    let _ = app_handle.emit(
                        "vcp-system-event",
                        serde_json::json!({
                            "type": "vcp-log-message",
                            "data": {
                                "id": "vcp_log_connection_status",
                                "status": "success",
                                "tool_name": "VCPLog",
                                "content": "✅ VCPLog 连接成功！已建立实时数据通道。",
                                "source": "VCPLog"
                            }
                        }),
                    );

                    let (reset_tx, mut reset_rx) = mpsc::channel::<()>(8);
                    {
                        let mut tx_lock = HEARTBEAT_RESET_TX.lock().await;
                        *tx_lock = Some(reset_tx);
                    }

                    let initial_ms = HEARTBEAT_INTERVAL_MS.load(Ordering::SeqCst);
                    let mut heartbeat_timer = Box::pin(sleep(Duration::from_millis(initial_ms)));

                    loop {
                        tokio::select! {
                            // 监听 URL 变更
                            _ = url_rx.changed() => {
                                log::info!("[VCPLog] URL changed, closing current connection.");
                                break;
                            }
                            // 监听心跳重置信号
                            Some(_) = reset_rx.recv() => {
                                let current_ms = HEARTBEAT_INTERVAL_MS.load(Ordering::SeqCst);
                                log::info!("[VCPLog] Heartbeat interval updated to {}ms, resetting timer.", current_ms);
                                heartbeat_timer.as_mut().reset(tokio::time::Instant::now() + Duration::from_millis(current_ms));
                            }
                            // 心跳周期触发
                            _ = &mut heartbeat_timer => {
                                if let Err(e) = ws_write.send(Message::Ping(vec![].into())).await {
                                    log::error!("[VCPLog] Failed to send Ping: {}", e);
                                    break;
                                }
                                let current_ms = HEARTBEAT_INTERVAL_MS.load(Ordering::SeqCst);
                                heartbeat_timer.as_mut().reset(tokio::time::Instant::now() + Duration::from_millis(current_ms));
                            }
                            // 处理接收到的消息
                            msg_result = ws_read.next() => {
                                match msg_result {
                                    Some(Ok(msg)) => {
                                        if msg.is_text() {
                                            let text = msg.to_text().unwrap_or_default();
                                            match serde_json::from_str::<Value>(text) {
                                                Ok(payload) => {
                                                    if payload_has_agent_message_hint(&payload, 0)
                                                    {
                                                        log::info!(
                                                            "[VCPLog] Incoming AgentMessage-related payload summary: {}",
                                                            summarize_agent_message_payload(&payload)
                                                        );
                                                    }
                                                    maybe_push_agent_message_notification(
                                                        &app_handle,
                                                        &payload,
                                                    );
                                                    if let Err(e) =
                                                        app_handle.emit("vcp-system-event", payload)
                                                    {
                                                        log::error!(
                                                            "[VCPLog] Failed to emit event to frontend: {}",
                                                            e
                                                        );
                                                    }
                                                }
                                                Err(_) => {
                                                    let raw_payload = serde_json::json!({
                                                        "type": "raw_text",
                                                        "data": text
                                                    });
                                                    maybe_push_agent_message_notification(
                                                        &app_handle,
                                                        &raw_payload,
                                                    );
                                                    let _ = app_handle.emit("vcp-system-event", serde_json::json!({
                                                        "type": "raw_text",
                                                        "data": text
                                                    }));
                                                }
                                            }
                                        }
                                    }
                                    Some(Err(e)) => {
                                        log::error!("[VCPLog] WebSocket error during read: {}", e);
                                        break;
                                    }
                                    None => {
                                        log::warn!("[VCPLog] Connection closed by server.");
                                        break;
                                    }
                                }
                            }
                            // 处理待发送的消息
                            payload_opt = rx.recv() => {
                                if let Some(payload) = payload_opt {
                                    if let Ok(text) = serde_json::to_string(&payload) {
                                        if let Err(e) = ws_write.send(Message::Text(text.into())).await {
                                            log::error!("[VCPLog] Failed to send message: {}", e);
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                    }

                    {
                        let mut tx_lock = HEARTBEAT_RESET_TX.lock().await;
                        *tx_lock = None;
                    }

                    log::info!("[VCPLog] Disconnected from {}.", ws_url);
                    {
                        *CURRENT_LOG_STATUS.write().await = "closed".to_string();
                    }
                    let _ = app_handle.emit(
                        "vcp-system-event",
                        serde_json::json!({
                            "type": "vcp-log-status",
                            "status": "closed",
                            "message": "连接已断开",
                            "source": "VCPLog"
                        }),
                    );
                }
                Err(e) => {
                    {
                        *CURRENT_LOG_STATUS.write().await = "error".to_string();
                    }
                    log::error!("[VCPLog] Connection Error: {}. Status: {}", e, e);
                    let _ = app_handle.emit(
                        "vcp-system-event",
                        serde_json::json!({
                            "type": "vcp-log-status",
                            "status": "error",
                            "message": "连接错误",
                            "source": "VCPLog"
                        }),
                    );

                    // 额外发送一条连接错误的通知卡片，辅助排查 (错误卡片 2)
                    let _ = app_handle.emit(
                        "vcp-system-event",
                        serde_json::json!({
                            "type": "vcp-log-message",
                            "data": {
                                "id": "vcp_log_connection_status",
                                "status": "error",
                                "tool_name": "VCPLog 连接失败",                                
                                "content": format!("❌ 连接错误: {}\n\n提示：\n1. 请检查桌面端 VCP 是否已开启且 VCPLog 服务正常。\n2. 检查 VCP API 地址和 Key 配置是否正确。", e),
                                "source": "VCPLog"
                            }
                        }),
                    );
                }
            },
            Err(_) => {
                {
                    *CURRENT_LOG_STATUS.write().await = "error".to_string();
                }
                log::error!(
                    "[VCPLog] Connection timed out after 10 seconds. Retrying in 5 seconds..."
                );
                let _ = app_handle.emit(
                    "vcp-system-event",
                    serde_json::json!({
                        "type": "vcp-log-status",
                        "status": "error",
                        "message": "连接错误",
                        "source": "VCPLog"
                    }),
                );

                // 错误卡片 3：连接超时
                let _ = app_handle.emit(
                    "vcp-system-event",
                    serde_json::json!({
                        "type": "vcp-log-message",
                        "data": {
                            "id": "vcp_log_connection_status",
                            "status": "error",
                            "tool_name": "VCPLog 连接超时",
                            "content": "❌ 连接 VCPLog 超时 (10s)。\n\n提示：\n1. 请检查桌面端是否处于运行状态。\n2. 确认手机与电脑是否处于同一局域网。",
                            "source": "VCPLog"
                        }
                    }),
                );
            }
        }

        tokio::select! {
            _ = url_rx.changed() => log::info!("[VCPLog] URL changed during retry wait."),
            _ = sleep(retry_delay) => {},
        }
        retry_delay = (retry_delay * 2).min(Duration::from_secs(60));
    }
    LOG_CONNECTION_ACTIVE.store(false, Ordering::SeqCst);
    log::info!("[VCPLog] Listener task terminated, connection flag reset.");
}

fn maybe_push_agent_message_notification<R: tauri::Runtime>(
    app_handle: &AppHandle<R>,
    payload: &Value,
) {
    if let Some((title, body)) = extract_agent_message_notification(payload) {
        log::info!(
            "[VCPLog] AgentMessage payload matched for Android notification (title_len={}, body_len={}).",
            title.chars().count(),
            body.chars().count()
        );
        if let Err(e) = tauri_plugin_vcp_mobile::system::show_system_notification(
            app_handle.clone(),
            title,
            body,
        ) {
            log::warn!("[VCPLog] Failed to push agent_message notification: {}", e);
        } else {
            log::info!("[VCPLog] Android notification request submitted for AgentMessage.");
        }
    } else if payload_has_agent_message_hint(payload, 0) {
        if agent_message_was_delivered_locally(payload) {
            log::info!(
                "[VCPLog] AgentMessage notification already delivered locally; skipping duplicate Android notification."
            );
            return;
        }
        log::warn!(
            "[VCPLog] AgentMessage hint found but no notification body extracted. summary={}",
            summarize_agent_message_payload(payload)
        );
    }
}

fn extract_agent_message_notification(payload: &Value) -> Option<(String, String)> {
    let agent_data = find_agent_message_payload(payload, 0)
        .or_else(|| find_agent_message_tool_payload(payload, 0))?;

    if agent_data
        .get("androidNotification")
        .and_then(|v| v.get("delivered"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return None;
    }

    if agent_data.get("type").and_then(Value::as_str) != Some("agent_message") {
        return None;
    }

    let body = get_non_empty_string(&agent_data, "originalContent")
        .or_else(|| get_non_empty_string(&agent_data, "message"))?;
    let title = get_non_empty_string(&agent_data, "title")
        .or_else(|| {
            get_non_empty_string(&agent_data, "recipient").map(|name| format!("{} 的消息", name))
        })
        .unwrap_or_else(|| "Agent 消息".to_string());

    Some((title, body))
}

fn find_agent_message_tool_payload(value: &Value, depth: usize) -> Option<Value> {
    if depth > 6 {
        return None;
    }

    if let Some(text) = value.as_str() {
        let parsed = serde_json::from_str::<Value>(text).ok()?;
        return find_agent_message_payload(&parsed, depth + 1)
            .or_else(|| find_agent_message_tool_payload(&parsed, depth + 1));
    }

    let object = value.as_object()?;

    if is_agent_message_tool_object(object) {
        if let Some(agent_payload) = build_agent_message_payload_from_tool_object(value, depth) {
            return Some(agent_payload);
        }
    }

    for key in [
        "data",
        "callbackData",
        "result",
        "payload",
        "message",
        "content",
        "original_plugin_output",
        "raw",
        "details",
    ] {
        if let Some(nested) = object.get(key) {
            if let Some(found) = find_agent_message_tool_payload(nested, depth + 1) {
                return Some(found);
            }
        }
    }

    None
}

fn build_agent_message_payload_from_tool_object(value: &Value, depth: usize) -> Option<Value> {
    let object = value.as_object()?;

    for key in [
        "result",
        "payload",
        "original_plugin_output",
        "content",
        "message",
        "body",
        "data",
    ] {
        if let Some(nested) = object.get(key) {
            if let Some(found) = find_agent_message_payload(nested, depth + 1) {
                return Some(found);
            }
        }
    }

    let body = first_tool_body_string(value, depth + 1)?;
    let title = get_non_empty_string(value, "title")
        .or_else(|| sender_name_from_tool_object(value).map(|name| format!("{} 的消息", name)))
        .unwrap_or_else(|| "Agent 消息".to_string());

    Some(serde_json::json!({
        "type": "agent_message",
        "title": title,
        "message": body,
        "originalContent": body,
        "recipient": sender_name_from_tool_object(value),
        "androidNotification": value.get("androidNotification").cloned().unwrap_or(Value::Null)
    }))
}

fn first_tool_body_string(value: &Value, depth: usize) -> Option<String> {
    if depth > 7 {
        return None;
    }

    if let Some(text) = value.as_str() {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return None;
        }
        if let Ok(parsed) = serde_json::from_str::<Value>(trimmed) {
            if let Some((_, body)) = extract_agent_message_notification(&parsed) {
                return Some(body);
            }
            if let Some(text) = first_tool_body_string(&parsed, depth + 1) {
                return Some(text);
            }
        }
        return Some(trimmed.to_string());
    }

    let object = value.as_object()?;
    for key in [
        "originalContent",
        "body",
        "message",
        "content",
        "result",
        "original_plugin_output",
    ] {
        if let Some(nested) = object.get(key) {
            if let Some(text) = first_tool_body_string(nested, depth + 1) {
                return Some(text);
            }
        }
    }

    None
}

fn find_agent_message_payload(value: &Value, depth: usize) -> Option<Value> {
    if depth > 6 {
        return None;
    }

    if let Some(text) = value.as_str() {
        let parsed = serde_json::from_str::<Value>(text).ok()?;
        return find_agent_message_payload(&parsed, depth + 1);
    }

    let object = value.as_object()?;
    if object.get("type").and_then(Value::as_str) == Some("agent_message") {
        return Some(value.clone());
    }

    // VCPToolBox may wrap plugin results as:
    // { type, data }, { callbackData }, or { status, result } depending on
    // whether the source is a local plugin callback or a distributed callback.
    for key in [
        "data",
        "callbackData",
        "result",
        "payload",
        "message",
        "content",
        "original_plugin_output",
    ] {
        if let Some(nested) = object.get(key) {
            if let Some(found) = find_agent_message_payload(nested, depth + 1) {
                return Some(found);
            }
        }
    }

    None
}

fn is_agent_message_tool_object(object: &serde_json::Map<String, Value>) -> bool {
    for key in [
        "tool_name",
        "toolName",
        "pluginName",
        "PLUGIN_NAME_FOR_CALLBACK",
        "name",
    ] {
        if object
            .get(key)
            .and_then(Value::as_str)
            .map(is_agent_message_tool_name)
            .unwrap_or(false)
        {
            return true;
        }
    }
    false
}

fn is_agent_message_tool_name(value: &str) -> bool {
    let normalized: String = value
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect();
    normalized == "agentmessage"
        || normalized == "mobileagentmessage"
        || normalized.starts_with("agentmessage")
}

fn sender_name_from_tool_object(value: &Value) -> Option<String> {
    for key in ["recipient", "Maid", "maid", "MaidName", "sender_name"] {
        if let Some(name) = get_non_empty_string(value, key) {
            return Some(name);
        }
    }
    None
}

fn get_non_empty_string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
}

fn agent_message_was_delivered_locally(payload: &Value) -> bool {
    find_agent_message_payload(payload, 0)
        .or_else(|| find_agent_message_tool_payload(payload, 0))
        .and_then(|agent_data| agent_data.get("androidNotification").cloned())
        .and_then(|notification| notification.get("delivered").cloned())
        .and_then(|delivered| delivered.as_bool())
        .unwrap_or(false)
}

fn payload_has_agent_message_hint(value: &Value, depth: usize) -> bool {
    if depth > 4 {
        return false;
    }

    if let Some(text) = value.as_str() {
        return text.contains("AgentMessage") || text.contains("agent_message");
    }

    let Some(object) = value.as_object() else {
        return false;
    };

    if object.get("type").and_then(Value::as_str) == Some("agent_message")
        || is_agent_message_tool_object(object)
    {
        return true;
    }

    for key in [
        "data",
        "callbackData",
        "result",
        "payload",
        "message",
        "content",
        "original_plugin_output",
        "raw",
        "details",
    ] {
        if object
            .get(key)
            .map(|nested| payload_has_agent_message_hint(nested, depth + 1))
            .unwrap_or(false)
        {
            return true;
        }
    }

    false
}

fn summarize_agent_message_payload(value: &Value) -> String {
    let root_type = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("<none>");
    let data = value.get("data").unwrap_or(value);
    let data_type = data.get("type").and_then(Value::as_str).unwrap_or("<none>");
    let tool_name = [
        "tool_name",
        "toolName",
        "pluginName",
        "PLUGIN_NAME_FOR_CALLBACK",
        "name",
    ]
    .iter()
    .find_map(|key| data.get(*key).and_then(Value::as_str))
    .unwrap_or("<none>");
    let status = data
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("<none>");
    let content_len = data
        .get("content")
        .and_then(Value::as_str)
        .map(|s| s.chars().count())
        .unwrap_or(0);

    format!(
        "root_type={}, data_type={}, tool={}, status={}, content_len={}",
        root_type, data_type, tool_name, status, content_len
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extracts_agent_message_from_vcp_log_payload() {
        let payload = json!({
            "type": "vcp_log",
            "data": {
                "type": "agent_message",
                "recipient": "小克",
                "message": "2026-06-06 18:00:00 - 小克\n格式化消息",
                "originalContent": "格式化消息"
            }
        });

        assert_eq!(
            extract_agent_message_notification(&payload),
            Some(("小克 的消息".to_string(), "格式化消息".to_string()))
        );
    }

    #[test]
    fn extracts_agent_message_from_callback_result_payload() {
        let payload = json!({
            "type": "plugin_callback_notification",
            "data": {
                "status": "success",
                "result": {
                    "type": "agent_message",
                    "recipient": "小克",
                    "message": "2026-06-06 18:00:00 - 小克\n格式化消息"
                }
            }
        });

        assert_eq!(
            extract_agent_message_notification(&payload),
            Some((
                "小克 的消息".to_string(),
                "2026-06-06 18:00:00 - 小克\n格式化消息".to_string()
            ))
        );
    }

    #[test]
    fn extracts_agent_message_from_callback_data_payload() {
        let payload = json!({
            "type": "vcp_log",
            "data": {
                "callbackData": {
                    "type": "agent_message",
                    "recipient": "小克",
                    "originalContent": "只推送原始内容"
                }
            }
        });

        assert_eq!(
            extract_agent_message_notification(&payload),
            Some(("小克 的消息".to_string(), "只推送原始内容".to_string()))
        );
    }

    #[test]
    fn extracts_agent_message_from_json_string_payload() {
        let payload = json!({
            "type": "vcp_log",
            "data": {
                "content": "{\"type\":\"agent_message\",\"recipient\":\"小克\",\"originalContent\":\"字符串包裹消息\"}"
            }
        });

        assert_eq!(
            extract_agent_message_notification(&payload),
            Some(("小克 的消息".to_string(), "字符串包裹消息".to_string()))
        );
    }

    #[test]
    fn skips_agent_message_already_delivered_locally() {
        let payload = json!({
            "type": "vcp_log",
            "data": {
                "type": "agent_message",
                "message": "已推送",
                "androidNotification": { "delivered": true }
            }
        });

        assert_eq!(extract_agent_message_notification(&payload), None);
    }

    #[test]
    fn extracts_agent_message_from_tool_executor_log_content() {
        let payload = json!({
            "type": "vcp_log",
            "data": {
                "tool_name": "AgentMessage",
                "status": "success",
                "content": "{\n  \"type\": \"agent_message\",\n  \"recipient\": \"小克\",\n  \"message\": \"2026-06-06 19:28:00 - 小克\\n提醒内容\",\n  \"originalContent\": \"提醒内容\"\n}"
            }
        });

        assert_eq!(
            extract_agent_message_notification(&payload),
            Some(("小克 的消息".to_string(), "提醒内容".to_string()))
        );
    }

    #[test]
    fn falls_back_to_agent_message_tool_content_text() {
        let payload = json!({
            "type": "vcp_log",
            "data": {
                "tool_name": "AgentMessage",
                "status": "success",
                "content": "提醒内容"
            }
        });

        assert_eq!(
            extract_agent_message_notification(&payload),
            Some(("Agent 消息".to_string(), "提醒内容".to_string()))
        );
    }

    #[test]
    fn extracts_agent_message_from_distributed_callback_tool_payload() {
        let payload = json!({
            "type": "plugin_callback_notification",
            "data": {
                "pluginName": "AgentMessage",
                "taskId": "abc",
                "result": {
                    "type": "agent_message",
                    "recipient": "小克",
                    "originalContent": "分布式回调内容"
                }
            }
        });

        assert_eq!(
            extract_agent_message_notification(&payload),
            Some(("小克 的消息".to_string(), "分布式回调内容".to_string()))
        );
    }
}

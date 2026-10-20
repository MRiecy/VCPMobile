use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use std::collections::VecDeque;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tokio::sync::{mpsc, watch};
use tokio::time::{sleep, Duration};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use url::Url;

type ConnectionIntent = (u64, Option<Url>);

static HEARTBEAT_INTERVAL_MS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(15000);
const SOCKET_WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_BACKGROUND_LOGS: usize = 256;
const MAX_BACKGROUND_LOG_BYTES: usize = 2 * 1024 * 1024;

struct LogSizeBudget(usize);

impl Write for LogSizeBudget {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.0 {
            return Err(std::io::Error::other("background log exceeds cache budget"));
        }
        self.0 -= bytes.len();
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[derive(Default)]
struct BackgroundLogBuffer {
    entries: VecDeque<(Value, usize)>,
    bytes: usize,
    dropped: usize,
}

impl BackgroundLogBuffer {
    fn push(&mut self, payload: Value) {
        // Count encoded bytes without allocating another copy of a large payload.
        let mut budget = LogSizeBudget(MAX_BACKGROUND_LOG_BYTES);
        if serde_json::to_writer(&mut budget, &payload).is_err() {
            self.dropped = self.dropped.saturating_add(1);
            return;
        }
        let size = MAX_BACKGROUND_LOG_BYTES - budget.0;
        while self.entries.len() >= MAX_BACKGROUND_LOGS
            || self.bytes + size > MAX_BACKGROUND_LOG_BYTES
        {
            if let Some((_, removed_size)) = self.entries.pop_front() {
                self.bytes -= removed_size;
                self.dropped = self.dropped.saturating_add(1);
            } else {
                break;
            }
        }
        self.bytes += size;
        self.entries.push_back((payload, size));
    }
}

lazy_static::lazy_static! {
    static ref LOG_CONNECTION_ACTIVE: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
    static ref LOG_SENDER: Arc<tokio::sync::Mutex<Option<mpsc::Sender<Value>>>> = Arc::new(tokio::sync::Mutex::new(None));
    // 关键修复：保持 Sender 和一个 Receiver 都在生命周期内，防止通道因无接收者而被视为关闭
    static ref WS_URL_CHANNEL: (watch::Sender<ConnectionIntent>, watch::Receiver<ConnectionIntent>) = watch::channel((0, None));
    static ref CURRENT_LOG_STATUS: Arc<tokio::sync::RwLock<String>> = Arc::new(tokio::sync::RwLock::new("closed".to_string()));
    static ref HEARTBEAT_RESET_TX: Arc<tokio::sync::Mutex<Option<mpsc::Sender<()>>>> = Arc::new(tokio::sync::Mutex::new(None));
    // 缓存 App 在后台期间接收到的 VCPLog 消息，避免丢弃和 WebView 积压，待返回前台时一并冲刷
    static ref BACKGROUND_LOG_CACHE: std::sync::Mutex<BackgroundLogBuffer> = std::sync::Mutex::new(BackgroundLogBuffer::default());
}

fn set_log_url(url: Option<Url>) {
    WS_URL_CHANNEL.0.send_if_modified(|current| {
        if current.1 == url {
            return false;
        }
        current.0 = current.0.wrapping_add(1);
        current.1 = url;
        true
    });
}

pub async fn handle_foreground_state_change(_app: &AppHandle, is_foreground: bool) {
    // 自动根据前后台状态调整并重置心跳
    let heartbeat_ms = if is_foreground { 15000 } else { 120000 };
    HEARTBEAT_INTERVAL_MS.store(heartbeat_ms, Ordering::SeqCst);
    {
        let tx_lock = HEARTBEAT_RESET_TX.lock().await;
        if let Some(tx) = tx_lock.as_ref() {
            let _ = tx.try_send(());
        }
    }
}

pub async fn disconnect_log_connections(app: &AppHandle) {
    let _ = init_vcp_log_connection_internal(app.clone(), "".to_string(), "".to_string()).await;
    let _ = crate::vcp_modules::vcp_info_service::init_vcp_info_connection_internal(
        app.clone(),
        "".to_string(),
        "".to_string(),
    )
    .await;
}

pub async fn reconnect_log_connections(app: &AppHandle, log_url: String, log_key: String) {
    let _ = init_vcp_log_connection_internal(app.clone(), log_url.clone(), log_key.clone()).await;
    let _ = crate::vcp_modules::vcp_info_service::init_vcp_info_connection_internal(
        app.clone(),
        log_url,
        log_key,
    )
    .await;
}

fn emit_log_event<R: tauri::Runtime>(app: &AppHandle<R>, payload: serde_json::Value) {
    if !crate::vcp_modules::infra::lifecycle_manager::is_app_in_foreground(app) {
        // App 处于后台时，不直接发射到 WebView，而是缓存在 Rust 侧，防止内存泄漏，并在返回前台时补发
        if let Ok(mut cache) = BACKGROUND_LOG_CACHE.lock() {
            cache.push(payload);
        }
        return;
    }
    let _ = app.emit("vcp-system-event", payload);
}

pub fn flush_background_logs<R: tauri::Runtime>(app: &AppHandle<R>) {
    let logs = {
        if let Ok(mut cache) = BACKGROUND_LOG_CACHE.lock() {
            std::mem::take(&mut *cache)
        } else {
            BackgroundLogBuffer::default()
        }
    };
    if logs.dropped > 0 {
        log::warn!(
            "[VCPLog] Background UI cache discarded {} older/oversized entries",
            logs.dropped
        );
    }
    if !logs.entries.is_empty() {
        log::info!(
            "[VCPLog] Flashing {} cached background logs to WebView.",
            logs.entries.len()
        );
        for (log, _) in logs.entries {
            let _ = app.emit("vcp-system-event", log);
        }
    }
}

#[tauri::command]
pub async fn set_vcp_log_heartbeat(interval_ms: u64) -> Result<(), String> {
    HEARTBEAT_INTERVAL_MS.store(interval_ms, Ordering::SeqCst);
    let tx_lock = HEARTBEAT_RESET_TX.lock().await;
    if let Some(tx) = tx_lock.as_ref() {
        let _ = tx.try_send(());
    }
    Ok(())
}

pub async fn get_vcp_log_status_internal() -> String {
    CURRENT_LOG_STATUS.read().await.clone()
}

#[tauri::command]
pub async fn send_vcp_log_message(payload: serde_json::Value) -> Result<(), String> {
    if CURRENT_LOG_STATUS.read().await.as_str() != "connected" {
        return Err("VCPLog connection is not active".to_string());
    }
    let sender_lock = LOG_SENDER.lock().await;
    if let Some(sender) = sender_lock.as_ref() {
        sender
            .try_send(payload)
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

fn append_device_name(url: &Url, device_name: &str) -> Url {
    let mut new_url = url.clone();
    let query = new_url.query();

    let has_device_name = query.is_some_and(|q| q.contains("deviceName="));
    if !has_device_name {
        let encoded_device_name = urlencoding::encode(device_name);
        let new_query = match query {
            Some(q) if !q.is_empty() => format!("{}&deviceName={}", q, encoded_device_name),
            _ => format!("deviceName={}", encoded_device_name),
        };
        new_url.set_query(Some(&new_query));
    }
    new_url
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
    #[cfg(target_os = "ios")]
    if !crate::vcp_modules::lifecycle_manager::is_app_in_foreground(&app) {
        set_log_url(None);
        return Ok(());
    }
    // 如果 URL 或 Key 为空，发送 None 以停止现有连接并进入静默等待
    if url.trim().is_empty() || key.trim().is_empty() {
        set_log_url(None);
        return Ok(());
    }

    let ws_url = parse_log_url(&url, &key)?;

    // Always send the new URL to the watch channel
    set_log_url(Some(ws_url));

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

    let mut retry_delay = Duration::from_millis(1000);
    loop {
        // 获取当前 URL
        let (connection_revision, ws_url) = {
            let val = url_rx.borrow().clone();
            match val {
                (revision, Some(url)) => (revision, url),
                (_, None) => {
                    *CURRENT_LOG_STATUS.write().await = "closed".to_string();
                    if url_rx.changed().await.is_err() {
                        break;
                    }
                    continue;
                }
            }
        };

        log::info!("[VCPLog] Attempting to connect to {}...", ws_url);

        {
            *CURRENT_LOG_STATUS.write().await = "connecting".to_string();
        }

        emit_log_event(
            &app_handle,
            serde_json::json!({
                "type": "vcp-log-status",
                "status": "connecting",
                "message": "连接中...",
                "source": "VCPLog"
            }),
        );

        let new_url = append_device_name(&ws_url, "VCPChat-Mobile");
        let old_url = ws_url.clone();

        let urls_to_try = if new_url == old_url {
            vec![new_url]
        } else {
            vec![new_url, old_url]
        };

        let mut connection_succeeded = false;
        let mut ws_stream_opt = None;
        let mut final_ws_url = ws_url.clone();
        let mut connection_error = None;
        let mut interrupted_by_url_change = false;

        for (i, trial_url) in urls_to_try.iter().enumerate() {
            log::info!(
                "[VCPLog] Attempting connection trial {}/{}: {}...",
                i + 1,
                urls_to_try.len(),
                trial_url
            );

            let mut request = match trial_url.as_str().into_client_request() {
                Ok(req) => req,
                Err(e) => {
                    log::error!(
                        "[VCPLog] Failed to build request for trial {} ({}): {}",
                        i + 1,
                        trial_url,
                        e
                    );
                    connection_error = Some(tokio_tungstenite::tungstenite::Error::Io(
                        std::io::Error::new(std::io::ErrorKind::InvalidInput, e.to_string()),
                    ));
                    continue;
                }
            };

            if let Some(host) = trial_url.host_str() {
                let host_with_port = if let Some(port) = trial_url.port() {
                    format!("{}:{}", host, port)
                } else {
                    host.to_string()
                };
                if let Ok(val) = host_with_port.parse() {
                    request.headers_mut().insert("Host", val);
                }

                let origin_scheme = match trial_url.scheme() {
                    "wss" => "https",
                    _ => "http",
                };
                let origin = if let Some(port) = trial_url.port() {
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

            let connect_fut = connect_async(request);

            tokio::select! {
                biased;
                _ = url_rx.changed() => {
                    log::info!("[VCPLog] URL changed during connection trial, aborting.");
                    interrupted_by_url_change = true;
                    break;
                }
                res = tokio::time::timeout(Duration::from_secs(5), connect_fut) => {
                    match res {
                        Ok(Ok((ws_stream, _))) => {
                            ws_stream_opt = Some(ws_stream);
                            final_ws_url = trial_url.clone();
                            connection_succeeded = true;
                            break;
                        }
                        Ok(Err(e)) => {
                            log::warn!(
                                "[VCPLog] Connection failed for trial {} ({}): {}",
                                i + 1,
                                trial_url,
                                e
                            );
                            connection_error = Some(e);
                        }
                        Err(_) => {
                            log::warn!(
                                "[VCPLog] Connection timed out (5s) for trial {} ({})",
                                i + 1,
                                trial_url
                            );
                            connection_error = Some(tokio_tungstenite::tungstenite::Error::Io(
                                std::io::Error::new(std::io::ErrorKind::TimedOut, "Connection timed out (5s)")
                            ));
                        }
                    }
                }
            }

            if interrupted_by_url_change {
                break;
            }

            // 关键优化：检查错误类型是否值得降级尝试。
            // 只有当收到 HTTP 握手响应错误（如 404/400，表示 TCP 连通但握手协议细节被拒）时，才尝试 old_url 降级。
            // 底层网络错误（如连接被拒、超时等），降级尝试毫无意义，直接提前终止。
            if i + 1 < urls_to_try.len() {
                let should_fallback = matches!(
                    &connection_error,
                    Some(tokio_tungstenite::tungstenite::Error::Http(_))
                );
                if !should_fallback {
                    log::info!(
                        "[VCPLog] Underlying network error or timeout during trial {}. Skipping further fallback trials.",
                        i + 1
                    );
                    break;
                }
            }
        }

        if interrupted_by_url_change {
            continue;
        }

        if connection_succeeded {
            if let Some(ws_stream) = ws_stream_opt {
                if url_rx.borrow().0 != connection_revision {
                    continue;
                }
                retry_delay = Duration::from_millis(1000);
                // Outbound messages belong to this socket, not the persistent
                // reconnect loop. Dropping it also drops unsent old-config data.
                let (tx, mut rx) = mpsc::channel::<Value>(64);
                *LOG_SENDER.lock().await = Some(tx);
                {
                    *CURRENT_LOG_STATUS.write().await = "connected".to_string();
                }

                log::info!("[VCPLog] Connected successfully to {}", final_ws_url);

                let (mut ws_write, mut ws_read) = ws_stream.split();

                emit_log_event(
                    &app_handle,
                    serde_json::json!({
                        "type": "vcp-log-status",
                        "status": "connected",
                        "message": "已连接",
                        "source": "VCPLog"
                    }),
                );

                // 额外发送一条连接成功的通知卡片
                emit_log_event(
                    &app_handle,
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

                let mut configuration_changed = false;
                loop {
                    tokio::select! {
                        biased;
                        // 监听 URL 变更并防止 Flapping 瞬断
                        changed = url_rx.changed() => {
                            if changed.is_err() || url_rx.borrow().0 != connection_revision {
                                // A fast stop/resume of the same URL is still a
                                // new connection intent even if watch coalesces it.
                                log::info!("[VCPLog] Connection intent changed, closing socket.");
                                configuration_changed = true;
                                break;
                            }
                        }
                        // 监听心跳重置信号
                        Some(_) = reset_rx.recv() => {
                            let current_ms = HEARTBEAT_INTERVAL_MS.load(Ordering::SeqCst);
                            log::info!("[VCPLog] Heartbeat interval updated to {}ms, resetting timer.", current_ms);
                            heartbeat_timer.as_mut().reset(tokio::time::Instant::now() + Duration::from_millis(current_ms));
                        }
                        // 心跳周期触发
                        _ = &mut heartbeat_timer => {
                            if let Err(e) = tokio::time::timeout(SOCKET_WRITE_TIMEOUT, ws_write.send(Message::Ping(vec![].into())))
                                .await.map_err(|_| "WebSocket ping timed out".to_string())
                                .and_then(|result| result.map_err(|error| error.to_string())) {
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
                                                emit_log_event(&app_handle, payload);
                                            }
                                            Err(_) => {
                                                emit_log_event(&app_handle, serde_json::json!({
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
                                    if let Err(e) = tokio::time::timeout(SOCKET_WRITE_TIMEOUT, ws_write.send(Message::Text(text.into())))
                                        .await.map_err(|_| "WebSocket send timed out".to_string())
                                        .and_then(|result| result.map_err(|error| error.to_string())) {
                                        log::error!("[VCPLog] Failed to send message: {}", e);
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }

                {
                    *LOG_SENDER.lock().await = None;
                    let mut tx_lock = HEARTBEAT_RESET_TX.lock().await;
                    *tx_lock = None;
                }

                log::info!("[VCPLog] Disconnected from {}.", final_ws_url);
                {
                    *CURRENT_LOG_STATUS.write().await = "closed".to_string();
                }
                emit_log_event(
                    &app_handle,
                    serde_json::json!({
                        "type": "vcp-log-status",
                        "status": "closed",
                        "message": "连接已断开",
                        "source": "VCPLog"
                    }),
                );
                if configuration_changed {
                    continue;
                }
            }
        } else {
            {
                *CURRENT_LOG_STATUS.write().await = "error".to_string();
            }
            let last_error = connection_error
                .map(|e| e.to_string())
                .unwrap_or_else(|| "Unknown connection error".to_string());
            log::error!(
                "[VCPLog] All connection attempts failed. Last error: {}. Retrying...",
                last_error
            );
            emit_log_event(
                &app_handle,
                serde_json::json!({
                    "type": "vcp-log-status",
                    "status": "error",
                    "message": "连接错误",
                    "source": "VCPLog"
                }),
            );

            // 额外发送一条连接错误的通知卡片，辅助排查
            emit_log_event(
                &app_handle,
                serde_json::json!({
                    "type": "vcp-log-message",
                    "data": {
                        "id": "vcp_log_connection_status",
                        "status": "error",
                        "tool_name": "VCPLog 连接失败",
                        "content": format!(
                            "❌ 连接失败: {}\n\n提示：\n1. 请检查桌面端 VCP 是否已开启且 VCPLog 服务正常。\n2. 检查 VCP API 地址和 Key 配置是否正确。",
                            last_error
                        ),
                        "source": "VCPLog"
                    }
                }),
            );
        }

        tokio::select! {
            _ = url_rx.changed() => log::info!("[VCPLog] URL changed during retry wait."),
            _ = sleep(retry_delay) => {},
        }
        retry_delay = (retry_delay * 2).min(Duration::from_secs(60));
    }
    {
        let mut sender_lock = LOG_SENDER.lock().await;
        *sender_lock = None;
    }
    LOG_CONNECTION_ACTIVE.store(false, Ordering::SeqCst);
    log::info!("[VCPLog] Listener task terminated, connection flag and sender reset.");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn background_log_cache_discards_old_entries_and_rejects_an_oversized_payload() {
        let mut cache = BackgroundLogBuffer::default();
        for index in 0..MAX_BACKGROUND_LOGS + 3 {
            cache.push(serde_json::json!({ "index": index }));
        }
        assert_eq!(
            cache
                .entries
                .front()
                .map(|(value, _)| value["index"].as_u64()),
            Some(Some(3))
        );
        assert_eq!(cache.entries.len(), MAX_BACKGROUND_LOGS);
        let old_bytes = cache.bytes;
        cache.push(serde_json::json!({ "data": "x".repeat(MAX_BACKGROUND_LOG_BYTES) }));
        assert_eq!(cache.bytes, old_bytes);
        assert_eq!(cache.dropped, 4);
    }

    #[test]
    fn background_log_byte_budget_eviction_is_independent_of_entry_count() {
        let mut cache = BackgroundLogBuffer::default();
        let payload = serde_json::json!({ "data": "x".repeat(MAX_BACKGROUND_LOG_BYTES / 2) });
        cache.push(payload.clone());
        cache.push(payload);
        assert_eq!(cache.entries.len(), 1);
        assert!(cache.bytes <= MAX_BACKGROUND_LOG_BYTES);
        assert_eq!(cache.dropped, 1);
    }
}

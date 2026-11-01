use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use std::collections::VecDeque;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tokio::sync::{mpsc, watch};
use tokio::time::Duration;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use url::Url;

use super::vcp_ws_activity::{self, Heartbeat, HeartbeatAction, Retry, SocketEvent};
use super::vcp_ws_endpoint::{compile_log_connection, endpoint_log_label, log_connection_attempts};

type ConnectionIntent = (u64, Option<Url>);

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

pub async fn reconnect_log_connections<R: tauri::Runtime>(
    app: &AppHandle<R>,
    log_url: String,
    log_key: String,
) -> Result<(), String> {
    // Prepare both channels before publishing either intent. Settings validation,
    // startup and lifecycle restoration use the same endpoint compiler.
    match compile_log_connection(&log_url, &log_key) {
        Ok(endpoints) => {
            let (log, info) = endpoints
                .map(|endpoints| (Some(endpoints.log), Some(endpoints.info)))
                .unwrap_or((None, None));
            apply_log_endpoint(app, log);
            super::vcp_info_service::apply_info_endpoint(app, info);
            Ok(())
        }
        Err(issue) => {
            apply_log_endpoint(app, None);
            super::vcp_info_service::apply_info_endpoint(app, None);
            let message = issue.to_string();
            report_connection_configuration_error(app, "VCPLog / VCPInfo", &message);
            Err(message)
        }
    }
}

pub(super) fn report_connection_configuration_error<R: tauri::Runtime>(
    app: &AppHandle<R>,
    source: &str,
    message: &str,
) {
    // Keep the stopped state as `closed`. A separate notification retains the
    // cause when the listener acknowledges None, and cannot overwrite a later
    // connection's status. There is no await between stopping and reporting.
    emit_log_event(
        app,
        serde_json::json!({
            "type": "vcp-log-message",
            "data": {
                "id": "vcp_connection_configuration_error",
                "status": "error",
                "tool_name": format!("{source} 配置错误"),
                "content": format!("连接已停止：{message} 请在设置中修正 WebSocket 地址或 Key。"),
                "source": source
            }
        }),
    );
}

fn emit_log_event<R: tauri::Runtime>(app: &AppHandle<R>, payload: serde_json::Value) {
    use super::lifecycle_controller::is_app_in_foreground;

    if !is_app_in_foreground(app) {
        let Ok(mut cache) = BACKGROUND_LOG_CACHE.lock() else {
            return;
        };
        // A resume may have flushed the cache while this producer waited for it.
        if !is_app_in_foreground(app) {
            // The foreground snapshot owns status. Replaying an earlier socket's
            // status could overwrite the current socket's live status.
            if payload["type"] != "vcp-log-status" {
                cache.push(payload);
            }
            return;
        }
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
    match compile_log_connection(&url, &key) {
        Ok(endpoints) => {
            apply_log_endpoint(&app, endpoints.map(|endpoints| endpoints.log));
            Ok(())
        }
        Err(issue) => {
            apply_log_endpoint(&app, None);
            let message = issue.to_string();
            report_connection_configuration_error(&app, "VCPLog", &message);
            Err(message)
        }
    }
}

fn apply_log_endpoint<R: tauri::Runtime>(app: &AppHandle<R>, endpoint: Option<Url>) {
    let configured = endpoint.is_some();
    set_log_url(endpoint);
    if !configured {
        return;
    }

    if LOG_CONNECTION_ACTIVE.swap(true, Ordering::SeqCst) {
        return;
    }

    let h = app.clone();
    tauri::async_runtime::spawn(async move {
        start_vcp_log_listener(h).await;
    });
}

async fn start_vcp_log_listener<R: tauri::Runtime>(app_handle: AppHandle<R>) {
    let mut url_rx = WS_URL_CHANNEL.0.subscribe();
    let mut activity_rx = vcp_ws_activity::subscribe(&app_handle);

    let mut retry = Retry::default();
    'listener: loop {
        let activity = *activity_rx.borrow_and_update();
        let (connection_revision, ws_url) = {
            let val = url_rx.borrow_and_update().clone();
            match val {
                (revision, Some(url)) if activity.maintain() => (revision, url),
                _ => {
                    let status_changed = {
                        let mut status = CURRENT_LOG_STATUS.write().await;
                        let changed = status.as_str() != "closed";
                        *status = "closed".to_string();
                        changed
                    };
                    if status_changed {
                        emit_log_event(
                            &app_handle,
                            serde_json::json!({
                                "type": "vcp-log-status",
                                "status": "closed",
                                "message": "连接已停止",
                                "source": "VCPLog"
                            }),
                        );
                    }
                    retry = Retry::default();
                    let wake = tokio::select! {
                        changed = url_rx.changed() => changed,
                        changed = activity_rx.changed() => changed,
                    };
                    if wake.is_err() {
                        break;
                    }
                    continue;
                }
            }
        };

        log::info!(
            "[VCPLog] Attempting to connect to {}...",
            endpoint_log_label(&ws_url)
        );

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

        let urls_to_try = log_connection_attempts(&ws_url);

        let mut connection_succeeded = false;
        let mut ws_stream_opt = None;
        let mut final_ws_url = ws_url.clone();
        let mut connection_error = None;

        for (i, trial_url) in urls_to_try.iter().enumerate() {
            log::info!(
                "[VCPLog] Attempting connection trial {}/{}: {}...",
                i + 1,
                urls_to_try.len(),
                endpoint_log_label(trial_url)
            );

            let mut request = match trial_url.as_str().into_client_request() {
                Ok(req) => req,
                Err(e) => {
                    log::error!(
                        "[VCPLog] Failed to build request for trial {} ({}): {}",
                        i + 1,
                        endpoint_log_label(trial_url),
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
                    retry = Retry::default();
                    continue 'listener;
                }
                // Resuming an in-flight handshake reuses it; only idle parks dialing.
                _ = vcp_ws_activity::until_idle(&mut activity_rx) => continue 'listener,
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
                                endpoint_log_label(trial_url),
                                e
                            );
                            connection_error = Some(e);
                        }
                        Err(_) => {
                            log::warn!(
                                "[VCPLog] Connection timed out (5s) for trial {} ({})",
                                i + 1,
                                endpoint_log_label(trial_url)
                            );
                            connection_error = Some(tokio_tungstenite::tungstenite::Error::Io(
                                std::io::Error::new(std::io::ErrorKind::TimedOut, "Connection timed out (5s)")
                            ));
                        }
                    }
                }
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

        if connection_succeeded {
            if let Some(ws_stream) = ws_stream_opt {
                if url_rx.borrow().0 != connection_revision {
                    continue;
                }
                // Outbound messages belong to this socket, not the persistent
                // reconnect loop. Dropping it also drops unsent old-config data.
                let (tx, mut rx) = mpsc::channel::<Value>(64);
                *LOG_SENDER.lock().await = Some(tx);
                {
                    *CURRENT_LOG_STATUS.write().await = "connected".to_string();
                }

                log::info!(
                    "[VCPLog] Connected successfully to {}",
                    endpoint_log_label(&final_ws_url)
                );

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

                let current = *activity_rx.borrow_and_update();
                if current.probe_epoch != activity.probe_epoch {
                    retry = Retry::default();
                }
                let mut heartbeat = Heartbeat::new(current);
                let mut intent_changed = false;
                loop {
                    tokio::select! {
                        biased;
                        // 监听 URL 变更并防止 Flapping 瞬断
                        changed = url_rx.changed() => {
                            if changed.is_err() || url_rx.borrow().0 != connection_revision {
                                // A fast stop/resume of the same URL is still a
                                // new connection intent even if watch coalesces it.
                                log::info!("[VCPLog] Connection intent changed, closing socket.");
                                intent_changed = true;
                                break;
                            }
                        }
                        _ = activity_rx.changed() => {
                            if heartbeat.update(*activity_rx.borrow_and_update()) {
                                retry = Retry::default();
                            }
                        }
                        event = heartbeat.next(&mut ws_read) => match event {
                            SocketEvent::Timer(HeartbeatAction::ProbeExpired) => {
                                log::warn!("[VCPLog] No matching Pong before deadline.");
                                break;
                            }
                            SocketEvent::Timer(HeartbeatAction::Ping(payload)) => {
                                if let Err(e) = heartbeat.send_ping(&mut ws_write, payload).await {
                                    log::error!("[VCPLog] Failed to send Ping: {}", e);
                                    break;
                                }
                            }
                            SocketEvent::Message(msg_result) => match msg_result {
                                Some(Ok(Message::Pong(payload))) => heartbeat.pong(&payload),
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
                        },
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

                *LOG_SENDER.lock().await = None;

                log::info!(
                    "[VCPLog] Disconnected from {}.",
                    endpoint_log_label(&final_ws_url)
                );
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
                if intent_changed {
                    retry = Retry::default();
                    continue;
                }
                if retry.disconnected(heartbeat.confirmed()) {
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

        if !connection_succeeded && activity_rx.borrow().probe_epoch != activity.probe_epoch {
            // A path/foreground hint reused this dial. If it failed, try the new path once.
            retry = Retry::default();
            continue;
        }
        retry.wait(&mut url_rx, &mut activity_rx).await;
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

    #[tokio::test]
    async fn both_channels_follow_activity_and_configuration_changes() {
        use super::super::{lifecycle_state::LifecycleState, vcp_ws_activity::ChatActivityGuard};
        use crate::vcp_modules::{
            agent_chat_application_service::PreparedAgentRequest,
            topic_types::TopicKey,
            vcp_client::{ActiveGroupTurns, ActiveRequests},
        };
        use tauri::{Listener, Manager};
        use tokio::io::AsyncReadExt;
        use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};

        // This is the only test that starts these process-wide listeners. Keep
        // all transitions in one test and leave both intents stopped at the end.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("ws://{}", listener.local_addr().unwrap());
        let (socket_tx, mut socket_rx) = mpsc::channel(8);
        let (ping_tx, mut ping_rx) = mpsc::channel(8);
        #[derive(Clone, Copy, Debug)]
        enum PeerCommand {
            Disconnect,
            Ping,
            StallInfo,
        }
        let (commands, _) = tokio::sync::broadcast::channel(2);
        let (pong_tx, mut pong_rx) = mpsc::channel(2);
        let (stalled_tx, mut stalled_rx) = mpsc::channel(1);
        let peer_control = commands.clone();
        let server = tokio::spawn(async move {
            let mut sockets = tokio::task::JoinSet::new();
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let events = socket_tx.clone();
                let pings = ping_tx.clone();
                let pongs = pong_tx.clone();
                let stalled = stalled_tx.clone();
                let mut commands = peer_control.subscribe();
                sockets.spawn(async move {
                    let mut path = String::new();
                    #[expect(
                        clippy::result_large_err,
                        reason = "Tungstenite's handshake callback fixes the HTTP error type"
                    )]
                    let mut socket = tokio_tungstenite::accept_hdr_async(
                        stream,
                        |request: &Request, response: Response| {
                            path = request.uri().path().to_owned();
                            Ok(response)
                        },
                    )
                    .await
                    .unwrap();
                    events.send((path.clone(), true)).await.unwrap();
                    loop {
                        tokio::select! {
                            command = commands.recv() => match command.unwrap() {
                                PeerCommand::Disconnect => break,
                                PeerCommand::Ping => {
                                    socket.send(Message::Ping(b"peer".to_vec().into())).await.unwrap();
                                }
                                PeerCommand::StallInfo => {
                                    if path == "/vcpinfo/VCP_Key=runtime-test" {
                                        // Keep the TCP connection open without reading any WS
                                        // frames. Tungstenite cannot auto-Pong until it reads.
                                        let (release, retired) = tokio::sync::oneshot::channel();
                                        stalled.send(release).await.unwrap();
                                        let _ = retired.await;
                                        break;
                                    }
                                }
                            },
                            message = socket.next() => match message {
                                Some(Ok(Message::Ping(_))) => {
                                    socket.flush().await.unwrap(); // deliver Tungstenite's automatic Pong
                                    pings.send(path.clone()).await.unwrap();
                                }
                                Some(Ok(Message::Pong(payload))) if payload.as_ref() == b"peer" => {
                                    pongs.send(()).await.unwrap();
                                }
                                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                                _ => {}
                            }
                        }
                    }
                    events.send((path, false)).await.unwrap();
                });
            }
        });

        let app = tauri::test::mock_app();
        app.manage(LifecycleState::new());
        app.manage(ActiveRequests::default());
        app.manage(ActiveGroupTurns::new(&app.state::<LifecycleState>()));
        let set_foreground = |foreground| {
            app.state::<LifecycleState>()
                .ws_activity
                .set_foreground(foreground);
        };
        let (notification_tx, mut notification_rx) = mpsc::channel(4);
        let (status_tx, mut status_rx) = mpsc::channel(16);
        let log_status_tx = status_tx.clone();
        let event_id = app.listen("vcp-system-event", move |event| {
            let payload: Value = serde_json::from_str(event.payload()).unwrap();
            if payload["data"]["id"] == "vcp_connection_configuration_error" {
                notification_tx.try_send(payload).unwrap();
            } else if payload["type"] == "vcp-log-status" && payload["status"] == "closed" {
                log_status_tx.try_send("log").unwrap();
            }
        });
        let info_event_id = app.listen("vcp-info-event", move |event| {
            let payload: Value = serde_json::from_str(event.payload()).unwrap();
            if payload["type"] == "vcp-info-status" && payload["status"] == "closed" {
                status_tx.try_send("info").unwrap();
            }
        });

        async fn next_socket(receiver: &mut mpsc::Receiver<(String, bool)>) -> (String, bool) {
            tokio::time::timeout(Duration::from_secs(5), receiver.recv())
                .await
                .unwrap()
                .unwrap()
        }

        async fn expect_pair(receiver: &mut mpsc::Receiver<(String, bool)>, connected: bool) {
            let mut paths = Vec::new();
            for _ in 0..2 {
                let (path, state) = next_socket(receiver).await;
                assert_eq!(state, connected);
                paths.push(path);
            }
            paths.sort();
            assert_eq!(
                paths,
                [
                    "/VCPlog/VCP_Key=runtime-test",
                    "/vcpinfo/VCP_Key=runtime-test"
                ]
            );
        }

        async fn expect_status(log: &str, info: &str) {
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    if get_vcp_log_status_internal().await == log
                        && super::super::vcp_info_service::get_vcp_info_connection_status()
                            .await
                            .unwrap()
                            == info
                    {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
        }

        // Configuration remains effective while parked. Neither socket dials
        // until a foreground transition or an admitted chat wakes it.
        set_foreground(false);
        reconnect_log_connections(app.handle(), address.clone(), "runtime-test".into())
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(200), socket_rx.recv())
                .await
                .is_err()
        );
        let activity = vcp_ws_activity::subscribe(app.handle());
        let first_chat = PreparedAgentRequest::acquire(
            app.handle(),
            &app.state::<ActiveRequests>(),
            "agent",
            "a",
            "user",
            None,
            tokio_util::sync::CancellationToken::new(),
        )
        .unwrap();
        assert!(
            activity.borrow().maintain(),
            "agent preparation keeps chat active"
        );
        let turns = app.state::<ActiveGroupTurns>();
        let topic = TopicKey::new("group", "g", "b");
        let last_chat = turns.try_acquire(topic.clone()).unwrap();
        assert!(turns.try_acquire(topic).is_err());
        last_chat.start_session("continue", true).unwrap();
        expect_pair(&mut socket_rx, true).await;
        expect_status("connected", "connected").await;

        // The remaining chat keeps both reconnect loops running.
        drop(first_chat);
        assert!(
            activity.borrow().maintain(),
            "JEV owns its decision and speaker gaps"
        );
        commands.send(PeerCommand::Disconnect).unwrap();
        expect_pair(&mut socket_rx, false).await;
        expect_pair(&mut socket_rx, true).await;
        expect_status("connected", "connected").await;

        // Last chat finishes in background. Resume must probe the surviving
        // sockets in place, well before the normal 15-second heartbeat.
        drop(last_chat);
        assert!(
            !activity.borrow().maintain(),
            "the final owner returned to background idle"
        );
        commands.send(PeerCommand::Ping).unwrap();
        for _ in 0..2 {
            tokio::time::timeout(Duration::from_secs(3), pong_rx.recv())
                .await
                .unwrap()
                .unwrap();
        }
        set_foreground(true);
        let mut probed = Vec::new();
        for _ in 0..2 {
            probed.push(
                tokio::time::timeout(Duration::from_secs(3), ping_rx.recv())
                    .await
                    .unwrap()
                    .unwrap(),
            );
        }
        probed.sort();
        assert_eq!(
            probed,
            [
                "/VCPlog/VCP_Key=runtime-test",
                "/vcpinfo/VCP_Key=runtime-test"
            ]
        );
        assert!(
            socket_rx.try_recv().is_err(),
            "resume replaced a healthy socket"
        );

        // Only Info goes half open. Log answers the new probe on the original
        // connection; the client must replace Info even though TCP never closed.
        set_foreground(false);
        commands.send(PeerCommand::StallInfo).unwrap();
        let release_stalled = tokio::time::timeout(Duration::from_secs(3), stalled_rx.recv())
            .await
            .unwrap()
            .unwrap();
        let resume_started = tokio::time::Instant::now();
        set_foreground(true);
        let healthy_ping = tokio::time::timeout(Duration::from_secs(3), ping_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(healthy_ping, "/VCPlog/VCP_Key=runtime-test");
        let replacement = tokio::time::timeout(Duration::from_secs(9), socket_rx.recv())
            .await
            .expect("Info did not reconnect after its resume probe timed out")
            .unwrap();
        assert_eq!(replacement, ("/vcpinfo/VCP_Key=runtime-test".into(), true));
        assert!(resume_started.elapsed() >= Duration::from_secs(5));
        release_stalled.send(()).unwrap();
        assert_eq!(
            next_socket(&mut socket_rx).await,
            ("/vcpinfo/VCP_Key=runtime-test".into(), false)
        );
        expect_status("connected", "connected").await;

        // Once disconnected in an idle interval, neither channel may redial
        // even after the first retry delay (one second) has elapsed.
        set_foreground(false);
        commands.send(PeerCommand::Disconnect).unwrap();
        expect_pair(&mut socket_rx, false).await;
        expect_status("closed", "closed").await;
        assert!(
            tokio::time::timeout(Duration::from_millis(1300), socket_rx.recv())
                .await
                .is_err()
        );
        set_foreground(true);
        expect_pair(&mut socket_rx, true).await;
        expect_status("connected", "connected").await;

        let error = reconnect_log_connections(
            app.handle(),
            "example.invalid".into(),
            "runtime-test".into(),
        )
        .await
        .unwrap_err();
        assert!(!error.contains("runtime-test"));
        let notification = tokio::time::timeout(Duration::from_secs(5), notification_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(notification["data"]["status"], "error");
        assert!(notification["data"]["content"]
            .as_str()
            .unwrap()
            .contains(&error));
        expect_pair(&mut socket_rx, false).await;
        expect_status("closed", "closed").await;
        assert!(send_vcp_log_message(serde_json::json!({ "test": true }))
            .await
            .is_err());

        reconnect_log_connections(app.handle(), address.clone(), "runtime-test".into())
            .await
            .unwrap();
        expect_pair(&mut socket_rx, true).await;
        expect_status("connected", "connected").await;

        // A syntactically valid replacement B must close A even if B never
        // finishes its handshake. Clearing the configuration must then cancel
        // both pending handshakes and publish closed to the WebView.
        let pending_server = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let pending_address = format!("ws://{}", pending_server.local_addr().unwrap());
        reconnect_log_connections(app.handle(), pending_address, "runtime-test".into())
            .await
            .unwrap();
        expect_pair(&mut socket_rx, false).await;
        let mut pending_sockets = Vec::new();
        for _ in 0..2 {
            let (socket, _) = tokio::time::timeout(Duration::from_secs(5), pending_server.accept())
                .await
                .unwrap()
                .unwrap();
            pending_sockets.push(socket);
        }
        expect_status("connecting", "connecting").await;
        let chat = ChatActivityGuard::acquire(app.handle());
        set_foreground(false);
        app.state::<LifecycleState>()
            .ws_activity
            .network_available();
        set_foreground(true);
        assert!(
            tokio::time::timeout(Duration::from_millis(200), pending_server.accept())
                .await
                .is_err(),
            "network and resume hints must reuse both in-flight handshakes"
        );
        drop(chat);
        while status_rx.try_recv().is_ok() {}
        reconnect_log_connections(app.handle(), String::new(), String::new())
            .await
            .unwrap();
        for mut socket in pending_sockets {
            tokio::time::timeout(Duration::from_secs(5), async {
                let mut buffer = [0; 1024];
                while let Ok(size) = socket.read(&mut buffer).await {
                    if size == 0 {
                        break;
                    }
                }
            })
            .await
            .unwrap();
        }
        let mut stopped = Vec::new();
        for _ in 0..2 {
            stopped.push(
                tokio::time::timeout(Duration::from_secs(5), status_rx.recv())
                    .await
                    .unwrap()
                    .unwrap(),
            );
        }
        stopped.sort();
        assert_eq!(stopped, ["info", "log"]);
        expect_status("closed", "closed").await;

        reconnect_log_connections(app.handle(), address, "runtime-test".into())
            .await
            .unwrap();
        expect_pair(&mut socket_rx, true).await;
        expect_status("connected", "connected").await;

        assert!(init_vcp_log_connection_internal(
            app.handle().clone(),
            "example.invalid".into(),
            "runtime-test".into(),
        )
        .await
        .is_err());
        assert_eq!(
            next_socket(&mut socket_rx).await,
            ("/VCPlog/VCP_Key=runtime-test".into(), false)
        );
        expect_status("closed", "connected").await;
        assert!(
            super::super::vcp_info_service::init_vcp_info_connection_internal(
                app.handle().clone(),
                "example.invalid".into(),
                "runtime-test".into(),
            )
            .await
            .is_err()
        );
        assert_eq!(
            next_socket(&mut socket_rx).await,
            ("/vcpinfo/VCP_Key=runtime-test".into(), false)
        );
        expect_status("closed", "closed").await;

        reconnect_log_connections(app.handle(), String::new(), String::new())
            .await
            .unwrap();
        app.unlisten(event_id);
        app.unlisten(info_event_id);
        server.abort();
    }

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

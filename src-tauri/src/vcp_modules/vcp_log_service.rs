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

lazy_static::lazy_static! {
static ref LOG_CONNECTION_ACTIVE: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
static ref LOG_SENDER: Arc<tokio::sync::Mutex<Option<mpsc::UnboundedSender<Value>>>> = Arc::new(tokio::sync::Mutex::new(None));
// 关键修复：保持 Sender 和一个 Receiver 都在生命周期内，防止通道因无接收者而被视为关闭
static ref WS_URL_CHANNEL: (watch::Sender<Option<Url>>, watch::Receiver<Option<Url>>) = watch::channel(None);
static ref CURRENT_LOG_STATUS: Arc<tokio::sync::RwLock<String>> = Arc::new(tokio::sync::RwLock::new("closed".to_string()));
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
                    _ = sleep(Duration::from_secs(5)) => {},
                }
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
                    {
                        *CURRENT_LOG_STATUS.write().await = "open".to_string();
                    }
                    log::info!("[VCPLog] Connected successfully to {}", masked_url);

                    let (mut ws_write, mut ws_read) = ws_stream.split();

                    let _ = app_handle.emit(
                        "vcp-system-event",
                        serde_json::json!({
                            "type": "vcp-log-status",
                            "status": "open",
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

                    loop {
                        tokio::select! {
                            // 监听 URL 变更
                            _ = url_rx.changed() => {
                                log::info!("[VCPLog] URL changed, closing current connection.");
                                break;
                            }
                            // 处理接收到的消息
                            msg_result = ws_read.next() => {
                                match msg_result {
                                    Some(Ok(msg)) => {
                                        if msg.is_text() {
                                            let text = msg.to_text().unwrap_or_default();
                                            match serde_json::from_str::<Value>(text) {
                                                Ok(payload) => {
                                                    if let Err(e) = app_handle.emit("vcp-system-event", payload) {
                                                        log::error!("[VCPLog] Failed to emit event to frontend: {}", e);
                                                    }
                                                }
                                                Err(_) => {
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
                                "tool_name": "VCPLog 连接失败",                                "content": format!("❌ 连接错误: {}\n\n提示：\n1. 请检查桌面端 VCP 是否已开启且 VCPLog 服务正常。\n2. 检查 VCP API 地址和 Key 配置是否正确。", e),
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
            _ = sleep(Duration::from_secs(5)) => {},
        }
    }
}

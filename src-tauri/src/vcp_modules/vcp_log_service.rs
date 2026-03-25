use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc;
use tokio::time::{sleep, Duration};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use url::Url;

lazy_static::lazy_static! {
    static ref LOG_CONNECTION_ACTIVE: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
    static ref LOG_SENDER: Arc<tokio::sync::Mutex<Option<mpsc::UnboundedSender<Value>>>> = Arc::new(tokio::sync::Mutex::new(None));
}

#[tauri::command]
pub async fn send_vcp_log_message(payload: Value) -> Result<(), String> {
    let sender_lock = LOG_SENDER.lock().await;
    if let Some(sender) = sender_lock.as_ref() {
        sender.send(payload).map_err(|e| format!("Failed to send message to VCPLog: {}", e))?;
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
    if LOG_CONNECTION_ACTIVE.swap(true, Ordering::SeqCst) {
        println!("[VCPLog] Connection thread already active, ignoring request.");
        return Ok(());
    }

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

    let ws_url = match Url::parse(&url_with_key) {
        Ok(u) => u,
        Err(e) => {
            LOG_CONNECTION_ACTIVE.store(false, Ordering::SeqCst);
            return Err(format!("Invalid URL: {}", e));
        }
    };

    tauri::async_runtime::spawn(async move {
        start_vcp_log_listener(app, ws_url).await;
    });

    Ok(())
}

async fn start_vcp_log_listener(app_handle: AppHandle, ws_url: Url) {
    println!("[VCPLog] Starting background listener for {}", ws_url);

    // 创建 mpsc 通道用于回传消息
    let (tx, mut rx) = mpsc::unbounded_channel::<Value>();
    
    // 将发送端存储在全局静态变量中供 send_vcp_log_message 使用
    {
        let mut sender_lock = LOG_SENDER.lock().await;
        *sender_lock = Some(tx);
    }

    loop {
        println!("[VCPLog] Attempting to connect...");
        
        let _ = app_handle.emit("vcp-system-event", serde_json::json!({
            "type": "connection_status",
            "status": "connecting",
            "message": "Connecting...",
            "source": "VCPLog"
        }));

        let mut request = match ws_url.as_str().into_client_request() {
            Ok(req) => req,
            Err(e) => {
                eprintln!("[VCPLog] Failed to build request: {}. Retrying in 5 seconds...", e);
                let _ = app_handle.emit("vcp-system-event", serde_json::json!({
                    "type": "connection_status",
                    "status": "error",
                    "message": format!("Request error: {}", e),
                    "source": "VCPLog"
                }));
                sleep(Duration::from_secs(5)).await;
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

        println!("[VCPLog] Request headers: {:?}", request.headers());

        match tokio::time::timeout(Duration::from_secs(10), connect_async(request)).await {
            Ok(connection_result) => match connection_result {
                Ok((ws_stream, _)) => {
                    println!("[VCPLog] Connected successfully.");
                    
                    let (mut ws_write, mut ws_read) = ws_stream.split();

                    let _ = app_handle.emit("vcp-system-event", serde_json::json!({
                        "type": "connection_status",
                        "status": "connected",
                        "message": "Connected to VCPLog",
                        "source": "VCPLog"
                    }));

                    loop {
                        tokio::select! {
                            // 处理接收到的消息
                            msg_result = ws_read.next() => {
                                match msg_result {
                                    Some(Ok(msg)) => {
                                        if msg.is_text() {
                                            let text = msg.to_text().unwrap_or_default();
                                            match serde_json::from_str::<Value>(text) {
                                                Ok(payload) => {
                                                    if let Err(e) = app_handle.emit("vcp-system-event", payload) {
                                                        eprintln!("[VCPLog] Failed to emit event to frontend: {}", e);
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
                                        eprintln!("[VCPLog] WebSocket error during read: {}", e);
                                        break;
                                    }
                                    None => {
                                        println!("[VCPLog] Connection closed by server.");
                                        break;
                                    }
                                }
                            }
                            // 处理待发送的消息
                            payload_opt = rx.recv() => {
                                if let Some(payload) = payload_opt {
                                    if let Ok(text) = serde_json::to_string(&payload) {
                                        if let Err(e) = ws_write.send(Message::Text(text.into())).await {
                                            eprintln!("[VCPLog] Failed to send message: {}", e);
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                    }
                    
                    println!("[VCPLog] Disconnected.");
                    let _ = app_handle.emit("vcp-system-event", serde_json::json!({
                        "type": "connection_status",
                        "status": "disconnected",
                        "message": "Disconnected from VCPLog",
                        "source": "VCPLog"
                    }));
                }
                Err(e) => {
                    eprintln!("[VCPLog] Detailed Connection Error: {:?}. Status: {}", e, e);
                    let _ = app_handle.emit("vcp-system-event", serde_json::json!({
                        "type": "connection_status",
                        "status": "error",
                        "message": format!("Connection failed: {}", e),
                        "source": "VCPLog"
                    }));
                }
            },
            Err(_) => {
                eprintln!("[VCPLog] Connection timed out after 10 seconds. Retrying in 5 seconds...");
                let _ = app_handle.emit("vcp-system-event", serde_json::json!({
                    "type": "connection_status",
                    "status": "error",
                    "message": "Connection timed out",
                    "source": "VCPLog"
                }));
            }
        }

        sleep(Duration::from_secs(5)).await;
    }
}

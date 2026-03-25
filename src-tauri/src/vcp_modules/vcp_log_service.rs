use futures_util::StreamExt;
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tokio::time::{sleep, Duration};
use tokio_tungstenite::connect_async;
use url::Url;

lazy_static::lazy_static! {
    static ref LOG_CONNECTION_ACTIVE: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
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

    let url_with_key = if url.contains('?') {
        format!("{}&key={}", url, key)
    } else {
        format!("{}?key={}", url, key)
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

    loop {
        println!("[VCPLog] Attempting to connect...");
        match connect_async(ws_url.as_str()).await {
            Ok((mut ws_stream, _)) => {
                println!("[VCPLog] Connected successfully.");
                
                let _ = app_handle.emit("vcp-system-event", serde_json::json!({
                    "type": "connection_status",
                    "status": "connected",
                    "message": "Connected to VCPLog"
                }));

                while let Some(msg_result) = ws_stream.next().await {
                    match msg_result {
                        Ok(msg) => {
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
                        Err(e) => {
                            eprintln!("[VCPLog] WebSocket error during read: {}", e);
                            break; 
                        }
                    }
                }
                
                println!("[VCPLog] Disconnected.");
                let _ = app_handle.emit("vcp-system-event", serde_json::json!({
                    "type": "connection_status",
                    "status": "disconnected",
                    "message": "Disconnected from VCPLog"
                }));
            }
            Err(e) => {
                eprintln!("[VCPLog] Connection failed: {}. Retrying in 5 seconds...", e);
                let _ = app_handle.emit("vcp-system-event", serde_json::json!({
                    "type": "connection_status",
                    "status": "error",
                    "message": format!("Connection failed: {}", e)
                }));
            }
        }

        sleep(Duration::from_secs(5)).await;
    }
}

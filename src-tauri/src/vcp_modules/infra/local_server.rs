use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use futures_util::{sink::SinkExt, stream::StreamExt};
use serde_json::{json, Value};
use std::net::SocketAddr;
use tauri::{
    http::{header, Request, StatusCode},
    AppHandle, Manager,
};
use tokio::sync::watch;

#[derive(Clone)]
struct AppState {
    app_handle: AppHandle,
}

/// 服务器句柄：持有此句柄可触发优雅关闭
pub struct ServerHandle {
    shutdown_tx: watch::Sender<bool>,
}

impl ServerHandle {
    /// 发送关闭信号，服务器将优雅退出
    pub fn shutdown(self) {
        let _ = self.shutdown_tx.send(true);
    }
}

/// 启动本地 HTTP + WebSocket 服务器，返回可用于关闭的句柄
pub fn start_server(app_handle: AppHandle) -> ServerHandle {
    let state = AppState { app_handle };
    let addr = SocketAddr::from(([127, 0, 0, 1], 14202));
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    tauri::async_runtime::spawn(async move {
        let app = Router::new()
            .route("/ws", get(ws_handler))
            .route("/", get(index_handler))
            .route("/floating", get(floating_handler))
            .fallback(any_handler)
            .with_state(state);

        let listener = match tokio::net::TcpListener::bind(&addr).await {
            Ok(l) => l,
            Err(e) => {
                log::error!("[LocalServer] Failed to bind to {}: {}", addr, e);
                return;
            }
        };

        log::info!("[LocalServer] Listening on: {}", addr);

        let shutdown_signal = async move {
            let mut rx = shutdown_rx;
            loop {
                if *rx.borrow_and_update() {
                    log::info!("[LocalServer] Shutdown signal received.");
                    break;
                }
                if rx.changed().await.is_err() {
                    break;
                }
            }
        };

        if let Err(e) = axum::serve(listener, app)
            .with_graceful_shutdown(shutdown_signal)
            .await
        {
            log::error!("[LocalServer] Server error: {}", e);
        }
        log::info!("[LocalServer] Server stopped.");
    });

    ServerHandle { shutdown_tx }
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();
    while let Some(Ok(msg)) = receiver.next().await {
        if let Message::Text(text) = msg {
            let app_handle = state.app_handle.clone();
            let req: Value = match serde_json::from_str(&text) {
                Ok(v) => v,
                Err(e) => {
                    log::error!("WS parse error: {}", e);
                    continue;
                }
            };

            let action = req["action"].as_str().unwrap_or("");
            log::info!("[LocalServer/WS] Received action: {}", action);
            match action {
                "handle_assistant_chat_stream" => {
                    log::info!("[LocalServer/WS] Handling assistant chat stream...");
                    let payload_val = req["payload"].clone();
                    let payload: crate::vcp_modules::agent_chat_application_service::AssistantChatPayload =
                        match serde_json::from_value(payload_val) {
                            Ok(p) => p,
                            Err(e) => {
                                let _ = sender.send(Message::Text(json!({"type": "error", "error": format!("Invalid payload: {}", e)}).to_string())).await;
                                continue;
                            }
                        };

                    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
                    let channel = tauri::ipc::Channel::new(move |event| {
                        let _ = tx.send(event);
                        Ok(())
                    });

                    let app_c = app_handle.clone();
                    tauri::async_runtime::spawn(async move {
                        let agent_state =
                            app_c.state::<crate::vcp_modules::agent_service::AgentConfigState>();
                        let active_requests =
                            app_c.state::<crate::vcp_modules::vcp_client::ActiveRequests>();
                        let _ = crate::vcp_modules::agent_chat_application_service::handle_assistant_chat_stream(
                            app_c.clone(),
                            agent_state,
                            active_requests,
                            payload,
                            channel
                        ).await;
                    });

                    while let Some(event) = rx.recv().await {
                        use tauri::ipc::InvokeResponseBody;
                        let event_json = match event {
                            InvokeResponseBody::Json(v) => v.to_string(),
                            _ => "{}".to_string(),
                        };
                        if sender.send(Message::Text(event_json)).await.is_err() {
                            break;
                        }
                    }
                }
                "archive_assistant_chat" => {
                    let payload = req["payload"].clone();
                    let owner_id = payload["ownerId"].as_str().unwrap_or("").to_string();
                    let owner_type = payload["ownerType"].as_str().unwrap_or("").to_string();
                    let temp_messages_val = payload["tempMessages"].clone();
                    let temp_messages: Vec<crate::vcp_modules::chat::topic_service::TempMessage> =
                        serde_json::from_value(temp_messages_val).unwrap_or_default();

                    let app_c = app_handle.clone();
                    let result = crate::vcp_modules::topic_service::archive_assistant_chat(
                        app_c.clone(),
                        app_c.state(),
                        owner_id,
                        owner_type,
                        temp_messages,
                    )
                    .await;

                    match result {
                        Ok(topic_id) => {
                            let _ = sender
                                .send(Message::Text(
                                    json!({"type": "archive_success", "topicId": topic_id})
                                        .to_string(),
                                ))
                                .await;
                        }
                        Err(e) => {
                            let _ = sender
                                .send(Message::Text(
                                    json!({"type": "error", "error": e}).to_string(),
                                ))
                                .await;
                        }
                    }
                }
                "get_initial_config" => {
                    let app_c = app_handle.clone();
                    match crate::vcp_modules::settings_manager::read_settings(
                        app_c.clone(),
                        app_c.state(),
                    )
                    .await
                    {
                        Ok(settings) => {
                            log::info!(
                                "[LocalServer/WS] Sending initial_config, assistantAgentId={}",
                                settings.assistant_agent_id
                            );
                            let _ = sender
                                .send(Message::Text(
                                    json!({
                                        "type": "initial_config",
                                        "settings": settings,
                                    })
                                    .to_string(),
                                ))
                                .await;
                        }
                        Err(e) => {
                            log::error!("[LocalServer/WS] Failed to read settings: {}", e);
                            let _ = sender
                                .send(Message::Text(
                                    json!({
                                        "type": "error",
                                        "error": format!("Failed to read settings: {}", e),
                                    })
                                    .to_string(),
                                ))
                                .await;
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

async fn index_handler(State(state): State<AppState>) -> impl IntoResponse {
    serve_asset(state, "index.html".to_string(), "".to_string()).await
}

async fn floating_handler(State(state): State<AppState>) -> impl IntoResponse {
    serve_asset(state, "floating.html".to_string(), "".to_string()).await
}

async fn any_handler(
    State(state): State<AppState>,
    req: Request<axum::body::Body>,
) -> impl IntoResponse {
    let path = req.uri().path().trim_start_matches('/').to_string();
    let query = req.uri().query().unwrap_or("").to_string();
    serve_asset(state, path, query).await
}

async fn serve_asset(state: AppState, path: String, query: String) -> Response<axum::body::Body> {
    let clean_path = if path.is_empty() {
        "index.html".to_string()
    } else {
        path
    };

    // 1. 优先尝试从 Tauri 内置资源读取
    if let Some(asset) = state.app_handle.asset_resolver().get(clean_path.clone()) {
        let mut mime = mime_guess::from_path(&clean_path)
            .first_or_octet_stream()
            .to_string();
        // 💥 修复：在内置资源模式下也需要处理 Vite import
        if query.contains("import") || query.contains("t=") {
            mime = "application/javascript".to_string();
        }

        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, mime)
            .body(axum::body::Body::from(asset.bytes))
            .unwrap();
    }

    // 2. 关键修复：开发模式下的透明代理 (带完整的 query 参数)
    #[cfg(debug_assertions)]
    {
        let client = reqwest::Client::new();
        let dev_url = if query.is_empty() {
            format!("http://127.0.0.1:1420/{}", clean_path)
        } else {
            format!("http://127.0.0.1:1420/{}?{}", clean_path, query)
        };

        match client.get(&dev_url).send().await {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    let mut mime = resp
                        .headers()
                        .get(header::CONTENT_TYPE)
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("application/octet-stream")
                        .to_string();

                    // 💥 修复：针对 Vite 的 ?import 请求，强制返回 JS MIME 类型
                    if query.contains("import") || query.contains("t=") {
                        mime = "application/javascript".to_string();
                    }

                    let body_bytes = resp.bytes().await.unwrap_or_default();
                    return Response::builder()
                        .status(StatusCode::OK)
                        .header(header::CONTENT_TYPE, mime)
                        .body(axum::body::Body::from(body_bytes))
                        .unwrap();
                }
            }
            Err(e) => {
                log::warn!("[LocalServer] Dev proxy failed for {}: {}", dev_url, e);
            }
        }
    }

    // 3. 兜底 index.html
    if clean_path != "index.html" {
        if let Some(index_asset) = state
            .app_handle
            .asset_resolver()
            .get("index.html".to_string())
        {
            return Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/html")
                .body(axum::body::Body::from(index_asset.bytes))
                .unwrap();
        }
    }

    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .body(axum::body::Body::from(format!(
            r#"<div style="padding: 20px; background: rgba(255,0,0,0.1); color: red; font-family: sans-serif;">
                <h3>VCP 助手资源未找到 (404)</h3>
                <p>路径: <b>{}?{}</b></p>
                <p><b>调试诊断 (USB 模式)：</b></p>
                <ul>
                    <li>如果您在使用 <b>pnpm dev:usb</b>，请确保电脑端 Vite 已启动 (1420 端口)。</li>
                    <li>检查 <b>adb reverse tcp:1420 tcp:1420</b> 是否在运行。</li>
                </ul>
                <button onclick="window.AndroidBridge.closeWindow()" style="padding: 8px 16px;">关闭悬浮窗</button>
            </div>"#, 
            clean_path, query
        )))
        .unwrap()
}

// distributed/client.rs
// WebSocket client for VCP Distributed Node
// Mirrors VCPChat/VCPDistributedServer/VCPDistributedServer.js (class DistributedServer)
// Self-contained — does NOT import anything from vcp_modules/.

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tauri::{AppHandle, Emitter};
use tokio::sync::{watch, Mutex, RwLock};
use tokio::time;

use super::tool_registry::ToolRegistry;
use super::types::*;

/// Distributed node state, shared across async tasks.
pub struct DistributedClient {
    /// Signal to stop all background tasks.
    shutdown_tx: watch::Sender<bool>,
    /// Current connection status.
    status: Arc<RwLock<DistributedStatus>>,
    /// Handle to the background connection task.
    task_handle: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl DistributedClient {
    pub fn new() -> Self {
        let (shutdown_tx, _) = watch::channel(false);
        Self {
            shutdown_tx,
            status: Arc::new(RwLock::new(DistributedStatus::default())),
            task_handle: Mutex::new(None),
        }
    }

    /// Start the distributed node connection.
    /// `ws_url`: base URL of the main server, e.g. "ws://192.168.1.100:5800"
    /// `vcp_key`: authentication key
    /// `device_name`: node name (maps to VCPChat's `serverName` / config.env `ServerName`)
    pub async fn start(
        &self,
        app: AppHandle,
        ws_url: String,
        vcp_key: String,
        device_name: String,
        registry: Arc<ToolRegistry>,
    ) -> Result<(), String> {
        // If already running, stop first.
        self.stop().await;

        // Reset shutdown signal.
        let _ = self.shutdown_tx.send(false);
        let shutdown_rx = self.shutdown_tx.subscribe();
        let status = self.status.clone();

        // Clear any previous error
        {
            let mut s = status.write().await;
            *s = DistributedStatus::default();
        }
        Self::emit_status(&app, &status).await;

        let handle = tokio::spawn(Self::connection_loop(
            app,
            ws_url,
            vcp_key,
            device_name,
            shutdown_rx,
            status,
            registry,
        ));

        *self.task_handle.lock().await = Some(handle);
        Ok(())
    }

    /// Stop the distributed node.
    pub async fn stop(&self) {
        let _ = self.shutdown_tx.send(true);
        if let Some(handle) = self.task_handle.lock().await.take() {
            handle.abort();
            let _ = handle.await;
        }
        let mut s = self.status.write().await;
        s.connected = false;
        s.server_id = None;
        s.client_id = None;
    }

    /// Get current status snapshot.
    pub async fn get_status(&self) -> DistributedStatus {
        self.status.read().await.clone()
    }

    // ================================================================
    // Connection loop — mirrors DistributedServer.connect() + scheduleReconnect()
    // ================================================================

    async fn connection_loop(
        app: AppHandle,
        ws_url: String,
        vcp_key: String,
        device_name: String,
        mut shutdown_rx: watch::Receiver<bool>,
        status: Arc<RwLock<DistributedStatus>>,
        registry: Arc<ToolRegistry>,
    ) {
        let mut reconnect_interval = Duration::from_secs(5);
        let max_reconnect_interval = Duration::from_secs(60);

        loop {
            // Check shutdown before connecting.
            if *shutdown_rx.borrow() {
                break;
            }

            // Build connection URL: ws://host:port/vcp-distributed-server/VCP_Key=<key>
            let connection_url = format!(
                "{}/vcp-distributed-server/VCP_Key={}",
                ws_url.trim_end_matches('/'),
                vcp_key
            );

            log::info!(
                "[Distributed] Connecting to main server: {}",
                connection_url.replace(&vcp_key, "***")
            );

            match tokio_tungstenite::connect_async(&connection_url).await {
                Ok((ws_stream, _response)) => {
                    log::info!("[Distributed] WebSocket connected.");
                    reconnect_interval = Duration::from_secs(5); // Reset backoff on success.

                    // Run the session until it ends.
                    Self::run_session(
                        &app,
                        ws_stream,
                        &device_name,
                        &mut shutdown_rx,
                        &status,
                        &registry,
                    )
                    .await;

                    // Session ended — update status.
                    {
                        let mut s = status.write().await;
                        s.connected = false;
                        s.server_id = None;
                        s.client_id = None;
                    }
                    Self::emit_status(&app, &status).await;
                }
                Err(e) => {
                    log::warn!("[Distributed] Connection failed: {}", e);
                    {
                        let mut s = status.write().await;
                        s.connected = false;
                        s.last_error = Some(format!("Connection failed: {}", e));
                    }
                    Self::emit_status(&app, &status).await;
                }
            }

            // Check shutdown before waiting.
            if *shutdown_rx.borrow() {
                break;
            }

            // Exponential backoff — mirrors scheduleReconnect()
            log::info!(
                "[Distributed] Reconnecting in {}s...",
                reconnect_interval.as_secs()
            );

            tokio::select! {
                _ = time::sleep(reconnect_interval) => {},
                _ = shutdown_rx.changed() => {
                    break;
                }
            }

            reconnect_interval =
                std::cmp::min(reconnect_interval * 2, max_reconnect_interval);
        }

        log::info!("[Distributed] Connection loop exited.");
    }

    // ================================================================
    // Session handler — processes one WS connection lifetime
    // ================================================================

    async fn run_session(
        app: &AppHandle,
        ws_stream: tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        device_name: &str,
        shutdown_rx: &mut watch::Receiver<bool>,
        status: &Arc<RwLock<DistributedStatus>>,
        registry: &Arc<ToolRegistry>,
    ) {
        use tokio_tungstenite::tungstenite::Message;

        let (ws_tx, mut ws_rx) = ws_stream.split();

        // Wrap tx in Arc<Mutex> so we can send from multiple places.
        let ws_tx = Arc::new(Mutex::new(ws_tx));

        // Static placeholder push timer — mirrors setupStaticPlaceholderUpdates() (30s interval)
        let mut placeholder_interval = time::interval(Duration::from_secs(30));
        // Skip the first immediate tick; we do an initial push below after registration.
        placeholder_interval.tick().await;

        loop {
            tokio::select! {
                // --- Receive messages from main server ---
                msg = ws_rx.next() => {
                    match msg {
                        Some(Ok(Message::Text(text))) => {
                            Self::handle_incoming(
                                app,
                                &text,
                                device_name,
                                &ws_tx,
                                status,
                                registry,
                            ).await;
                        }
                        Some(Ok(Message::Ping(data))) => {
                            let mut tx = ws_tx.lock().await;
                            let _ = tx.send(Message::Pong(data)).await;
                        }
                        Some(Ok(Message::Close(_))) => {
                            log::info!("[Distributed] Server sent close frame.");
                            break;
                        }
                        Some(Err(e)) => {
                            log::warn!("[Distributed] WS error: {}", e);
                            break;
                        }
                        None => {
                            log::info!("[Distributed] WS stream ended.");
                            break;
                        }
                        _ => {} // Binary, Pong — ignore
                    }
                }

                // --- Periodic static placeholder push ---
                _ = placeholder_interval.tick() => {
                    Self::push_static_placeholders(device_name, &ws_tx, registry).await;
                }

                // --- Shutdown signal ---
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        log::info!("[Distributed] Shutdown signal received, closing session.");
                        let mut tx = ws_tx.lock().await;
                        let _ = tx.close().await;
                        break;
                    }
                }
            }
        }
    }

    // ================================================================
    // Incoming message handler
    // ================================================================

    async fn handle_incoming(
        app: &AppHandle,
        text: &str,
        device_name: &str,
        ws_tx: &Arc<Mutex<futures_util::stream::SplitSink<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
            tokio_tungstenite::tungstenite::Message,
        >>>,
        status: &Arc<RwLock<DistributedStatus>>,
        registry: &Arc<ToolRegistry>,
    ) {
        let envelope: IncomingEnvelope = match serde_json::from_str(text) {
            Ok(e) => e,
            Err(e) => {
                log::warn!("[Distributed] Failed to parse message: {}", e);
                return;
            }
        };

        match envelope.parse() {
            IncomingMessage::ConnectionAck {
                server_id,
                client_id,
            } => {
                log::info!(
                    "[Distributed] Connection acknowledged. serverId={}, clientId={}",
                    server_id,
                    client_id
                );

                // Update status
                {
                    let mut s = status.write().await;
                    s.connected = true;
                    s.server_id = Some(server_id.clone());
                    s.client_id = Some(client_id.clone());
                    s.last_error = None;
                }
                Self::emit_status_with_app(app, status).await;

                // Register tools — mirrors registerTools()
                Self::register_tools(device_name, ws_tx, registry, status).await;

                // Report IP — mirrors reportIPAddress()
                Self::report_ip(device_name, ws_tx).await;

                // Initial static placeholder push (2s delay in VCPChat, do it immediately here)
                Self::push_static_placeholders(device_name, ws_tx, registry).await;
            }

            IncomingMessage::ExecuteTool {
                request_id,
                tool_name,
                tool_args,
            } => {
                log::info!(
                    "[Distributed] Execute tool request: {} (requestId={})",
                    tool_name,
                    request_id
                );

                // Execute and return result.
                let response = Self::execute_tool(app, &request_id, &tool_name, tool_args, registry).await;
                Self::send_message(ws_tx, &response).await;
            }

            IncomingMessage::Unknown(msg_type) => {
                log::debug!("[Distributed] Unknown message type: {}", msg_type);
            }
        }
    }

    // ================================================================
    // Protocol actions (mirrors DistributedServer methods)
    // ================================================================

    /// Register tools with the main server.
    /// VCPChat ref: registerTools() line 271-308
    async fn register_tools(
        device_name: &str,
        ws_tx: &Arc<Mutex<futures_util::stream::SplitSink<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
            tokio_tungstenite::tungstenite::Message,
        >>>,
        registry: &Arc<ToolRegistry>,
        status: &Arc<RwLock<DistributedStatus>>,
    ) {
        let tools = registry.get_all_manifests();

        if tools.is_empty() {
            log::info!("[Distributed] No tools to register.");
            return;
        }

        let count = tools.len();
        let msg = OutgoingMessage::RegisterTools {
            server_name: device_name.to_string(),
            tools,
        };
        Self::send_message(ws_tx, &msg).await;

        // Update status with tool count
        {
            let mut s = status.write().await;
            s.registered_tools = count;
        }

        log::info!("[Distributed] Registered {} tools with main server.", count);
    }

    /// Report IP addresses to the main server.
    /// VCPChat ref: reportIPAddress() line 310-347
    async fn report_ip(
        device_name: &str,
        ws_tx: &Arc<Mutex<futures_util::stream::SplitSink<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
            tokio_tungstenite::tungstenite::Message,
        >>>,
    ) {
        // Collect local IPv4 addresses (simplified — no external crate needed)
        let local_ips = Vec::new(); // TODO: enumerate network interfaces in Phase 2

        // Optional: fetch public IP
        let public_ip: Option<String> = match reqwest::get("https://api.ipify.org?format=json").await {
            Ok(resp) => {
                if let Ok(data) = resp.json::<Value>().await {
                    data.get("ip").and_then(|v| v.as_str()).map(|s| s.to_string())
                } else {
                    None
                }
            }
            Err(e) => {
                log::warn!("[Distributed] Could not fetch public IP: {}", e);
                None
            }
        };

        let msg = OutgoingMessage::ReportIp {
            server_name: device_name.to_string(),
            local_ips,
            public_ip,
        };
        Self::send_message(ws_tx, &msg).await;
        log::info!("[Distributed] IP report sent.");
    }

    /// Push static placeholder values.
    /// VCPChat ref: pushStaticPlaceholderValues() line 374-398
    async fn push_static_placeholders(
        device_name: &str,
        ws_tx: &Arc<Mutex<futures_util::stream::SplitSink<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
            tokio_tungstenite::tungstenite::Message,
        >>>,
        registry: &Arc<ToolRegistry>,
    ) {
        let placeholders = registry.get_all_placeholder_values();

        if placeholders.is_empty() {
            return;
        }

        let msg = OutgoingMessage::UpdateStaticPlaceholders {
            server_name: device_name.to_string(),
            placeholders,
        };
        Self::send_message(ws_tx, &msg).await;
    }

    /// Execute a tool and return the result message.
    /// VCPChat ref: handleToolExecutionRequest() line 428-649
    async fn execute_tool(
        app: &AppHandle,
        request_id: &str,
        tool_name: &str,
        tool_args: Value,
        registry: &Arc<ToolRegistry>,
    ) -> OutgoingMessage {
        match registry.execute(tool_name, tool_args, app).await {
            Ok(result) => {
                log::info!("[Distributed] Tool '{}' executed successfully.", tool_name);
                OutgoingMessage::ToolResult {
                    request_id: request_id.to_string(),
                    status: "success".to_string(),
                    result: Some(result),
                    error: None,
                }
            }
            Err(e) => {
                log::warn!("[Distributed] Tool '{}' failed: {}", tool_name, e);
                OutgoingMessage::ToolResult {
                    request_id: request_id.to_string(),
                    status: "error".to_string(),
                    result: None,
                    error: Some(e),
                }
            }
        }
    }

    // ================================================================
    // Helpers
    // ================================================================

    /// Serialize and send a message over WebSocket.
    async fn send_message(
        ws_tx: &Arc<Mutex<futures_util::stream::SplitSink<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
            tokio_tungstenite::tungstenite::Message,
        >>>,
        msg: &OutgoingMessage,
    ) {
        match serde_json::to_string(msg) {
            Ok(json) => {
                let mut tx = ws_tx.lock().await;
                if let Err(e) = tx
                    .send(tokio_tungstenite::tungstenite::Message::Text(json.into()))
                    .await
                {
                    log::warn!("[Distributed] Failed to send message: {}", e);
                }
            }
            Err(e) => {
                log::error!("[Distributed] Failed to serialize message: {}", e);
            }
        }
    }

    /// Emit status to the Vue frontend.
    async fn emit_status(app: &AppHandle, status: &Arc<RwLock<DistributedStatus>>) {
        let s = status.read().await.clone();
        let _ = app.emit("vcp-distributed-status", &s);
    }

    async fn emit_status_with_app(app: &AppHandle, status: &Arc<RwLock<DistributedStatus>>) {
        Self::emit_status(app, status).await;
    }
}

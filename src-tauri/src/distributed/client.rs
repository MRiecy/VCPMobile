// distributed/client.rs
// WebSocket client for VCP Distributed Node
// Mirrors VCPChat/VCPDistributedServer/VCPDistributedServer.js (class DistributedServer)
// Self-contained — does NOT import anything from vcp_modules/.

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tauri::{AppHandle, Emitter};
use tokio::sync::{Mutex, RwLock};
use tokio::time;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_util::sync::CancellationToken;

use super::tool_registry::ToolRegistry;
use super::types::*;

/// Type alias for the WebSocket sink to avoid excessive complexity in signatures.
type WsSink = Arc<
    Mutex<
        futures_util::stream::SplitSink<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
            WsMessage,
        >,
    >,
>;

/// Immutable configuration for a single connection lifecycle.
struct ConnectionConfig {
    ws_url: String,
    vcp_key: String,
    device_name: String,
}

/// Runtime context for a single connection lifecycle (channel receivers).
struct SessionContext {
    status: Arc<RwLock<DistributedStatus>>,
    registry: Arc<ToolRegistry>,
    re_register_rx: tokio::sync::mpsc::Receiver<()>,
    reconnect_rx: tokio::sync::mpsc::Receiver<()>,
}

/// Handle to an active connection session — created by start(), dropped by stop().
struct ConnectionSession {
    cancel_token: CancellationToken,
    re_register_tx: tokio::sync::mpsc::Sender<()>,
    reconnect_tx: tokio::sync::mpsc::Sender<()>,
    task_handle: tokio::task::JoinHandle<()>,
}

/// Distributed node state, shared across async tasks.
pub struct DistributedClient {
    /// Current connection status.
    status: Arc<RwLock<DistributedStatus>>,
    /// Active session handle — None when disconnected.
    session: Mutex<Option<ConnectionSession>>,
}

impl DistributedClient {
    pub fn new() -> Self {
        Self {
            status: Arc::new(RwLock::new(DistributedStatus::default())),
            session: Mutex::new(None),
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
        // Prevent duplicate activation using ConnectionState.
        {
            let mut s = self.status.write().await;
            if s.state == ConnectionState::Connected || s.state == ConnectionState::Connecting {
                log::info!(
                    "[Distributed] Connection is already running or connecting ({:?}), skipping start request.",
                    s.state
                );
                return Ok(());
            }
            s.state = ConnectionState::Connecting;
            s.connected = false;
            s.server_id = None;
            s.client_id = None;
            s.last_error = None;
        }

        // Gracefully shut down any existing session before creating a new one.
        if let Some(old_session) = self.session.lock().await.take() {
            old_session.cancel_token.cancel();
            let _ = old_session.task_handle.await;
        }

        // Create fresh channels and cancellation token — no state reuse from previous cycles.
        let cancel_token = CancellationToken::new();
        let (re_register_tx, re_register_rx) = tokio::sync::mpsc::channel(1);
        let (reconnect_tx, reconnect_rx) = tokio::sync::mpsc::channel(1);
        let status = self.status.clone();

        Self::emit_status(&app, &status).await;

        #[cfg(target_os = "android")]
        if let Err(e) = tauri_plugin_vcp_mobile::stream::set_keepalive_mode(&app, true) {
            log::warn!(
                "[Distributed] Failed to start keepalive foreground service: {}",
                e
            );
        }

        let config = ConnectionConfig {
            ws_url,
            vcp_key,
            device_name,
        };
        let ctx = SessionContext {
            status,
            registry,
            re_register_rx,
            reconnect_rx,
        };
        let loop_token = cancel_token.clone();

        let task_handle = tokio::spawn(Self::connection_loop(app, config, loop_token, ctx));

        *self.session.lock().await = Some(ConnectionSession {
            cancel_token,
            re_register_tx,
            reconnect_tx,
            task_handle,
        });
        Ok(())
    }

    /// Stop the distributed node.
    pub async fn stop(&self, _app: &AppHandle) {
        #[cfg(target_os = "android")]
        if let Err(e) = tauri_plugin_vcp_mobile::stream::set_keepalive_mode(_app, false) {
            log::warn!(
                "[Distributed] Failed to stop keepalive foreground service: {}",
                e
            );
        }

        {
            let mut s = self.status.write().await;
            if s.state == ConnectionState::Disconnected || s.state == ConnectionState::Disconnecting
            {
                log::info!(
                    "[Distributed] Already disconnected or disconnecting, skipping stop request."
                );
                return;
            }
            s.state = ConnectionState::Disconnecting;
        }

        // Take the session out and gracefully shut it down.
        // cancel() signals the connection_loop to exit; task_handle.await waits for
        // the loop's tail cleanup (which sets Disconnected + emits status).
        if let Some(session) = self.session.lock().await.take() {
            session.cancel_token.cancel();
            let _ = session.task_handle.await;
            // session drops here → re_register_tx, reconnect_tx naturally close
        }

        // Safety net: ensure final Disconnected state if loop didn't clean up properly.
        {
            let mut s = self.status.write().await;
            if s.state == ConnectionState::Disconnecting {
                s.state = ConnectionState::Disconnected;
                s.connected = false;
                s.server_id = None;
                s.client_id = None;
            }
        }
        Self::emit_status(_app, &self.status).await;
    }

    /// Get current status snapshot.
    pub async fn get_status(&self) -> DistributedStatus {
        self.status.read().await.clone()
    }

    /// Check if the distributed client is connected.
    pub async fn is_connected(&self) -> bool {
        self.status.read().await.connected
    }

    /// Check if the connection task is running (connecting, connected, or disconnecting).
    pub async fn is_running(&self) -> bool {
        self.status.read().await.state != ConnectionState::Disconnected
    }

    /// Trigger re-registration of tools.
    pub async fn re_register_tools(&self) {
        if let Some(session) = self.session.lock().await.as_ref() {
            let _ = session.re_register_tx.send(()).await;
        }
    }

    /// Trigger immediate reconnection.
    pub async fn trigger_reconnect(&self) {
        if let Some(session) = self.session.lock().await.as_ref() {
            let _ = session.reconnect_tx.send(()).await;
        }
    }

    // ================================================================
    // Connection loop — mirrors DistributedServer.connect() + scheduleReconnect()
    // ================================================================

    async fn connection_loop(
        app: AppHandle,
        config: ConnectionConfig,
        cancel_token: CancellationToken,
        ctx: SessionContext,
    ) {
        let mut reconnect_interval = Duration::from_secs(5);
        let max_reconnect_interval = Duration::from_secs(60);
        let re_register_rx = Arc::new(Mutex::new(ctx.re_register_rx));
        let mut reconnect_rx = ctx.reconnect_rx;
        let status = ctx.status;
        let registry = ctx.registry;

        loop {
            // Check cancellation before connecting.
            if cancel_token.is_cancelled() {
                break;
            }

            // Build connection URL: ws://host:port/vcp-distributed-server/VCP_Key=<key>
            let connection_url = format!(
                "{}/vcp-distributed-server/VCP_Key={}",
                config.ws_url.trim_end_matches('/'),
                config.vcp_key
            );

            log::info!(
                "[Distributed] Connecting to main server: {}",
                connection_url.replace(&config.vcp_key, "***")
            );

            // Connect with cancellation support — avoids blocking on TCP timeout during shutdown.
            acquire_wake_lock_helper(&app);
            let connect_result = tokio::select! {
                result = tokio_tungstenite::connect_async(&connection_url) => Some(result),
                _ = cancel_token.cancelled() => None,
            };
            release_wake_lock_helper(&app);

            match connect_result {
                Some(Ok((ws_stream, _response))) => {
                    log::info!("[Distributed] WebSocket connected.");
                    reconnect_interval = Duration::from_secs(5); // Reset backoff on success.

                    // Run the session until it ends.
                    let exit_reason = Self::run_session(
                        &app,
                        ws_stream,
                        &config.device_name,
                        &cancel_token,
                        &status,
                        &registry,
                        re_register_rx.clone(),
                    )
                    .await;

                    // Session ended — update status.
                    {
                        let mut s = status.write().await;
                        if s.state != ConnectionState::Disconnecting {
                            s.state = ConnectionState::Connecting;
                        }
                        s.connected = false;
                        s.server_id = None;
                        s.client_id = None;
                        s.last_error = Some(exit_reason);
                    }
                    Self::emit_status(&app, &status).await;
                }
                Some(Err(e)) => {
                    log::warn!("[Distributed] Connection failed: {}", e);
                    {
                        let mut s = status.write().await;
                        if s.state != ConnectionState::Disconnecting {
                            s.state = ConnectionState::Connecting;
                        }
                        s.connected = false;
                        s.last_error = Some(format!("Connection failed: {}", e));
                    }
                    Self::emit_status(&app, &status).await;
                }
                None => {
                    // Cancelled during connect — exit loop immediately.
                    break;
                }
            }

            // Check cancellation before waiting.
            if cancel_token.is_cancelled() {
                break;
            }

            // Exponential backoff — mirrors scheduleReconnect()
            log::info!(
                "[Distributed] Reconnecting in {}s...",
                reconnect_interval.as_secs()
            );

            tokio::select! {
                _ = time::sleep(reconnect_interval) => {},
                _ = reconnect_rx.recv() => {
                    log::info!("[Distributed] Triggering immediate reconnect due to network restore event.");
                }
                _ = cancel_token.cancelled() => {
                    break;
                }
            }

            reconnect_interval = std::cmp::min(reconnect_interval * 2, max_reconnect_interval);
        }

        {
            let mut s = status.write().await;
            s.state = ConnectionState::Disconnected;
            s.connected = false;
            s.server_id = None;
            s.client_id = None;
        }
        Self::emit_status(&app, &status).await;
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
        cancel_token: &CancellationToken,
        status: &Arc<RwLock<DistributedStatus>>,
        registry: &Arc<ToolRegistry>,
        re_register_rx: Arc<Mutex<tokio::sync::mpsc::Receiver<()>>>,
    ) -> String {
        use tokio_tungstenite::tungstenite::Message;

        #[cfg(target_os = "android")]
        if let Err(e) = tauri_plugin_vcp_mobile::system::start_sensor_collection(app.clone()) {
            log::warn!(
                "[Distributed] Failed to start native sensor collection: {}",
                e
            );
        }

        let (ws_tx, mut ws_rx) = ws_stream.split();

        // Wrap tx in Arc<Mutex> so we can send from multiple places.
        let ws_tx = Arc::new(Mutex::new(ws_tx));

        // Static placeholder push timer — mirrors setupStaticPlaceholderUpdates() (30s interval)
        let mut placeholder_interval = time::interval(Duration::from_secs(30));
        // Skip the first immediate tick; we do an initial push below after registration.
        placeholder_interval.tick().await;

        #[allow(unused_assignments)]
        let mut exit_reason = "Connection closed normally".to_string();

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
                        Some(Ok(Message::Close(reason))) => {
                            let r_str = reason.map(|r| format!("{} (code: {})", r.reason, r.code)).unwrap_or_else(|| "No reason provided".to_string());
                            log::info!("[Distributed] Server sent close frame: {}", r_str);
                            exit_reason = format!("Server closed connection: {}", r_str);
                            break;
                        }
                        Some(Err(e)) => {
                            log::warn!("[Distributed] WS error: {}", e);
                            exit_reason = format!("WebSocket error: {}", e);
                            break;
                        }
                        None => {
                            log::info!("[Distributed] WS stream ended.");
                            exit_reason = "WS stream ended (server disconnected)".to_string();
                            break;
                        }
                        _ => {} // Binary, Pong — ignore
                    }
                }

                // --- Out-of-band re-registration request ---
                opt = async {
                    let mut rx = re_register_rx.lock().await;
                    rx.recv().await
                } => {
                    if opt.is_some() {
                        log::info!("[Distributed] Re-registering tools due to configuration change.");
                        Self::register_tools(device_name, &ws_tx, registry, status).await;
                        Self::emit_status_with_app(app, status).await;
                    }
                }

                // --- Periodic static placeholder push ---
                _ = placeholder_interval.tick() => {
                    acquire_wake_lock_helper(app);
                    Self::push_static_placeholders(app, device_name, &ws_tx, registry).await;
                    release_wake_lock_helper(app);
                }

                // --- Cancellation signal ---
                _ = cancel_token.cancelled() => {
                    log::info!("[Distributed] Shutdown signal received, closing session.");
                    let mut tx = ws_tx.lock().await;
                    let _ = tx.close().await;
                    exit_reason = "Client requested shutdown".to_string();
                    break;
                }
            }
        }

        #[cfg(target_os = "android")]
        if let Err(e) = tauri_plugin_vcp_mobile::system::stop_sensor_collection(app.clone()) {
            log::warn!(
                "[Distributed] Failed to stop native sensor collection: {}",
                e
            );
        }

        exit_reason
    }

    // ================================================================
    // Incoming message handler
    // ================================================================

    async fn handle_incoming(
        app: &AppHandle,
        text: &str,
        device_name: &str,
        ws_tx: &WsSink,
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
                    s.state = ConnectionState::Connected;
                    s.connected = true;
                    s.server_id = Some(server_id.clone());
                    s.client_id = Some(client_id.clone());
                    s.last_error = None;
                }
                Self::emit_status_with_app(app, status).await;

                // Register tools — mirrors registerTools()
                Self::register_tools(device_name, ws_tx, registry, status).await;
                Self::emit_status_with_app(app, status).await;

                // Report IP — mirrors reportIPAddress()
                let device_name_clone = device_name.to_string();
                let ws_tx_clone = ws_tx.clone();
                tokio::spawn(async move {
                    Self::report_ip(&device_name_clone, &ws_tx_clone).await;
                });

                // Initial static placeholder push (2s delay in VCPChat, do it immediately here)
                Self::push_static_placeholders(app, device_name, ws_tx, registry).await;
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
                acquire_wake_lock_helper(app);
                let response =
                    Self::execute_tool(app, &request_id, &tool_name, tool_args, registry).await;
                Self::send_message(ws_tx, &response).await;
                release_wake_lock_helper(app);
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
        ws_tx: &WsSink,
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
    async fn report_ip(device_name: &str, ws_tx: &WsSink) {
        // Collect local IPv4 addresses (simplified — no external crate needed)
        let local_ips = Vec::new(); // TODO: enumerate network interfaces in Phase 2

        // Optional: fetch public IP with a 5-second timeout
        let public_ip: Option<String> = {
            let fetch_fut = async {
                match reqwest::get("https://api.ipify.org?format=json").await {
                    Ok(resp) => {
                        if let Ok(data) = resp.json::<Value>().await {
                            data.get("ip")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string())
                        } else {
                            None
                        }
                    }
                    Err(e) => {
                        log::warn!("[Distributed] Could not fetch public IP: {}", e);
                        None
                    }
                }
            };
            match tokio::time::timeout(Duration::from_secs(5), fetch_fut).await {
                Ok(val) => val,
                Err(_) => {
                    log::warn!("[Distributed] Fetching public IP timed out");
                    None
                }
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
        app: &AppHandle,
        device_name: &str,
        ws_tx: &WsSink,
        registry: &Arc<ToolRegistry>,
    ) {
        let placeholders = registry.get_all_placeholder_values(app);

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
    async fn send_message(ws_tx: &WsSink, msg: &OutgoingMessage) {
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

#[cfg(target_os = "android")]
fn acquire_wake_lock_helper(app: &tauri::AppHandle) {
    if let Err(e) = tauri_plugin_vcp_mobile::system::acquire_wake_lock(app.clone()) {
        log::warn!("[Distributed] Failed to acquire native wake lock: {}", e);
    }
}

#[cfg(target_os = "android")]
fn release_wake_lock_helper(app: &tauri::AppHandle) {
    if let Err(e) = tauri_plugin_vcp_mobile::system::release_wake_lock(app.clone()) {
        log::warn!("[Distributed] Failed to release native wake lock: {}", e);
    }
}

#[cfg(not(target_os = "android"))]
fn acquire_wake_lock_helper(_app: &tauri::AppHandle) {}

#[cfg(not(target_os = "android"))]
fn release_wake_lock_helper(_app: &tauri::AppHandle) {}

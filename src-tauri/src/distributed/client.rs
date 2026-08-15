// distributed/client.rs
// WebSocket client for VCP Distributed Node
// Mirrors VCPChat/VCPDistributedServer/VCPDistributedServer.js (class DistributedServer).
// This transport stays generic: domain-specific execution is delegated through ToolRegistry.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{mpsc, oneshot, Mutex, OwnedSemaphorePermit, RwLock, Semaphore};
use tokio::task::JoinSet;
use tokio::time;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_util::sync::CancellationToken;

use super::tool_registry::{ToolExecutionContext, ToolRegistry};
use super::tools::distributed_operation_id;
use super::types::*;
use crate::vcp_modules::cli::runtime::MobileCliRuntimeState;

const WS_OPERATION_TIMEOUT: Duration = Duration::from_secs(5);
const WS_OUTBOUND_CAPACITY: usize = 64;
const MAX_INCOMING_MESSAGE_BYTES: usize = 512 * 1024;
const MAX_TOOL_ARGUMENT_BYTES: usize = 256 * 1024;
const MAX_VCP_CONTEXT_BYTES: usize = 128 * 1024;
const MAX_TOOL_REQUEST_ID_BYTES: usize = 128;
const MAX_TOOL_NAME_BYTES: usize = 128;
const MAX_IN_FLIGHT_TOOL_REQUESTS: usize = 8;
const REMEMBERED_TOOL_REQUEST_IDS: usize = 1024;
const VCP_MOBILE_CLI_TOOL_NAME: &str = "VCPMobileCLI";

fn inbound_websocket_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(MAX_INCOMING_MESSAGE_BYTES))
        .max_frame_size(Some(MAX_INCOMING_MESSAGE_BYTES))
}

struct OutboundFrame {
    message: WsMessage,
    completion: oneshot::Sender<Result<(), String>>,
}

type WsSender = mpsc::Sender<OutboundFrame>;

struct SessionTaskTracker {
    cancel_token: CancellationToken,
    closed: AtomicBool,
    tasks: Mutex<JoinSet<()>>,
    tool_requests: ToolRequestGate,
}

impl SessionTaskTracker {
    fn new(cancel_token: CancellationToken) -> Self {
        Self {
            cancel_token,
            closed: AtomicBool::new(false),
            tasks: Mutex::new(JoinSet::new()),
            tool_requests: ToolRequestGate::new(),
        }
    }

    async fn spawn<F>(&self, future: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        if self.closed.load(Ordering::SeqCst) {
            return;
        }

        let cancel_token = self.cancel_token.clone();
        let mut tasks = self.tasks.lock().await;
        if self.closed.load(Ordering::SeqCst) {
            return;
        }
        tasks.spawn(async move {
            tokio::select! {
                biased;
                _ = cancel_token.cancelled() => {}
                _ = future => {}
            }
        });
    }

    async fn close_and_wait(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.cancel_token.cancel();
        let mut tasks = self.tasks.lock().await;
        while let Some(result) = tasks.join_next().await {
            if let Err(error) = result {
                log::warn!("[Distributed] Session child task failed: {}", error);
            }
        }
    }
}

struct ToolRequestGateState {
    seen: HashMap<String, String>,
    order: VecDeque<String>,
}

struct ToolRequestGate {
    permits: Arc<Semaphore>,
    state: Mutex<ToolRequestGateState>,
}

struct ToolRequestPermit {
    _permit: OwnedSemaphorePermit,
}

impl ToolRequestGate {
    fn new() -> Self {
        Self {
            permits: Arc::new(Semaphore::new(MAX_IN_FLIGHT_TOOL_REQUESTS)),
            state: Mutex::new(ToolRequestGateState {
                seen: HashMap::new(),
                order: VecDeque::new(),
            }),
        }
    }

    async fn try_claim(
        &self,
        request_id: &str,
        tool_name: &str,
        tool_args: &Value,
        vcp_context: Option<&Value>,
    ) -> Result<ToolRequestPermit, String> {
        if request_id.is_empty() || request_id.len() > MAX_TOOL_REQUEST_ID_BYTES {
            return Err("requestId is empty or exceeds 128 bytes".to_string());
        }
        if tool_name.is_empty() || tool_name.len() > MAX_TOOL_NAME_BYTES {
            return Err("toolName is empty or exceeds 128 bytes".to_string());
        }
        let encoded_args = serde_json::to_vec(tool_args)
            .map_err(|error| format!("toolArgs serialization failed: {}", error))?;
        if encoded_args.len() > MAX_TOOL_ARGUMENT_BYTES {
            return Err("toolArgs exceeds 256KB budget".to_string());
        }
        let request_digest = tool_request_digest(tool_name, &encoded_args, vcp_context)?;

        let mut state = self.state.lock().await;
        if let Some(existing_digest) = state.seen.get(request_id) {
            if existing_digest != &request_digest {
                return Err("requestId replay payload/context conflict".to_string());
            }
            if tool_name != VCP_MOBILE_CLI_TOOL_NAME {
                return Err("duplicate requestId rejected".to_string());
            }
        }
        let permit = self
            .permits
            .clone()
            .try_acquire_owned()
            .map_err(|_| "too many in-flight tool requests".to_string())?;

        if !state.seen.contains_key(request_id) {
            while state.order.len() >= REMEMBERED_TOOL_REQUEST_IDS {
                if let Some(expired) = state.order.pop_front() {
                    state.seen.remove(&expired);
                }
            }
            state.seen.insert(request_id.to_string(), request_digest);
            state.order.push_back(request_id.to_string());
        }
        Ok(ToolRequestPermit { _permit: permit })
    }
}

fn tool_request_digest(
    tool_name: &str,
    encoded_args: &[u8],
    vcp_context: Option<&Value>,
) -> Result<String, String> {
    let encoded_context = vcp_context
        .map(serde_json::to_vec)
        .transpose()
        .map_err(|error| format!("_vcpContext serialization failed: {error}"))?;
    let mut hasher = Sha256::new();
    hasher.update((tool_name.len() as u64).to_be_bytes());
    hasher.update(tool_name.as_bytes());
    hasher.update((encoded_args.len() as u64).to_be_bytes());
    hasher.update(encoded_args);
    if let Some(context) = encoded_context {
        hasher.update([1]);
        hasher.update((context.len() as u64).to_be_bytes());
        hasher.update(context);
    } else {
        hasher.update([0]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn validate_vcp_context(vcp_context: Option<&Value>) -> Result<(), String> {
    if let Some(context) = vcp_context {
        let encoded = serde_json::to_vec(context)
            .map_err(|error| format!("_vcpContext serialization failed: {error}"))?;
        if encoded.len() > MAX_VCP_CONTEXT_BYTES {
            return Err("_vcpContext exceeds 128KB budget".to_string());
        }
    }
    Ok(())
}

fn current_tool_execution_context(
    status: &DistributedStatus,
    expected_epoch: u64,
    request_id: &str,
    remote_identity: &str,
    vcp_context: Option<Value>,
) -> Result<ToolExecutionContext, String> {
    if status.session_id != expected_epoch
        || status.state != ConnectionState::Connected
        || !status.connected
    {
        return Err(
            "remote_disconnected: tool request does not belong to the current online epoch"
                .to_string(),
        );
    }
    let server_id = status
        .server_id
        .as_deref()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            "remote_disconnected: current connection has no server identity".to_string()
        })?;
    let client_id = status
        .client_id
        .as_deref()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            "remote_disconnected: current connection has no client identity".to_string()
        })?;
    Ok(ToolExecutionContext {
        request_id: request_id.to_string(),
        remote_identity: remote_identity.to_string(),
        connection_epoch: expected_epoch,
        server_id: server_id.to_string(),
        client_id: client_id.to_string(),
        vcp_context,
    })
}

fn spawn_durable_tool_execution<F>(future: F) -> oneshot::Receiver<OutgoingMessage>
where
    F: Future<Output = OutgoingMessage> + Send + 'static,
{
    let (result_tx, result_rx) = oneshot::channel();
    tokio::spawn(async move {
        let result = future.await;
        let _ = result_tx.send(result);
    });
    result_rx
}

struct WakeLockLease {
    app: AppHandle,
    tag: String,
}

impl WakeLockLease {
    fn acquire(app: &AppHandle, tag: String) -> Self {
        acquire_wake_lock_helper(app, &tag);
        Self {
            app: app.clone(),
            tag,
        }
    }
}

impl Drop for WakeLockLease {
    fn drop(&mut self) {
        release_wake_lock_helper(&self.app, &self.tag);
    }
}

async fn with_scoped_guard<G, F>(guard: G, future: F) -> F::Output
where
    F: Future,
{
    let _guard = guard;
    future.await
}

fn normalize_remote_ws_url(raw_url: &str) -> Result<String, String> {
    let mut url = url::Url::parse(raw_url.trim())
        .map_err(|error| format!("invalid distributed WebSocket URL: {error}"))?;
    if !matches!(url.scheme(), "ws" | "wss") {
        return Err("distributed URL must use ws:// or wss://".to_string());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("distributed URL must not contain userinfo".to_string());
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err("distributed URL must not contain query or fragment".to_string());
    }
    let normalized_path = url.path().trim_end_matches('/').to_string();
    url.set_path(&normalized_path);
    Ok(url.as_str().trim_end_matches('/').to_string())
}

fn stable_remote_identity(ws_url: &str, device_name: &str) -> Result<String, String> {
    let normalized_url = normalize_remote_ws_url(ws_url)?;
    let normalized_device = device_name.trim();
    if normalized_device.is_empty() {
        return Err("distributed device name must not be empty".to_string());
    }
    let mut hasher = Sha256::new();
    hasher.update(normalized_url.as_bytes());
    hasher.update([0]);
    hasher.update(normalized_device.as_bytes());
    Ok(hex::encode(hasher.finalize()))
}

/// Immutable configuration for a single connection lifecycle.
struct ConnectionConfig {
    ws_url: String,
    vcp_key: String,
    device_name: String,
    remote_identity: String,
}

/// Runtime context for a single connection lifecycle (channel receivers).
struct SessionContext {
    status: Arc<RwLock<DistributedStatus>>,
    registry: Arc<ToolRegistry>,
    re_register_rx: tokio::sync::mpsc::Receiver<()>,
    reconnect_rx: tokio::sync::mpsc::Receiver<()>,
    session_id: u64,
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
    /// Serializes start/stop so a stop request cannot pass a start that has not
    /// installed its session handle yet.
    lifecycle: Mutex<()>,
    /// Active session handle — None when disconnected.
    session: Mutex<Option<ConnectionSession>>,
}

impl DistributedClient {
    fn clear_registered_tools_for_session(status: &mut DistributedStatus, session_id: u64) {
        if status.session_id == session_id {
            status.registered_tools = 0;
        }
    }

    pub fn new() -> Self {
        Self {
            status: Arc::new(RwLock::new(DistributedStatus::default())),
            lifecycle: Mutex::new(()),
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
        let _lifecycle_guard = self.lifecycle.lock().await;
        let remote_identity = stable_remote_identity(&ws_url, &device_name)?;

        // Prevent duplicate activation using ConnectionState.
        let next_session_id = {
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
            s.registered_tools = 0;
            s.session_id += 1;
            s.session_id
        };

        // Gracefully shut down any existing session before creating a new one.
        let old_session = { self.session.lock().await.take() };
        if let Some(session) = old_session {
            session.cancel_token.cancel();
            let _ = session.task_handle.await;
        }

        // Check if state changed during startup setup (e.g. stop requested during await)
        {
            let s = self.status.read().await;
            if s.state != ConnectionState::Connecting || s.session_id != next_session_id {
                log::info!("[Distributed] State changed during start setup, aborting start.");
                return Ok(());
            }
        }

        // Create fresh channels and cancellation token — no state reuse from previous cycles.
        let cancel_token = CancellationToken::new();
        let (re_register_tx, re_register_rx) = tokio::sync::mpsc::channel(1);
        let (reconnect_tx, reconnect_rx) = tokio::sync::mpsc::channel(1);
        let status = self.status.clone();

        Self::emit_status(&app, &status).await;

        let config = ConnectionConfig {
            ws_url,
            vcp_key,
            device_name,
            remote_identity,
        };
        let ctx = SessionContext {
            status,
            registry,
            re_register_rx,
            reconnect_rx,
            session_id: next_session_id,
        };
        let loop_token = cancel_token.clone();

        // Keep the generation check immediately adjacent to the final commit.
        // The lifecycle mutex already excludes stop(), while this check also
        // protects against any future owner-aware transition added here.
        {
            let s = self.status.read().await;
            if s.state != ConnectionState::Connecting || s.session_id != next_session_id {
                cancel_token.cancel();
                log::info!("[Distributed] Session owner changed before install, aborting start.");
                return Ok(());
            }
        }

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
    pub async fn stop(&self, app: &AppHandle) {
        let _lifecycle_guard = self.lifecycle.lock().await;

        let stop_generation = {
            let mut s = self.status.write().await;
            let was_running = s.state != ConnectionState::Disconnected || s.connected;
            s.session_id = s.session_id.wrapping_add(1);
            let stop_generation = s.session_id;

            if was_running {
                s.state = ConnectionState::Disconnecting;
            } else {
                log::info!(
                    "[Distributed] Already disconnected; advancing session generation only."
                );
                s.state = ConnectionState::Disconnected;
            }
            s.connected = false;
            s.server_id = None;
            s.client_id = None;
            s.registered_tools = 0;
            stop_generation
        };

        // Take the session out and gracefully shut it down.
        let old_session = { self.session.lock().await.take() };
        if let Some(session) = old_session {
            session.cancel_token.cancel();
            let _ = session.task_handle.await;
        }

        // Safety net: ensure final Disconnected state if loop didn't clean up properly.
        {
            let mut s = self.status.write().await;
            if s.session_id == stop_generation {
                s.state = ConnectionState::Disconnected;
                s.connected = false;
                s.server_id = None;
                s.client_id = None;
                s.registered_tools = 0;
            }
        }
        Self::emit_status(app, &self.status).await;
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
            let _ = session.re_register_tx.try_send(());
        }
    }

    /// Trigger immediate reconnection.
    pub async fn trigger_reconnect(&self) {
        if let Some(session) = self.session.lock().await.as_ref() {
            let _ = session.reconnect_tx.try_send(());
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
        let mut re_register_rx = ctx.re_register_rx;
        let mut reconnect_rx = ctx.reconnect_rx;
        let status = ctx.status;
        let registry = ctx.registry;
        let session_id = ctx.session_id;

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
            let connect_result = with_scoped_guard(
                WakeLockLease::acquire(&app, "distributed:connect".to_string()),
                async {
                    tokio::select! {
                        result = tokio_tungstenite::connect_async_with_config(
                            &connection_url,
                            Some(inbound_websocket_config()),
                            false,
                        ) => Some(result),
                        _ = cancel_token.cancelled() => None,
                    }
                },
            )
            .await;

            match connect_result {
                Some(Ok((ws_stream, _response))) => {
                    log::info!("[Distributed] WebSocket connected.");
                    reconnect_interval = Duration::from_secs(5); // Reset backoff on success.

                    // Run the session until it ends.
                    let exit_reason = Self::run_session(
                        &app,
                        ws_stream,
                        &config.device_name,
                        &config.remote_identity,
                        &cancel_token,
                        &status,
                        &registry,
                        &mut re_register_rx,
                        session_id,
                    )
                    .await;

                    // Session ended — update status.
                    {
                        let mut s = status.write().await;
                        if s.session_id == session_id {
                            if s.state != ConnectionState::Disconnecting {
                                s.state = ConnectionState::Connecting;
                            }
                            s.connected = false;
                            s.server_id = None;
                            s.client_id = None;
                            Self::clear_registered_tools_for_session(&mut s, session_id);
                            s.last_error = Some(exit_reason);
                        }
                    }
                    Self::emit_status(&app, &status).await;
                }
                Some(Err(e)) => {
                    log::warn!("[Distributed] Connection failed: {}", e);
                    {
                        let mut s = status.write().await;
                        if s.session_id == session_id {
                            if s.state != ConnectionState::Disconnecting {
                                s.state = ConnectionState::Connecting;
                            }
                            s.connected = false;
                            Self::clear_registered_tools_for_session(&mut s, session_id);
                            s.last_error = Some(format!("Connection failed: {}", e));
                        }
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
            if s.session_id == session_id {
                s.state = ConnectionState::Disconnected;
                s.connected = false;
                s.server_id = None;
                s.client_id = None;
                Self::clear_registered_tools_for_session(&mut s, session_id);
            }
        }
        Self::emit_status(&app, &status).await;
        log::info!("[Distributed] Connection loop exited.");
    }

    // ================================================================
    // Session handler — processes one WS connection lifetime
    // ================================================================

    #[allow(clippy::too_many_arguments)]
    async fn run_session(
        app: &AppHandle,
        ws_stream: tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        device_name: &str,
        remote_identity: &str,
        cancel_token: &CancellationToken,
        status: &Arc<RwLock<DistributedStatus>>,
        registry: &Arc<ToolRegistry>,
        re_register_rx: &mut tokio::sync::mpsc::Receiver<()>,
        session_id: u64,
    ) -> String {
        use tokio_tungstenite::tungstenite::Message;

        #[cfg(target_os = "android")]
        if let Err(e) = tauri_plugin_vcp_mobile::system::start_sensor_collection(app.clone()) {
            log::warn!(
                "[Distributed] Failed to start native sensor collection: {}",
                e
            );
        }

        let session_cancel = cancel_token.child_token();
        let child_tracker = Arc::new(SessionTaskTracker::new(session_cancel.clone()));
        let (mut ws_sink, mut ws_rx) = ws_stream.split();
        let (ws_tx, mut outbound_rx) = mpsc::channel::<OutboundFrame>(WS_OUTBOUND_CAPACITY);

        let writer_cancel = session_cancel.clone();
        child_tracker
            .spawn(async move {
                loop {
                    let frame = tokio::select! {
                        biased;
                        _ = writer_cancel.cancelled() => break,
                        frame = outbound_rx.recv() => match frame {
                            Some(frame) => frame,
                            None => break,
                        },
                    };

                    let send_result = match time::timeout(
                        WS_OPERATION_TIMEOUT,
                        ws_sink.send(frame.message),
                    )
                    .await
                    {
                        Ok(Ok(())) => Ok(()),
                        Ok(Err(error)) => Err(format!("WebSocket send failed: {}", error)),
                        Err(_) => Err("WebSocket send timed out".to_string()),
                    };
                    let failed = send_result.is_err();
                    if let Err(error) = &send_result {
                        log::warn!("[Distributed] {}", error);
                    }
                    let _ = frame.completion.send(send_result);
                    if failed {
                        writer_cancel.cancel();
                        break;
                    }
                }
            })
            .await;

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
                                remote_identity,
                                &ws_tx,
                                status,
                                registry,
                                &child_tracker,
                                session_id,
                            ).await;
                        }
                        Some(Ok(Message::Ping(data))) => {
                            if let Err(error) = Self::send_ws_frame(&ws_tx, Message::Pong(data)).await {
                                log::warn!("[Distributed] Failed to send pong: {}", error);
                            }
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
                opt = re_register_rx.recv() => {
                    if opt.is_some() {
                        log::info!("[Distributed] Re-registering tools due to configuration change.");
                        Self::register_tools(
                            app,
                            device_name,
                            &ws_tx,
                            registry,
                            status,
                            session_id,
                        )
                        .await;
                        Self::emit_status_with_app(app, status).await;
                    }
                }

                // --- Periodic static placeholder push ---
                _ = placeholder_interval.tick() => {
                    Self::push_static_placeholders(
                        app,
                        device_name,
                        &ws_tx,
                        registry,
                        &child_tracker,
                    ).await;
                }

                // --- Cancellation signal ---
                _ = session_cancel.cancelled() => {
                    log::info!("[Distributed] Shutdown signal received, closing session.");
                    exit_reason = "Client requested shutdown".to_string();
                    break;
                }
            }
        }

        if !session_cancel.is_cancelled() {
            let _ = Self::send_ws_frame(&ws_tx, Message::Close(None)).await;
        }
        child_tracker.close_and_wait().await;

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

    #[allow(clippy::too_many_arguments)]
    async fn handle_incoming(
        app: &AppHandle,
        text: &str,
        device_name: &str,
        remote_identity: &str,
        ws_tx: &WsSender,
        status: &Arc<RwLock<DistributedStatus>>,
        registry: &Arc<ToolRegistry>,
        child_tracker: &Arc<SessionTaskTracker>,
        session_id: u64,
    ) {
        if text.len() > MAX_INCOMING_MESSAGE_BYTES {
            log::warn!(
                "[Distributed] Incoming message exceeds {} byte budget; ignored.",
                MAX_INCOMING_MESSAGE_BYTES
            );
            return;
        }
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

                // Update status only while this session still owns the generation.
                let accepted = {
                    let mut s = status.write().await;
                    Self::commit_connection_ack(
                        &mut s,
                        session_id,
                        server_id.clone(),
                        client_id.clone(),
                    )
                };
                if !accepted {
                    log::info!(
                        "[Distributed] Ignoring stale connection ACK for session {}.",
                        session_id
                    );
                    return;
                }
                Self::emit_status_with_app(app, status).await;

                // Register tools — mirrors registerTools()
                Self::register_tools(app, device_name, ws_tx, registry, status, session_id).await;
                Self::emit_status_with_app(app, status).await;

                // Report IP — mirrors reportIPAddress()
                let device_name_clone = device_name.to_string();
                let ws_tx_clone = ws_tx.clone();
                child_tracker
                    .spawn(async move {
                        Self::report_ip(&device_name_clone, &ws_tx_clone).await;
                    })
                    .await;

                // Initial static placeholder push (2s delay in VCPChat, do it immediately here)
                Self::push_static_placeholders(app, device_name, ws_tx, registry, child_tracker)
                    .await;
            }

            IncomingMessage::ExecuteTool {
                request_id,
                tool_name,
                tool_args,
                vcp_context,
            } => {
                if let Err(error) = validate_vcp_context(vcp_context.as_ref()) {
                    Self::send_tool_rejection(ws_tx, &request_id, error).await;
                    return;
                }
                let execution_context = {
                    let current = status.read().await;
                    match current_tool_execution_context(
                        &current,
                        session_id,
                        &request_id,
                        remote_identity,
                        vcp_context,
                    ) {
                        Ok(context) => context,
                        Err(error) => {
                            drop(current);
                            Self::send_tool_rejection(ws_tx, &request_id, error).await;
                            return;
                        }
                    }
                };
                let request_permit = match child_tracker
                    .tool_requests
                    .try_claim(
                        &request_id,
                        &tool_name,
                        &tool_args,
                        execution_context.vcp_context.as_ref(),
                    )
                    .await
                {
                    Ok(permit) => permit,
                    Err(error) => {
                        Self::send_tool_rejection(ws_tx, &request_id, error).await;
                        return;
                    }
                };

                log::info!(
                    "[Distributed] Execute tool request: {} (requestId={})",
                    tool_name,
                    request_id
                );

                // Execute and return result asynchronously to avoid blocking the main WS receiver loop.
                let app_clone = app.clone();
                let ws_tx_clone = ws_tx.clone();
                let registry_clone = registry.clone();
                let request_id_clone = request_id.clone();
                let tool_name_clone = tool_name.clone();

                if tool_name == VCP_MOBILE_CLI_TOOL_NAME {
                    let result_rx = spawn_durable_tool_execution(async move {
                        let _request_permit = request_permit;
                        let tag = format!(
                            "distributed:tool:{}:{}",
                            request_id_clone,
                            uuid::Uuid::new_v4()
                        );
                        let _lease = WakeLockLease::acquire(&app_clone, tag);
                        Self::execute_tool(
                            &app_clone,
                            &request_id_clone,
                            &tool_name_clone,
                            tool_args,
                            &registry_clone,
                            Some(execution_context),
                        )
                        .await
                    });
                    child_tracker
                        .spawn(async move {
                            if let Ok(response) = result_rx.await {
                                if let Err(error) =
                                    Self::send_message(&ws_tx_clone, &response).await
                                {
                                    log::warn!(
                                        "[Distributed] Failed to return durable tool result: {}",
                                        error
                                    );
                                }
                            }
                        })
                        .await;
                    return;
                }

                child_tracker
                    .spawn(async move {
                        let _request_permit = request_permit;
                        let tag = format!(
                            "distributed:tool:{}:{}",
                            request_id_clone,
                            uuid::Uuid::new_v4()
                        );
                        let _lease = WakeLockLease::acquire(&app_clone, tag);
                        let response = Self::execute_tool(
                            &app_clone,
                            &request_id_clone,
                            &tool_name_clone,
                            tool_args,
                            &registry_clone,
                            Some(execution_context),
                        )
                        .await;
                        if let Err(error) = Self::send_message(&ws_tx_clone, &response).await {
                            log::warn!("[Distributed] Failed to return tool result: {}", error);
                        }
                    })
                    .await;
            }

            IncomingMessage::CancelTool { request_id } => {
                log::info!(
                    "[Distributed] Cancel tool request: (requestId={})",
                    request_id
                );
                if let Some(runtime) = app.try_state::<MobileCliRuntimeState>() {
                    let operation_id = distributed_operation_id(remote_identity, &request_id);
                    if let Err(error) = runtime.cancel_operation(app, &operation_id).await {
                        log::warn!(
                            "[VCPMobileCLI] cancel_tool failed for {}: {}",
                            request_id,
                            error
                        );
                    }
                }
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
        app: &AppHandle,
        device_name: &str,
        ws_tx: &WsSender,
        registry: &Arc<ToolRegistry>,
        status: &Arc<RwLock<DistributedStatus>>,
        session_id: u64,
    ) {
        let tools = registry.get_registration_manifests(app).await;
        Self::publish_tool_registration(device_name, tools, ws_tx, status, session_id).await;
    }

    async fn publish_tool_registration(
        device_name: &str,
        tools: Vec<Box<serde_json::value::RawValue>>,
        ws_tx: &WsSender,
        status: &Arc<RwLock<DistributedStatus>>,
        session_id: u64,
    ) {
        let count = tools.len();
        let msg = OutgoingMessage::RegisterTools {
            server_name: device_name.to_string(),
            tools,
            capabilities: ServerCapabilities { cancel_tool: true },
        };
        if let Err(error) = Self::send_message(ws_tx, &msg).await {
            log::warn!("[Distributed] Failed to register tools: {}", error);
            return;
        }

        // This count means the current local WebSocket writer accepted the frame. The protocol
        // has no server-side register acknowledgement, so it must not be presented as such.
        {
            let mut s = status.write().await;
            if s.session_id == session_id {
                s.registered_tools = count;
            }
        }

        if count == 0 {
            log::info!("[Distributed] Published an empty tool manifest set.");
        } else {
            log::info!(
                "[Distributed] Published {} tool manifests to the current WebSocket writer.",
                count
            );
        }
    }

    /// Report IP addresses to the main server.
    /// VCPChat ref: reportIPAddress() line 310-347
    async fn report_ip(device_name: &str, ws_tx: &WsSender) {
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
        match Self::send_message(ws_tx, &msg).await {
            Ok(()) => log::info!("[Distributed] IP report sent."),
            Err(error) => log::warn!("[Distributed] Failed to report IP: {}", error),
        }
    }

    /// Push static placeholder values asynchronously to avoid blocking.
    /// VCPChat ref: pushStaticPlaceholderValues() line 374-398
    async fn push_static_placeholders(
        app: &AppHandle,
        device_name: &str,
        ws_tx: &WsSender,
        registry: &Arc<ToolRegistry>,
        child_tracker: &Arc<SessionTaskTracker>,
    ) {
        let app_clone = app.clone();
        let device_name_clone = device_name.to_string();
        let ws_tx_clone = ws_tx.clone();
        let registry_clone = registry.clone();

        child_tracker
            .spawn(async move {
                let tag = format!("distributed:placeholder_push:{}", uuid::Uuid::new_v4());
                let _lease = WakeLockLease::acquire(&app_clone, tag);
                let placeholders = registry_clone.get_all_placeholder_values(&app_clone);
                if !placeholders.is_empty() {
                    let msg = OutgoingMessage::UpdateStaticPlaceholders {
                        server_name: device_name_clone,
                        placeholders,
                    };
                    if let Err(error) = Self::send_message(&ws_tx_clone, &msg).await {
                        log::warn!(
                            "[Distributed] Failed to push static placeholders: {}",
                            error
                        );
                    }
                }
            })
            .await;
    }

    /// Execute a tool and return the result message.
    /// VCPChat ref: handleToolExecutionRequest() line 428-649
    async fn send_tool_rejection(ws_tx: &WsSender, request_id: &str, error: String) {
        let response = OutgoingMessage::ToolResult {
            request_id: request_id.chars().take(MAX_TOOL_REQUEST_ID_BYTES).collect(),
            status: "error".to_string(),
            result: None,
            error: Some(error),
        };
        if let Err(send_error) = Self::send_message(ws_tx, &response).await {
            log::warn!(
                "[Distributed] Failed to reject tool request: {}",
                send_error
            );
        }
    }

    async fn execute_tool(
        app: &AppHandle,
        request_id: &str,
        tool_name: &str,
        tool_args: Value,
        registry: &Arc<ToolRegistry>,
        context: Option<ToolExecutionContext>,
    ) -> OutgoingMessage {
        match registry
            .execute_with_context(tool_name, tool_args, app, context)
            .await
        {
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

    fn commit_connection_ack(
        status: &mut DistributedStatus,
        session_id: u64,
        server_id: String,
        client_id: String,
    ) -> bool {
        if status.session_id != session_id {
            return false;
        }

        status.state = ConnectionState::Connected;
        status.connected = true;
        status.server_id = Some(server_id);
        status.client_id = Some(client_id);
        status.last_error = None;
        true
    }

    /// Serialize and send a message over WebSocket.
    async fn send_message(ws_tx: &WsSender, msg: &OutgoingMessage) -> Result<(), String> {
        let json = serde_json::to_string(msg)
            .map_err(|error| format!("Failed to serialize message: {}", error))?;
        Self::send_ws_frame(ws_tx, WsMessage::Text(json.into())).await
    }

    async fn send_ws_frame(ws_tx: &WsSender, message: WsMessage) -> Result<(), String> {
        Self::send_ws_frame_with_timeout(ws_tx, message, WS_OPERATION_TIMEOUT).await
    }

    async fn send_ws_frame_with_timeout(
        ws_tx: &WsSender,
        message: WsMessage,
        deadline: Duration,
    ) -> Result<(), String> {
        time::timeout(deadline, async {
            let (completion, completed) = oneshot::channel();
            ws_tx
                .send(OutboundFrame {
                    message,
                    completion,
                })
                .await
                .map_err(|_| "WebSocket writer is closed".to_string())?;
            completed
                .await
                .map_err(|_| "WebSocket writer stopped before send completed".to_string())?
        })
        .await
        .map_err(|_| "WebSocket outbound operation timed out".to_string())?
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
fn acquire_wake_lock_helper(app: &tauri::AppHandle, tag: &str) {
    if let Err(e) = tauri_plugin_vcp_mobile::stream::acquire_foreground_inner(
        app,
        tag,
        10, // priority = PRIORITY_DISTRIBUTED
        "[分布式连接]",
        false, // screen_keep_on = false
    ) {
        log::warn!(
            "[Distributed] Failed to acquire native wake lock with tag {}: {}",
            tag,
            e
        );
    }
}

#[cfg(target_os = "android")]
fn release_wake_lock_helper(app: &tauri::AppHandle, tag: &str) {
    if let Err(e) = tauri_plugin_vcp_mobile::stream::release_foreground_inner(app, tag) {
        log::warn!(
            "[Distributed] Failed to release native wake lock with tag {}: {}",
            tag,
            e
        );
    }
}

#[cfg(not(target_os = "android"))]
fn acquire_wake_lock_helper(_app: &tauri::AppHandle, _tag: &str) {}

#[cfg(not(target_os = "android"))]
fn release_wake_lock_helper(_app: &tauri::AppHandle, _tag: &str) {}

#[cfg(test)]
mod tests {
    use super::*;

    struct DropSignal(Option<oneshot::Sender<()>>);

    impl Drop for DropSignal {
        fn drop(&mut self) {
            if let Some(sender) = self.0.take() {
                let _ = sender.send(());
            }
        }
    }

    #[tokio::test]
    async fn lifecycle_mutex_serializes_transitions() {
        let client = Arc::new(DistributedClient::new());
        let guard = client.lifecycle.lock().await;
        let waiter_client = client.clone();
        let waiter = tokio::spawn(async move {
            let _guard = waiter_client.lifecycle.lock().await;
        });

        tokio::task::yield_now().await;
        assert!(!waiter.is_finished());

        drop(guard);
        tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("queued lifecycle transition should resume")
            .expect("queued lifecycle transition should not panic");
    }

    #[test]
    fn advanced_generation_rejects_late_connection_ack() {
        let mut status = DistributedStatus {
            state: ConnectionState::Disconnecting,
            session_id: 2,
            ..DistributedStatus::default()
        };

        assert!(!DistributedClient::commit_connection_ack(
            &mut status,
            1,
            "old-server".to_string(),
            "old-client".to_string(),
        ));
        assert_eq!(status.state, ConnectionState::Disconnecting);
        assert!(!status.connected);
        assert_eq!(status.session_id, 2);
    }

    #[tokio::test]
    async fn session_tracker_cancels_and_joins_children() {
        let tracker = Arc::new(SessionTaskTracker::new(CancellationToken::new()));
        let (started_tx, started_rx) = oneshot::channel();
        let (dropped_tx, dropped_rx) = oneshot::channel();

        tracker
            .spawn(async move {
                let _drop_signal = DropSignal(Some(dropped_tx));
                let _ = started_tx.send(());
                std::future::pending::<()>().await;
            })
            .await;
        started_rx.await.expect("child should start");

        time::timeout(Duration::from_secs(1), tracker.close_and_wait())
            .await
            .expect("tracker shutdown should be bounded");
        time::timeout(Duration::from_secs(1), dropped_rx)
            .await
            .expect("cancelled child should be dropped")
            .expect("drop signal should be delivered");
    }

    #[tokio::test]
    async fn session_shutdown_drops_waiter_but_not_durable_cli_execution() {
        let tracker = Arc::new(SessionTaskTracker::new(CancellationToken::new()));
        let (release_tx, release_rx) = oneshot::channel();
        let (completed_tx, completed_rx) = oneshot::channel();
        let result_rx = spawn_durable_tool_execution(async move {
            let _ = release_rx.await;
            let _ = completed_tx.send(());
            OutgoingMessage::ToolResult {
                request_id: "request-1".to_string(),
                status: "success".to_string(),
                result: Some(Value::Null),
                error: None,
            }
        });
        tracker
            .spawn(async move {
                let _ = result_rx.await;
            })
            .await;

        time::timeout(Duration::from_secs(1), tracker.close_and_wait())
            .await
            .expect("session waiter shutdown should be bounded");
        release_tx
            .send(())
            .expect("detached execution should still own its receiver");
        time::timeout(Duration::from_secs(1), completed_rx)
            .await
            .expect("durable execution should outlive session waiter")
            .expect("durable execution completion signal");
    }

    #[tokio::test]
    async fn scoped_guard_is_released_when_connect_await_finishes() {
        let (dropped_tx, dropped_rx) = oneshot::channel();

        let result = with_scoped_guard(DropSignal(Some(dropped_tx)), async { 42 }).await;

        assert_eq!(result, 42);
        dropped_rx
            .await
            .expect("connect lease must be released before session processing begins");
    }

    #[tokio::test]
    async fn bounded_outbound_queue_times_out_instead_of_waiting_forever() {
        let (ws_tx, mut ws_rx) = mpsc::channel(1);
        let (completion, _completed) = oneshot::channel();
        ws_tx
            .send(OutboundFrame {
                message: WsMessage::Ping(Vec::new().into()),
                completion,
            })
            .await
            .expect("first frame should fill the queue");

        let result = DistributedClient::send_ws_frame_with_timeout(
            &ws_tx,
            WsMessage::Ping(Vec::new().into()),
            Duration::from_millis(10),
        )
        .await;

        assert_eq!(
            result,
            Err("WebSocket outbound operation timed out".to_string())
        );
        assert!(ws_rx.try_recv().is_ok());
    }

    #[test]
    fn websocket_protocol_budget_matches_incoming_handler_budget() {
        let config = inbound_websocket_config();

        assert_eq!(config.max_message_size, Some(MAX_INCOMING_MESSAGE_BYTES));
        assert_eq!(config.max_frame_size, Some(MAX_INCOMING_MESSAGE_BYTES));
    }

    #[test]
    fn empty_registration_is_a_wire_message_that_withdraws_all_tools() {
        let message = OutgoingMessage::RegisterTools {
            server_name: "mobile".to_string(),
            tools: vec![],
            capabilities: ServerCapabilities { cancel_tool: true },
        };
        assert_eq!(
            serde_json::to_value(message).unwrap(),
            serde_json::json!({
                "type": "register_tools",
                "data": {
                    "serverName": "mobile",
                    "tools": [],
                    "capabilities": { "cancelTool": true }
                }
            })
        );
    }

    #[tokio::test]
    async fn repeated_empty_registration_withdraws_tools_and_keeps_count_zero() {
        let status = Arc::new(RwLock::new(DistributedStatus {
            registered_tools: 4,
            session_id: 9,
            ..DistributedStatus::default()
        }));

        for _ in 0..2 {
            let (ws_tx, mut ws_rx) = mpsc::channel::<OutboundFrame>(1);
            let receiver = tokio::spawn(async move {
                let frame = ws_rx.recv().await.expect("registration frame");
                let value = match frame.message {
                    WsMessage::Text(text) => serde_json::from_str::<Value>(&text).unwrap(),
                    other => panic!("unexpected registration frame: {other:?}"),
                };
                frame.completion.send(Ok(())).unwrap();
                value
            });

            DistributedClient::publish_tool_registration("mobile", vec![], &ws_tx, &status, 9)
                .await;
            let value = receiver.await.unwrap();
            assert_eq!(value["data"]["tools"], serde_json::json!([]));
            assert_eq!(status.read().await.registered_tools, 0);
        }
    }

    #[test]
    fn registered_tool_count_clear_is_generation_guarded() {
        let mut status = DistributedStatus {
            state: ConnectionState::Connected,
            connected: true,
            server_id: Some("server".to_string()),
            client_id: Some("client".to_string()),
            registered_tools: 3,
            session_id: 7,
            ..DistributedStatus::default()
        };

        DistributedClient::clear_registered_tools_for_session(&mut status, 6);
        assert_eq!(status.registered_tools, 3);

        DistributedClient::clear_registered_tools_for_session(&mut status, 7);

        assert_eq!(status.registered_tools, 0);
    }

    #[test]
    fn remote_identity_normalizes_endpoint_and_excludes_connection_epoch() {
        let first =
            stable_remote_identity("WS://Example.COM:80/vcp/", " Mobile ").expect("first identity");
        let second =
            stable_remote_identity("ws://example.com/vcp", "Mobile").expect("second identity");
        assert_eq!(first, second);
        assert_eq!(first.len(), 64);
        assert!(normalize_remote_ws_url("https://example.com").is_err());
        assert!(normalize_remote_ws_url("ws://example.com/path?node=other").is_err());
    }

    #[test]
    fn execution_context_requires_current_acknowledged_epoch() {
        let status = DistributedStatus {
            state: ConnectionState::Connected,
            connected: true,
            server_id: Some("server-1".to_string()),
            client_id: Some("client-1".to_string()),
            session_id: 7,
            ..DistributedStatus::default()
        };
        let context = current_tool_execution_context(
            &status,
            7,
            "request-1",
            "remote-fingerprint",
            Some(serde_json::json!({"river": "text"})),
        )
        .expect("current epoch context");
        assert_eq!(context.connection_epoch, 7);
        assert_eq!(context.server_id, "server-1");
        assert_eq!(context.client_id, "client-1");
        assert_eq!(context.remote_identity, "remote-fingerprint");
        assert!(current_tool_execution_context(
            &status,
            6,
            "request-1",
            "remote-fingerprint",
            None,
        )
        .is_err());
    }

    #[tokio::test]
    async fn websocket_protocol_rejects_oversized_message_before_handler() {
        use tokio_tungstenite::tungstenite::{error::CapacityError, Error, Message};
        use tokio_tungstenite::{tungstenite::protocol::Role, WebSocketStream};

        let (client_io, server_io) = tokio::io::duplex(MAX_INCOMING_MESSAGE_BYTES * 2);
        let (mut client, mut server) = tokio::join!(
            WebSocketStream::from_raw_socket(client_io, Role::Client, None),
            WebSocketStream::from_raw_socket(
                server_io,
                Role::Server,
                Some(inbound_websocket_config()),
            ),
        );

        let oversized = Message::Text("x".repeat(MAX_INCOMING_MESSAGE_BYTES + 1).into());
        let (send_result, receive_result) = tokio::join!(client.send(oversized), server.next());

        assert!(send_result.is_ok());
        assert!(matches!(
            receive_result,
            Some(Err(Error::Capacity(CapacityError::MessageTooLong { .. })))
        ));
    }

    #[tokio::test]
    async fn tool_request_gate_rejects_duplicates_oversized_args_and_fanout() {
        let gate = ToolRequestGate::new();
        let first = gate
            .try_claim(
                "request-1",
                "test_tool",
                &serde_json::json!({"value": 1}),
                None,
            )
            .await
            .expect("first request should be accepted");
        assert!(gate
            .try_claim(
                "request-1",
                "test_tool",
                &serde_json::json!({"value": 2}),
                None,
            )
            .await
            .is_err());

        let oversized = "x".repeat(MAX_TOOL_ARGUMENT_BYTES + 1);
        assert!(gate
            .try_claim(
                "oversized",
                "test_tool",
                &serde_json::json!({"value": oversized}),
                None,
            )
            .await
            .is_err());
        assert!(gate
            .try_claim(
                "bad-tool-name",
                &"x".repeat(MAX_TOOL_NAME_BYTES + 1),
                &serde_json::json!({}),
                None,
            )
            .await
            .is_err());

        let mut permits = vec![first];
        for index in 2..=MAX_IN_FLIGHT_TOOL_REQUESTS {
            permits.push(
                gate.try_claim(
                    &format!("request-{index}"),
                    "test_tool",
                    &serde_json::json!({"value": index}),
                    None,
                )
                .await
                .expect("request within in-flight budget should be accepted"),
            );
        }
        assert!(gate
            .try_claim(
                "request-overflow",
                "test_tool",
                &serde_json::json!({}),
                None,
            )
            .await
            .is_err());

        drop(permits);
        assert!(gate
            .try_claim(
                "request-after-release",
                "test_tool",
                &serde_json::json!({}),
                None,
            )
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn cli_exact_replay_is_admitted_but_changed_args_or_context_conflicts() {
        let gate = ToolRequestGate::new();
        let args = serde_json::json!({"action": "list"});
        let context = serde_json::json!({"river": "text"});
        let first = gate
            .try_claim("request-1", VCP_MOBILE_CLI_TOOL_NAME, &args, Some(&context))
            .await
            .expect("first CLI request");
        let replay = gate
            .try_claim("request-1", VCP_MOBILE_CLI_TOOL_NAME, &args, Some(&context))
            .await
            .expect("exact CLI replay reaches Runtime idempotency");
        assert!(gate
            .try_claim(
                "request-1",
                VCP_MOBILE_CLI_TOOL_NAME,
                &serde_json::json!({"action": "run", "command": "true"}),
                Some(&context),
            )
            .await
            .err()
            .expect("changed arguments must conflict")
            .contains("conflict"));
        assert!(gate
            .try_claim(
                "request-1",
                VCP_MOBILE_CLI_TOOL_NAME,
                &args,
                Some(&serde_json::json!({"river": "last:5"})),
            )
            .await
            .err()
            .expect("changed context must conflict")
            .contains("conflict"));
        drop((first, replay));

        let ordinary = ToolRequestGate::new();
        let _first = ordinary
            .try_claim("ordinary", "MobileClipboard", &args, None)
            .await
            .expect("first ordinary request");
        assert_eq!(
            ordinary
                .try_claim("ordinary", "MobileClipboard", &args, None)
                .await
                .err()
                .expect("ordinary duplicate must be rejected"),
            "duplicate requestId rejected"
        );
    }

    #[test]
    fn top_level_vcp_context_has_a_strict_budget() {
        assert!(validate_vcp_context(Some(&serde_json::json!({
            "river": "text"
        })))
        .is_ok());
        let exact_limit = Value::String("x".repeat(MAX_VCP_CONTEXT_BYTES - 2));
        assert_eq!(
            serde_json::to_vec(&exact_limit).unwrap().len(),
            MAX_VCP_CONTEXT_BYTES
        );
        assert!(validate_vcp_context(Some(&exact_limit)).is_ok());
        let oversized = Value::String("x".repeat(MAX_VCP_CONTEXT_BYTES));
        assert_eq!(
            validate_vcp_context(Some(&oversized)),
            Err("_vcpContext exceeds 128KB budget".to_string())
        );
    }
}

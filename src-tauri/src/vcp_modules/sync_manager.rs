use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::sync_types::{EntityState, SyncDataType, SyncManifest};
use crate::vcp_modules::sync_dto::{AgentSyncDTO, GroupSyncDTO, TopicSyncDTO};
use crate::vcp_modules::agent_service::{self, AgentConfigState};
use crate::vcp_modules::group_service::{self, GroupManagerState};
use crate::vcp_modules::hash_aggregator::HashAggregator;
use crate::vcp_modules::sync_retry::{RetryPolicy, is_network_retryable};
use crate::vcp_modules::sync_metrics::SyncMetrics;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, Runtime, State};
use tokio::sync::{mpsc, RwLock, Semaphore};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use std::path::Path;
use dashmap::DashMap;

/// =================================================================
/// vcp_modules/sync_manager.rs - 手机端同步调度中心 (Pointer-Based)
/// =================================================================

pub struct SyncState {
    pub ws_sender: mpsc::UnboundedSender<SyncCommand>,
    pub connection_status: Arc<RwLock<String>>,
    pub op_tracker: Arc<SyncOperationTracker>,
    pub metrics: Arc<SyncMetrics>,
    pub network_semaphore: Arc<NetworkAwareSemaphore>,
}

pub struct SyncOperationTracker {
    in_progress: DashMap<String, Instant>,
    ttl: Duration,
}

impl SyncOperationTracker {
    pub fn new() -> Self {
        Self {
            in_progress: DashMap::new(),
            ttl: Duration::from_secs(300), // 5 minutes
        }
    }

    pub fn try_start(&self, op_key: &str) -> bool {
        if self.in_progress.contains_key(op_key) {
            // Check if stale
            if let Some(start_time) = self.in_progress.get(op_key) {
                if Instant::now().duration_since(*start_time) < self.ttl {
                    return false;
                }
            }
        }
        self.in_progress.insert(op_key.to_string(), Instant::now());
        true
    }

    pub fn finish(&self, op_key: &str) {
        self.in_progress.remove(op_key);
    }
}

pub struct NetworkAwareSemaphore {
    semaphore: Arc<Semaphore>,
    network_type: Arc<RwLock<NetworkType>>,
    success_streak: AtomicU32,
    failure_streak: AtomicU32,
}

#[derive(Clone, Copy, PartialEq)]
pub enum NetworkType {
    WiFi,      // 20 permits
    Cell5G,    // 15 permits
    Cell4G,    // 10 permits
    Unknown,   // 8 permits
}

impl NetworkAwareSemaphore {
    pub fn new() -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(8)),
            network_type: Arc::new(RwLock::new(NetworkType::Unknown)),
            success_streak: AtomicU32::new(0),
            failure_streak: AtomicU32::new(0),
        }
    }

    pub async fn acquire(&self) -> tokio::sync::SemaphorePermit<'_> {
        self.semaphore.acquire().await.unwrap()
    }

    pub fn on_success(&self) {
        let streak = self.success_streak.fetch_add(1, Ordering::Relaxed) + 1;
        self.failure_streak.store(0, Ordering::Relaxed);
        if streak >= 10 {
            // Could implement dynamic permit adjustment here
            self.success_streak.store(0, Ordering::Relaxed);
        }
    }

    pub fn on_failure(&self) {
        self.success_streak.store(0, Ordering::Relaxed);
        let streak = self.failure_streak.fetch_add(1, Ordering::Relaxed) + 1;
        if streak >= 3 {
            // Could implement dynamic permit adjustment here
            self.failure_streak.store(0, Ordering::Relaxed);
        }
    }
}

fn make_op_key(action: &str, entity_type: &str, id: &str) -> String {
    format!("{}:{}:{}", action, entity_type, id)
}

fn generate_idempotency_key(action: &str, entity_type: &str, id: &str) -> String {
    use sha2::{Sha256, Digest};
    let mut hasher = Sha256::new();
    hasher.update(action.as_bytes());
    hasher.update(entity_type.as_bytes());
    hasher.update(id.as_bytes());
    // Add current minute to key to allow retries but prevent long-term duplicates
    let now = chrono::Utc::now().timestamp() / 60;
    hasher.update(now.to_string().as_bytes());
    format!("{:x}", hasher.finalize())
}

pub enum SyncCommand {
    NotifyLocalChange { id: String, data_type: SyncDataType, hash: String, ts: i64 },
    StartFullSync,
    RequestMessageManifest { topic_id: String },
}

fn parse_sync_data_type(value: &Value) -> Option<SyncDataType> {
    serde_json::from_value::<SyncDataType>(value.clone()).ok()
}

fn stringify_sync_data_type(data_type: &SyncDataType) -> String {
    serde_json::to_string(data_type)
        .unwrap_or_else(|_| "\"agent\"".to_string())
        .trim_matches('"')
        .to_string()
}

async fn publish_sync_status<R: Runtime>(app_handle: &AppHandle<R>, status: &Arc<RwLock<String>>, next_status: &str) {
    {
        let mut guard = status.write().await;
        if guard.as_str() == next_status {
            return;
        }
        *guard = next_status.to_string();
    }

    println!("[SyncManager] Status -> {}", next_status);
    let _ = app_handle.emit("vcp-sync-status", json!({ "status": next_status }));
}

pub fn init_sync_manager(app_handle: AppHandle) -> SyncState {
    let (tx, mut rx) = mpsc::unbounded_channel::<SyncCommand>();
    let handle_clone = app_handle.clone();
    let tx_internal = tx.clone();
    let connection_status = Arc::new(RwLock::new(String::from("connecting")));
    let connection_status_for_task = connection_status.clone();
    let op_tracker = Arc::new(SyncOperationTracker::new());
    let metrics = Arc::new(SyncMetrics::new());
    let network_semaphore = Arc::new(NetworkAwareSemaphore::new());
    
    let op_tracker_task = op_tracker.clone();
    let metrics_task = metrics.clone();
    let semaphore_task = network_semaphore.clone();

    tauri::async_runtime::spawn(async move {
        let http_client = reqwest::Client::new();
        let retry_policy = RetryPolicy::default();
        
        let mut pipeline_agents = std::collections::HashSet::new();
        let mut pipeline_groups = std::collections::HashSet::new();
        let mut pipeline_topics = std::collections::HashSet::new();
        let mut pipeline_messages = std::collections::HashMap::new();
        let mut pipeline_active = false;
        let mut last_pipeline_update = Instant::now();

        loop {
            let (ws_url, http_url) = {
                let settings_state = handle_clone.state::<crate::vcp_modules::settings_manager::SettingsState>();
                match crate::vcp_modules::settings_manager::read_settings(handle_clone.clone(), settings_state).await {
                    Ok(s) => {
                        // 检查 WebSocket URL
                        if s.sync_server_url.is_empty() {
                            println!("[SyncManager] sync_server_url (WebSocket) is empty, waiting...");
                            tokio::time::sleep(Duration::from_secs(10)).await;
                            continue;
                        }
                        // 检查 HTTP URL
                        if s.sync_http_url.is_empty() {
                            println!("[SyncManager] sync_http_url (HTTP) is empty, waiting...");
                            tokio::time::sleep(Duration::from_secs(10)).await;
                            continue;
                        }
                        
                        // WebSocket URL: 直接使用用户配置 of ws:// URL
                        let ws_addr = if let Ok(mut u) = url::Url::parse(&s.sync_server_url) {
                            u.set_query(Some(&format!("token={}", s.sync_token)));
                            u.to_string()
                        } else {
                            println!("[SyncManager] Failed to parse sync_server_url, using default");
                            format!("ws://127.0.0.1:5975?token={}", s.sync_token)
                        };
                        
                        println!("[SyncManager] WS URL: {}", ws_addr);
                        println!("[SyncManager] HTTP URL: {}", s.sync_http_url);
                        
                        (ws_addr, s.sync_http_url.clone())
                    }
                    Err(e) => {
                        println!("[SyncManager] Failed to read settings: {}, retrying in 5s...", e);
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        continue;
                    }
                }
            };

            publish_sync_status(&handle_clone, &connection_status_for_task, "connecting").await;

            match connect_async(&ws_url).await {
                Ok((mut ws_stream, _)) => {
                    println!("[SyncManager] WebSocket Connected successfully.");
                    publish_sync_status(&handle_clone, &connection_status_for_task, "connected").await;
                    
                    match generate_initial_manifests(&handle_clone).await {
                        Ok(manifests) => {
                            println!("[SyncManager] Generated {} manifests", manifests.len());
                            for manifest in manifests {
                                let data_type = stringify_sync_data_type(&manifest.data_type);
                                println!(
                                    "[SyncManager] Sending SYNC_MANIFEST dataType={} count={}",
                                    data_type,
                                    manifest.items.len()
                                );
                                let msg = json!({
                                    "type": "SYNC_MANIFEST",
                                    "data": manifest.items,
                                    "dataType": manifest.data_type
                                });
                                if let Err(e) = ws_stream.send(Message::Text(msg.to_string().into())).await {
                                    println!("[SyncManager] ERROR: Failed to send SYNC_MANIFEST: {}", e);
                                }
                            }
                        }
                        Err(e) => {
                            println!("[SyncManager] ERROR: Failed to generate manifests: {}", e);
                        }
                    }

                    match get_all_active_topic_ids(&handle_clone).await {
                        Ok(topic_ids) => {
                            println!("[SyncManager] Found {} active topics for message manifest", topic_ids.len());
                            for tid in topic_ids {
                                println!("[SyncManager] Requesting GET_MESSAGE_MANIFEST topicId={}", tid);
                                let msg = json!({ "type": "GET_MESSAGE_MANIFEST", "topicId": tid });
                                if let Err(e) = ws_stream.send(Message::Text(msg.to_string().into())).await {
                                    println!("[SyncManager] ERROR: Failed to send GET_MESSAGE_MANIFEST: {}", e);
                                }
                            }
                        }
                        Err(e) => {
                            println!("[SyncManager] ERROR: Failed to get active topic IDs: {}", e);
                        }
                    }

                    loop {
                        tokio::select! {
                            Some(cmd) = rx.recv() => {
                                match cmd {
                                    SyncCommand::NotifyLocalChange { id, data_type, hash, ts } => {
                                        let data_type_value = stringify_sync_data_type(&data_type);
                                        println!(
                                            "[SyncManager] Sending SYNC_ENTITY_UPDATE id={} dataType={} ts={}",
                                            id,
                                            data_type_value,
                                            ts
                                        );
                                        let msg = json!({ "type": "SYNC_ENTITY_UPDATE", "id": id, "dataType": data_type, "hash": hash, "ts": ts });
                                        if let Err(e) = ws_stream.send(Message::Text(msg.to_string().into())).await {
                                            println!("[SyncManager] ERROR: Failed to send SYNC_ENTITY_UPDATE: {}", e);
                                        }
                                    },
                                    SyncCommand::StartFullSync => {
                                        match generate_initial_manifests(&handle_clone).await {
                                            Ok(manifests) => {
                                                for manifest in manifests {
                                                    let data_type = stringify_sync_data_type(&manifest.data_type);
                                                    println!(
                                                        "[SyncManager] Re-sending SYNC_MANIFEST dataType={} count={}",
                                                        data_type,
                                                        manifest.items.len()
                                                    );
                                                    if let Err(e) = ws_stream.send(Message::Text(json!({"type":"SYNC_MANIFEST","data":manifest.items,"dataType":manifest.data_type}).to_string().into())).await {
                                                        println!("[SyncManager] ERROR: Failed to re-send SYNC_MANIFEST: {}", e);
                                                    }
                                                }
                                            }
                                            Err(e) => {
                                                println!("[SyncManager] ERROR: Failed to regenerate manifests: {}", e);
                                            }
                                        }
                                    },
                                    SyncCommand::RequestMessageManifest { topic_id } => {
                                        println!("[SyncManager] WS Command Requesting GET_MESSAGE_MANIFEST topicId={}", topic_id);
                                        let msg = json!({ "type": "GET_MESSAGE_MANIFEST", "topicId": topic_id });
                                        if let Err(e) = ws_stream.send(Message::Text(msg.to_string().into())).await {
                                            println!("[SyncManager] ERROR: Failed to send GET_MESSAGE_MANIFEST: {}", e);
                                        }
                                    }
                                }
                            }
                            Some(Ok(msg)) = ws_stream.next() => {
                                if let Message::Text(text) = msg {
                                    let payload: Value = serde_json::from_str(&text).unwrap_or_else(|e| {
                                        println!("[SyncManager] ERROR: Failed to parse WS message: {}", e);
                                        Value::Null
                                    });
                                    
                                    if payload.is_null() {
                                        continue;
                                    }
                                    
                                    let h = handle_clone.clone();
                                    let c = http_client.clone();
                                    let base = http_url.clone();
                                    let tx_in = tx_internal.clone();
                                    let sem_outer = semaphore_task.clone();
                                    let metrics_outer = metrics_task.clone();
                                    let retry_outer = retry_policy.default_clone();
                                    let op_tracker_outer = op_tracker_task.clone();
                                    
                                    match payload["type"].as_str() {
                                        Some("SYNC_ENTITY_UPDATE") => {
                                            let id = payload["id"].as_str().unwrap_or_default().to_string();
                                            let Some(data_type) = parse_sync_data_type(&payload["dataType"]) else {
                                                println!("[SyncManager] WARN: Ignoring SYNC_ENTITY_UPDATE with invalid dataType: {}", payload["dataType"]);
                                                continue;
                                            };
                                            let data_type_value = stringify_sync_data_type(&data_type);
                                            println!(
                                                "[SyncManager] Received SYNC_ENTITY_UPDATE id={} dataType={}",
                                                id,
                                                data_type_value
                                            );
                                            if data_type == SyncDataType::Message {
                                                let msg = json!({ "type": "GET_MESSAGE_MANIFEST", "topicId": id });
                                                if let Err(e) = ws_stream.send(Message::Text(msg.to_string().into())).await {
                                                    println!("[SyncManager] ERROR: Failed to send GET_MESSAGE_MANIFEST: {}", e);
                                                }
                                            } else {
                                                let h_inner = h.clone(); let c_inner = c.clone(); let base_inner = base.clone(); let sem_inner = sem_outer.clone(); let entity_type = data_type_value.clone();
                                                let metrics_inner = metrics_outer.clone();
                                                let op_tracker_inner = op_tracker_outer.clone();
                                                tauri::async_runtime::spawn(async move {
                                                    let op_key = make_op_key("pull", &entity_type, &id);
                                                    if !op_tracker_inner.try_start(&op_key) { return; }
                                                    scopeguard::defer! { op_tracker_inner.finish(&op_key); }

                                                    let _permit = sem_inner.acquire().await;
                                                    metrics_inner.record_start();
                                                    println!("[SyncManager] Starting PULL from SYNC_ENTITY_UPDATE id={} type={}", id, entity_type);
                                                    match perform_pull(&h_inner, &c_inner, &base_inner, &id, &entity_type).await {
                                                        Ok(_) => {
                                                            println!("[SyncManager] PULL from SYNC_ENTITY_UPDATE completed for id={}", id);
                                                            sem_inner.on_success();
                                                            metrics_inner.record_success();
                                                        }
                                                        Err(e) => {
                                                            println!("[SyncManager] ERROR: PULL from SYNC_ENTITY_UPDATE failed for id={}: {}", id, e);
                                                            sem_inner.on_failure();
                                                            metrics_inner.record_failure();
                                                        }
                                                    }
                                                    metrics_inner.emit_to_frontend(&h_inner);
                                                });
                                            }
                                        },
                                        Some("SYNC_DIFF_RESULTS") => {
                                            if let Some(items) = payload["data"].as_array() {
                                                let Some(data_type) = parse_sync_data_type(&payload["dataType"]) else {
                                                    println!("[SyncManager] WARN: Ignoring SYNC_DIFF_RESULTS with invalid dataType: {}", payload["dataType"]);
                                                    continue;
                                                };
                                                let data_type_value = stringify_sync_data_type(&data_type);
                                                println!(
                                                    "[SyncManager] Received SYNC_DIFF_RESULTS dataType={} count={}",
                                                    data_type_value,
                                                    items.len()
                                                );
                                                for item in items {
                                                    let id = item["id"].as_str().unwrap_or_default().to_string();
                                                    let action = item["action"].as_str().unwrap_or_default();
                                                    println!(
                                                        "[SyncManager] Diff action id={} dataType={} action={}",
                                                        id,
                                                        data_type_value,
                                                        action
                                                    );
                                                    let _h_inner = h.clone(); 
                                                    let _c_inner = c.clone(); 
                                                    let _base_inner = base.clone(); 
                                                    let _sem_inner = sem_outer.clone(); 
                                                    let _et_inner = data_type_value.clone();
                                                    let _tx_pull = tx_in.clone();
                                                    let _op_tracker_inner = op_tracker_outer.clone();
                                                    let _metrics_inner = metrics_outer.clone();
                                                    let _retry_inner = retry_outer.default_clone();
                                                    
                                                    if action == "PULL" {
                                                        let h_inner = h.clone(); 
                                                        let c_inner = c.clone(); 
                                                        let base_inner = base.clone(); 
                                                        let sem_inner = semaphore_task.clone(); 
                                                        let et_inner = data_type_value.clone();
                                                        let tx_pull = tx_in.clone();
                                                        let op_tracker_inner = op_tracker_task.clone();
                                                        let metrics_inner = metrics_task.clone();
                                                        let retry_inner = retry_policy.default_clone();

                                                        tauri::async_runtime::spawn(async move {
                                                            let op_key = make_op_key("pull", &et_inner, &id);
                                                            if !op_tracker_inner.try_start(&op_key) {
                                                                return;
                                                            }
                                                            scopeguard::defer! { op_tracker_inner.finish(&op_key); }

                                                            let _permit = sem_inner.acquire().await;
                                                            metrics_inner.record_start();
                                                            
                                                            let result = retry_inner.execute_with_retry(
                                                                || perform_pull(&h_inner, &c_inner, &base_inner, &id, &et_inner),
                                                                |e| is_network_retryable(e)
                                                            ).await;

                                                            match result {
                                                                Ok(_) => {
                                                                    println!("[SyncManager] PULL completed successfully for id={}", id);
                                                                    sem_inner.on_success();
                                                                    metrics_inner.record_success();
                                                                    if et_inner == "topic" {
                                                                        let _ = tx_pull.send(SyncCommand::RequestMessageManifest { topic_id: id });
                                                                    }
                                                                }
                                                                Err(e) => {
                                                                    println!("[SyncManager] ERROR: PULL failed for id={}: {}", id, e);
                                                                    sem_inner.on_failure();
                                                                    metrics_inner.record_failure();
                                                                }
                                                            }
                                                            metrics_inner.emit_to_frontend(&h_inner);
                                                        });
                                                    } else if action == "PUSH" {
                                                        let h_inner = h.clone(); 
                                                        let c_inner = c.clone(); 
                                                        let base_inner = base.clone(); 
                                                        let sem_inner = semaphore_task.clone(); 
                                                        let et_inner = data_type_value.clone();
                                                        let op_tracker_inner = op_tracker_task.clone();
                                                        let metrics_inner = metrics_task.clone();
                                                        let retry_inner = retry_policy.default_clone();

                                                        tauri::async_runtime::spawn(async move {
                                                            let op_key = make_op_key("push", &et_inner, &id);
                                                            if !op_tracker_inner.try_start(&op_key) {
                                                                return;
                                                            }
                                                            scopeguard::defer! { op_tracker_inner.finish(&op_key); }

                                                            let _permit = sem_inner.acquire().await;
                                                            metrics_inner.record_start();

                                                            let result = retry_inner.execute_with_retry(
                                                                || perform_push(&h_inner, &c_inner, &base_inner, &id, &et_inner),
                                                                |e| is_network_retryable(e)
                                                            ).await;

                                                            match result {
                                                                Ok(_) => {
                                                                    println!("[SyncManager] PUSH completed successfully for id={}", id);
                                                                    sem_inner.on_success();
                                                                    metrics_inner.record_success();
                                                                },
                                                                Err(e) => {
                                                                    println!("[SyncManager] ERROR: PUSH failed for id={}: {}", id, e);
                                                                    sem_inner.on_failure();
                                                                    metrics_inner.record_failure();
                                                                },
                                                            }
                                                            metrics_inner.emit_to_frontend(&h_inner);
                                                        });
                                                    }
                                                }
                                            } else {
                                                println!("[SyncManager] WARN: SYNC_DIFF_RESULTS missing data array");
                                            }
                                        },
                                        Some("MESSAGE_MANIFEST_RESULTS") => {
                                            let topic_id = payload["topicId"].as_str().unwrap_or_default().to_string();
                                            if let Some(remote_msgs) = payload["messages"].as_array() {
                                                println!(
                                                    "[SyncManager] Received MESSAGE_MANIFEST_RESULTS topicId={} count={}",
                                                    topic_id,
                                                    remote_msgs.len()
                                                );
                                                if pipeline_active {
                                                    last_pipeline_update = Instant::now();
                                                    pipeline_messages.insert(topic_id, remote_msgs.clone());
                                                } else {
                                                    let h_inner = h.clone(); let c_inner = c.clone(); let base_inner = base.clone(); let sem_inner = sem_outer.clone(); let msgs_vec = remote_msgs.clone();
                                                    let metrics_inner = metrics_outer.clone();
                                                    tauri::async_runtime::spawn(async move {
                                                        let _permit = sem_inner.acquire().await;
                                                        metrics_inner.record_start();
                                                        println!("[SyncManager] Starting history delta sync for topicId={}", topic_id);
                                                        match perform_history_delta_sync(&h_inner, &c_inner, &base_inner, &topic_id, &msgs_vec).await {
                                                            Ok(_) => {
                                                                println!("[SyncManager] History delta sync completed for topicId={}", topic_id);
                                                                sem_inner.on_success();
                                                                metrics_inner.record_success();
                                                            }
                                                            Err(e) => {
                                                                println!("[SyncManager] ERROR: History delta sync failed for topicId={}: {}", topic_id, e);
                                                                sem_inner.on_failure();
                                                                metrics_inner.record_failure();
                                                            }
                                                        }
                                                        metrics_inner.emit_to_frontend(&h_inner);
                                                    });
                                                }
                                            } else {
                                                println!("[SyncManager] WARN: MESSAGE_MANIFEST_RESULTS missing messages array");
                                            }
                                        },
                                        Some(unknown_type) => {
                                            println!("[SyncManager] WARN: Unknown message type: {}", unknown_type);
                                        },
                                        None => {
                                            println!("[SyncManager] WARN: Message missing type field: {}", text.chars().take(100).collect::<String>());
                                        }
                                    }
                                }
                            }
                            else => {
                                println!("[SyncManager] WebSocket stream ended, disconnecting...");
                                publish_sync_status(&handle_clone, &connection_status_for_task, "disconnected").await;
                                break;
                            },
                        }
                    }
                }
                Err(e) => {
                    println!("[SyncManager] ERROR: Connection failed: {}. Retrying in 5s...", e);
                    publish_sync_status(&handle_clone, &connection_status_for_task, "disconnected").await;
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            }
        }
    });

    SyncState {
        ws_sender: tx,
        connection_status,
        op_tracker,
        metrics,
        network_semaphore,
    }
}

#[tauri::command]
pub async fn get_sync_status(state: State<'_, SyncState>) -> Result<String, String> {
    let status = state.connection_status.read().await.clone();
    println!("[SyncManager] get_sync_status -> {}", status);
    Ok(status)
}

fn stable_stringify(value: &Value) -> String {
    crate::vcp_modules::sync_types::stable_stringify(value)
}

async fn generate_initial_manifests(app: &AppHandle) -> Result<Vec<SyncManifest>, String> {
    let db_state = app.state::<DbState>();
    let pool = &db_state.pool;
    let mut manifests = Vec::new();

    // Agent manifest
    let mut agent_items = Vec::new();
    match sqlx::query("SELECT agent_id, config_hash, content_hash, updated_at FROM agents WHERE deleted_at IS NULL").fetch_all(pool).await {
        Ok(rows) => {
            for r in rows {
                use sqlx::Row;
                let config_hash: String = r.get("config_hash");
                let content_hash: String = r.get("content_hash");
                let final_hash = HashAggregator::aggregate_agent_manifest_hash(&config_hash, &content_hash);
                agent_items.push(EntityState { id: r.get("agent_id"), hash: final_hash, ts: r.get("updated_at") });
            }
            println!("[SyncManager] Loaded {} agents for manifest", agent_items.len());
        }
        Err(e) => {
            println!("[SyncManager] ERROR: Failed to query agents: {}", e);
            return Err(format!("Failed to query agents: {}", e));
        }
    }
    manifests.push(SyncManifest { data_type: SyncDataType::Agent, items: agent_items });

    // Group manifest
    let mut group_items = Vec::new();
    match sqlx::query("SELECT group_id, config_hash, content_hash, updated_at FROM groups WHERE deleted_at IS NULL").fetch_all(pool).await {
        Ok(rows) => {
            for r in rows {
                use sqlx::Row;
                let config_hash: String = r.get("config_hash");
                let content_hash: String = r.get("content_hash");
                let final_hash = HashAggregator::aggregate_group_manifest_hash(&config_hash, &content_hash);
                group_items.push(EntityState { id: r.get("group_id"), hash: final_hash, ts: r.get("updated_at") });
            }
            println!("[SyncManager] Loaded {} groups for manifest", group_items.len());
        }
        Err(e) => {
            println!("[SyncManager] ERROR: Failed to query groups: {}", e);
            return Err(format!("Failed to query groups: {}", e));
        }
    }
    manifests.push(SyncManifest { data_type: SyncDataType::Group, items: group_items });

    // Avatar manifest
    let mut avatar_items = Vec::new();
    match sqlx::query("SELECT owner_id, owner_type, avatar_hash, updated_at FROM avatars").fetch_all(pool).await {
        Ok(rows) => {
            for r in rows {
                use sqlx::Row;
                let owner_id: String = r.get("owner_id");
                let owner_type: String = r.get("owner_type");
                avatar_items.push(EntityState { 
                    id: format!("{}:{}", owner_type, owner_id), 
                    hash: r.get("avatar_hash"), 
                    ts: r.get("updated_at") 
                });
            }
            println!("[SyncManager] Loaded {} avatars for manifest", avatar_items.len());
        }
        Err(e) => {
            println!("[SyncManager] ERROR: Failed to query avatars: {}", e);
            return Err(format!("Failed to query avatars: {}", e));
        }
    }
    manifests.push(SyncManifest { data_type: SyncDataType::Avatar, items: avatar_items });

    // Topic manifest
    let mut topic_items = Vec::new();
    match sqlx::query("SELECT topic_id, title, created_at, locked, unread, content_hash, updated_at, owner_id, owner_type FROM topics WHERE deleted_at IS NULL").fetch_all(pool).await {
        Ok(rows) => {
            for r in rows {
                use sqlx::Row;
                let id: String = r.get("topic_id");
                let name: String = r.get("title");
                let owner_id: String = r.get("owner_id");
                let owner_type: String = r.get("owner_type");
                let created_at: i64 = r.get("created_at");
                let locked: bool = r.get::<i64, _>("locked") != 0;
                let unread: bool = r.get::<i64, _>("unread") != 0;
                let content_hash: String = r.get("content_hash");

                let dto = TopicSyncDTO {
                    id: id.clone(),
                    name,
                    created_at,
                    locked: if owner_type == "group" { true } else { locked },
                    unread: if owner_type == "group" { false } else { unread },
                    owner_id,
                    owner_type,
                };
                
                let metadata_hash = HashAggregator::compute_topic_metadata_hash(&dto);
                let final_hash = HashAggregator::aggregate_topic_manifest_hash(&metadata_hash, &content_hash);

                topic_items.push(EntityState { id, hash: final_hash, ts: r.get("updated_at") });
            }
            println!("[SyncManager] Loaded {} topics for manifest", topic_items.len());
        }
        Err(e) => {
            println!("[SyncManager] ERROR: Failed to query topics: {}", e);
            return Err(format!("Failed to query topics: {}", e));
        }
    }
    manifests.push(SyncManifest { data_type: SyncDataType::Topic, items: topic_items });

    Ok(manifests)
}

async fn get_all_active_topic_ids(app: &AppHandle) -> Result<Vec<String>, String> {
    let db_state = app.state::<DbState>();
    match sqlx::query("SELECT topic_id FROM topics WHERE deleted_at IS NULL").fetch_all(&db_state.pool).await {
        Ok(rows) => {
            let ids: Vec<String> = rows.into_iter().map(|r| { use sqlx::Row; r.get(0) }).collect();
            Ok(ids)
        }
        Err(e) => {
            println!("[SyncManager] ERROR: Failed to query topic IDs: {}", e);
            Err(e.to_string())
        }
    }
}

async fn perform_pull<R: Runtime>(app: &AppHandle<R>, client: &reqwest::Client, http_url: &str, id: &str, entity_type: &str) -> Result<(), String> {
    println!("[SyncManager] perform_pull: id={} type={}", id, entity_type);
    
    let settings_state = app.state::<crate::vcp_modules::settings_manager::SettingsState>();
    let settings = crate::vcp_modules::settings_manager::read_settings(app.clone(), settings_state).await?;
    let url = format!("{}/api/mobile-sync/download-entity?id={}&type={}", http_url, id, entity_type);
    println!("[SyncManager] PULL URL: {}", url);
    
    let res = client.get(&url).header("x-sync-token", &settings.sync_token).send().await.map_err(|e| {
        println!("[SyncManager] ERROR: PULL request failed: {}", e);
        e.to_string()
    })?;
    
    if !res.status().is_success() {
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        println!("[SyncManager] ERROR: PULL failed with status {}: {}", status, body);
        return Err(format!("Pull failed: {} - {}", status, body));
    }

    if entity_type == "agent" {
        let dto: AgentSyncDTO = res.json().await.map_err(|e| {
            println!("[SyncManager] ERROR: Failed to parse AgentSyncDTO: {}", e);
            e.to_string()
        })?;
        agent_service::apply_sync_update(app, &app.state::<AgentConfigState>(), id, dto).await?;
        println!("[SyncManager] Agent {} synced successfully", id);
    } else if entity_type == "group" {
        let dto: GroupSyncDTO = res.json().await.map_err(|e| {
            println!("[SyncManager] ERROR: Failed to parse GroupSyncDTO: {}", e);
            e.to_string()
        })?;
        group_service::apply_sync_update(app, &app.state::<GroupManagerState>(), id, dto).await?;
        println!("[SyncManager] Group {} synced successfully", id);
    } else if entity_type == "avatar" {
        let parts: Vec<&str> = id.split(':').collect();
        if parts.len() == 2 {
            let owner_type = parts[0];
            let owner_id = parts[1];
            let settings = crate::vcp_modules::settings_manager::read_settings(app.clone(), app.state()).await?;
            let url = format!("{}/api/mobile-sync/download-avatar?id={}", http_url, owner_id);
            println!("[SyncManager] Avatar PULL URL: {}", url);
            
            let resp = client.get(&url).header("x-sync-token", &settings.sync_token).send().await.map_err(|e| e.to_string())?;
            if resp.status().is_success() {
                let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
                let hash = HashAggregator::compute_avatar_hash(&bytes);
                let now = chrono::Utc::now().timestamp_millis();
                let mime_type = "image/png";
                
                let db_state = app.state::<DbState>();
                sqlx::query("INSERT INTO avatars (owner_type, owner_id, avatar_hash, mime_type, image_data, updated_at) VALUES (?, ?, ?, ?, ?, ?) ON CONFLICT(owner_type, owner_id) DO UPDATE SET avatar_hash=excluded.avatar_hash, mime_type=excluded.mime_type, image_data=excluded.image_data, updated_at=excluded.updated_at")
                    .bind(owner_type).bind(owner_id).bind(&hash).bind(mime_type).bind(&bytes[..]).bind(now).execute(&db_state.pool).await.map_err(|e| {
                        println!("[SyncManager] ERROR: Failed to upsert avatar: {}", e);
                        e.to_string()
                    })?;
                println!("[SyncManager] Avatar {}:{} saved to DB ({} bytes)", owner_type, owner_id, bytes.len());
            } else {
                println!("[SyncManager] WARN: Avatar download failed with status: {}", resp.status());
            }
        } else {
            println!("[SyncManager] WARN: Invalid avatar id format: {}", id);
        }
    } else if entity_type == "topic" {
        let dto: TopicSyncDTO = res.json().await.map_err(|e| {
            println!("[SyncManager] ERROR: Failed to parse TopicSyncDTO: {}", e);
            e.to_string()
        })?;
        let db_state = app.state::<DbState>();
        let now = chrono::Utc::now().timestamp_millis();
        sqlx::query("INSERT INTO topics (topic_id, title, owner_id, owner_type, created_at, locked, unread, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(topic_id) DO UPDATE SET title = excluded.title, locked = excluded.locked, unread = excluded.unread, updated_at = excluded.updated_at")
            .bind(id).bind(&dto.name).bind(&dto.owner_id).bind(&dto.owner_type).bind(dto.created_at)
            .bind(if dto.locked { 1 } else { 0 }).bind(if dto.unread { 1 } else { 0 }).bind(now)
            .execute(&db_state.pool).await.map_err(|e| {
                println!("[SyncManager] ERROR: Failed to upsert topic: {}", e);
                e.to_string()
            })?;
        println!("[SyncManager] Topic {} synced successfully", id);
    }
    Ok(())
}

async fn perform_push(app: &AppHandle, client: &reqwest::Client, http_url: &str, id: &str, entity_type: &str) -> Result<(), String> {
    println!("[SyncManager] perform_push: id={} type={}", id, entity_type);
    
    let db_state = app.state::<DbState>();
    let pool = &db_state.pool;
    let settings = crate::vcp_modules::settings_manager::read_settings(app.clone(), app.state()).await?;

    // Avatar 使用独立的二进制上传流程
    if entity_type == "avatar" {
        let parts: Vec<&str> = id.split(':').collect();
        if parts.len() == 2 {
            let owner_type = parts[0];
            let owner_id = parts[1];
            let row = sqlx::query("SELECT image_data, mime_type FROM avatars WHERE owner_id = ? AND owner_type = ?")
                .bind(owner_id).bind(owner_type)
                .fetch_optional(pool).await.map_err(|e| e.to_string())?;
            
            let row = match row {
                Some(r) => r,
                None => {
                    println!("[SyncManager] DEBUG: Avatar {}:{} not found in DB, skipping push.", owner_type, owner_id);
                    return Ok(());
                }
            };

            use sqlx::Row;
            let image_data: Vec<u8> = row.get("image_data");
            let mime_type: String = row.get("mime_type");
            
            let url = format!("{}/api/mobile-sync/upload-avatar", http_url);
            let res = client.post(&url)
                .header("x-sync-token", &settings.sync_token)
                .header("Content-Type", mime_type)
                .query(&[("id", owner_id), ("type", owner_type)])
                .body(image_data)
                .send().await.map_err(|e| e.to_string())?;
            
            if res.status().is_success() {
                println!("[SyncManager] Avatar {}:{} pushed successfully", owner_type, owner_id);
            } else {
                println!("[SyncManager] WARN: Avatar push failed with status: {}", res.status());
            }
            return Ok(());
        } else { 
            println!("[SyncManager] WARN: Invalid avatar id format for push: {}", id);
            return Ok(()); 
        }
    }

    let payload = if entity_type == "topic" {
        let row = sqlx::query("SELECT topic_id, title, created_at, locked, unread, owner_id, owner_type FROM topics WHERE topic_id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await
            .map_err(|e| e.to_string())?;
        
        let row = match row {
            Some(r) => r,
            None => {
                println!("[SyncManager] DEBUG: Topic {} metadata missing during perform_push, skipping.", id);
                return Ok(());
            }
        };

        use sqlx::Row;
        
        let owner_type: String = row.get("owner_type");
        let dto = TopicSyncDTO {
            id: row.get("topic_id"),
            name: row.get("title"),
            created_at: row.get("created_at"),
            locked: if owner_type == "group" { true } else { row.get::<i64, _>("locked") != 0 },
            unread: if owner_type == "group" { false } else { row.get::<i64, _>("unread") != 0 },
            owner_id: row.get("owner_id"),
            owner_type,
        };
        json!({ "id": id, "type": "topic", "data": dto })
    } else if entity_type == "agent" {
        let state = app.state::<AgentConfigState>();
        let config = agent_service::read_agent_config(app.clone(), state.clone(), id.to_string(), None).await?;
        json!({ "id": id, "type": "agent", "data": AgentSyncDTO::from(&config) })
    } else if entity_type == "group" {
        let state = app.state::<GroupManagerState>();
        let config = group_service::read_group_config(app.clone(), state.clone(), id.to_string()).await?;
        json!({ "id": id, "type": "group", "data": GroupSyncDTO::from(&config) })
    } else { 
        println!("[SyncManager] WARN: Unknown entity type for push: {}", entity_type);
        return Ok(()); 
    };

    let url = format!("{}/api/mobile-sync/upload-entity", http_url);
    println!("[SyncManager] PUSH URL: {}", url);
    
    let idempotency_key = generate_idempotency_key("push", entity_type, id);
    let res = client.post(&url)
        .header("x-sync-token", &settings.sync_token)
        .header("x-idempotency-key", idempotency_key)
        .json(&payload)
        .send().await.map_err(|e| {
        println!("[SyncManager] ERROR: PUSH request failed: {}", e);
        e.to_string()
    })?;
    
    if !res.status().is_success() {
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        println!("[SyncManager] ERROR: PULL failed with status {}: {}", status, body);
        return Err(format!("Push failed: {} - {}", status, body));
    }
    
    if entity_type == "message" { 
        perform_history_push(app, client, http_url, id).await?; 
    }
    println!("[SyncManager] Pushed successfully: {} ({})", id, entity_type);
    Ok(())
}

async fn perform_history_push(app: &AppHandle, client: &reqwest::Client, http_url: &str, topic_id: &str) -> Result<(), String> {
    println!("[SyncManager] perform_history_push for topic: {}", topic_id);
    
    let pool = &app.state::<DbState>().pool;
    
    // 先检查 topic 是否存在于本地
    let topic_exists = sqlx::query("SELECT 1 FROM topics WHERE topic_id = ?")
        .bind(topic_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| e.to_string())?;
    
    if topic_exists.is_none() {
        println!("[SyncManager] WARN: Topic {} not found locally, skipping history push", topic_id);
        return Ok(());
    }
    
    let metadata_row = sqlx::query("SELECT owner_id, owner_type FROM topics WHERE topic_id = ?")
        .bind(topic_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| e.to_string())?;

    let (owner_id, owner_type) = match metadata_row {
        Some(row) => {
            use sqlx::Row;
            (row.get::<String, _>("owner_id"), row.get::<String, _>("owner_type"))
        },
        None => {
            println!("[SyncManager] DEBUG: Topic metadata for {} disappeared during history push, skipping.", topic_id);
            return Ok(());
        }
    };

    let history = crate::vcp_modules::message_service::load_chat_history_internal(app, &owner_id, &owner_type, topic_id, Some(1000), None).await?;
    let settings = crate::vcp_modules::settings_manager::read_settings(app.clone(), app.state()).await?;
    let app_data = app.path().app_data_dir().unwrap();

    println!("[SyncManager] Pushing {} messages for topic {}", history.len(), topic_id);

    for msg in &history {
        if let Some(atts) = &msg.attachments {
            for att in atts {
                if let Some(hash) = &att.hash {
                    let ext = Path::new(&att.name).extension().and_then(|s| s.to_str()).unwrap_or("bin");
                    let local_path = app_data.join("attachments").join(format!("{}.{}", hash, ext));
                    if local_path.exists() {
                        if let Ok(bytes) = std::fs::read(&local_path) {
                            let att_url = format!("{}/api/mobile-sync/upload-attachment?hash={}&name={}", http_url, hash, urlencoding::encode(&att.name));
                            if let Err(e) = client.post(&att_url).header("x-sync-token", &settings.sync_token).body(bytes).send().await {
                                println!("[SyncManager] WARN: Failed to upload attachment {}: {}", hash, e);
                            }
                        }
                    }
                }
            }
        }
    }

    let dto_history: Vec<crate::vcp_modules::sync_dto::MessageSyncDTO> = history.iter().map(crate::vcp_modules::sync_dto::MessageSyncDTO::from).collect();
    let url = format!("{}/api/mobile-sync/upload-messages", http_url);
    
    let res = client.post(&url)
        .header("x-sync-token", &settings.sync_token)
        .json(&json!({ "topicId": topic_id, "messages": dto_history }))
        .send().await.map_err(|e| {
            println!("[SyncManager] ERROR: Upload messages request failed: {}", e);
            e.to_string()
        })?;
    
    if !res.status().is_success() {
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        println!("[SyncManager] ERROR: Upload messages failed with status {}: {}", status, body);
        return Err(format!("Upload messages failed: {} - {}", status, body));
    }
    
    println!("[SyncManager] History push completed for topic {}", topic_id);
    Ok(())
}

async fn perform_history_delta_sync(app: &AppHandle, client: &reqwest::Client, http_url: &str, topic_id: &str, remote_msgs: &Vec<Value>) -> Result<(), String> {
    println!("[SyncManager] perform_history_delta_sync for topic: {} with {} remote messages", topic_id, remote_msgs.len());
    
    let db_state = app.state::<DbState>();
    let pool = &db_state.pool;
    let rows = sqlx::query("SELECT m.msg_id, m.content, m.updated_at, a.hash as att_hash FROM messages m LEFT JOIN message_attachments ma ON m.msg_id = ma.msg_id LEFT JOIN attachments a ON ma.hash = a.hash WHERE m.topic_id = ? AND m.deleted_at IS NULL").bind(topic_id).fetch_all(pool).await.map_err(|e| e.to_string())?;
    let mut local_map = std::collections::HashMap::new();
    for r in rows {
        use sqlx::Row;
        let id: String = r.get("msg_id");
        let entry = local_map.entry(id).or_insert((r.get::<String, _>("content"), r.get::<i64, _>("updated_at"), Vec::new()));
        if let Some(h) = r.get::<Option<String>, _>("att_hash") { entry.2.push(h); }
    }
    
    println!("[SyncManager] Local messages count: {}", local_map.len());

    let mut to_pull_ids = Vec::new();
    let mut to_push = false;
    let mut remote_ids = std::collections::HashSet::new();

    for rm in remote_msgs {
        let rid = rm["msg_id"].as_str().unwrap_or_default().to_string();
        remote_ids.insert(rid.clone());
        let rhash = rm["content_hash"].as_str().unwrap_or_default();
        let rts = rm["updated_at"].as_i64().unwrap_or(0);
        if let Some((lcontent, lts, latts)) = local_map.get(&rid) {
            let local_hash = HashAggregator::compute_message_fingerprint(lcontent, latts);
            if local_hash != rhash {
                println!("[SyncManager] Message {} hash mismatch: local={}, remote={}", rid, local_hash, rhash);
                if rts > *lts { to_pull_ids.push(rid); }
                else { to_push = true; }
            }
        } else { 
            println!("[SyncManager] Message {} not found locally, will pull", rid);
            to_pull_ids.push(rid); 
        }
    }

    // Check if mobile has messages that desktop doesn't have
    for lid in local_map.keys() {
        if !remote_ids.contains(lid) {
            println!("[SyncManager] Local message {} not found remotely, will push", lid);
            to_push = true;
            break;
        }
    }

    println!("[SyncManager] Delta sync result: to_pull={}, to_push={}", to_pull_ids.len(), to_push);

    if to_push { 
        println!("[SyncManager] Starting history push for topic {}", topic_id);
        let _ = perform_history_push(app, client, http_url, topic_id).await; 
    }
    
    if !to_pull_ids.is_empty() {
        // 先检查 topic 是否存在于本地，不存在则跳过（等待 topic 元数据同步后再处理）
        let topic_exists = sqlx::query("SELECT 1 FROM topics WHERE topic_id = ?")
            .bind(topic_id)
            .fetch_optional(pool)
            .await
            .map_err(|e| e.to_string())?;
        
        if topic_exists.is_none() {
            println!("[SyncManager] WARN: Topic {} not found locally, skipping message sync (will sync after topic metadata)", topic_id);
            return Ok(());
        }
        
        let metadata_row = sqlx::query("SELECT owner_id, owner_type FROM topics WHERE topic_id = ?")
            .bind(topic_id)
            .fetch_optional(pool)
            .await
            .map_err(|e| e.to_string())?;

        let (agent_id, owner_type) = match metadata_row {
            Some(row) => {
                use sqlx::Row;
                (row.get::<String, _>("owner_id"), row.get::<String, _>("owner_type"))
            },
            None => {
                println!("[SyncManager] DEBUG: Topic metadata for {} disappeared during delta sync, skipping.", topic_id);
                return Ok(());
            }
        };

        let settings = crate::vcp_modules::settings_manager::read_settings(app.clone(), app.state()).await?;
        let url = format!("{}/api/mobile-sync/download-messages", http_url);
        
        println!("[SyncManager] Downloading {} messages from {}", to_pull_ids.len(), url);
        
        let res = client.post(&url).header("x-sync-token", &settings.sync_token).json(&json!({ "topicId": topic_id, "msgIds": to_pull_ids })).send().await.map_err(|e| e.to_string())?;
        if res.status().is_success() {
            let messages: Vec<Value> = res.json().await.map_err(|e| e.to_string())?;
            println!("[SyncManager] Downloaded {} messages", messages.len());
            
            for m_val in messages {
                if let Ok(msg) = serde_json::from_value::<crate::vcp_modules::chat_manager::ChatMessage>(m_val) {
                    crate::vcp_modules::message_service::patch_single_message(app.clone(), pool, &agent_id, &owner_type, topic_id.to_string(), msg, true).await?;
                }
            }
            let msg_count: i32 = sqlx::query_scalar::<sqlx::Sqlite, i64>("SELECT COUNT(*) FROM messages WHERE topic_id = ? AND deleted_at IS NULL")
                .bind(topic_id).fetch_optional(pool).await.map_err(|e| e.to_string())?.unwrap_or(0) as i32;
            sqlx::query("UPDATE topics SET msg_count = ? WHERE topic_id = ?").bind(msg_count).bind(topic_id).execute(pool).await.map_err(|e| e.to_string())?;
            println!("[SyncManager] Updated topic {} msg_count to {}", topic_id, msg_count);
        } else {
            println!("[SyncManager] ERROR: Download messages failed with status: {}", res.status());
        }
    }
    
    println!("[SyncManager] History delta sync completed for topic {}", topic_id);
    Ok(())
}
Vec::new();
    let mut to_push = false;
    let mut remote_ids = std::collections::HashSet::new();

    for rm in remote_msgs {
        let rid = rm["msg_id"].as_str().unwrap_or_default().to_string();
        remote_ids.insert(rid.clone());
        let rhash = rm["content_hash"].as_str().unwrap_or_default();
        let rts = rm["updated_at"].as_i64().unwrap_or(0);
        if let Some((lcontent, lts, latts)) = local_map.get(&rid) {
            let local_hash = HashAggregator::compute_message_fingerprint(lcontent, latts);
            if local_hash != rhash {
                println!("[SyncManager] Message {} hash mismatch: local={}, remote={}", rid, local_hash, rhash);
                if rts > *lts { to_pull_ids.push(rid); }
                else { to_push = true; }
            }
        } else { 
            println!("[SyncManager] Message {} not found locally, will pull", rid);
            to_pull_ids.push(rid); 
        }
    }

    // Check if mobile has messages that desktop doesn't have
    for lid in local_map.keys() {
        if !remote_ids.contains(lid) {
            println!("[SyncManager] Local message {} not found remotely, will push", lid);
            to_push = true;
            break;
        }
    }

    println!("[SyncManager] Delta sync result: to_pull={}, to_push={}", to_pull_ids.len(), to_push);

    if to_push { 
        println!("[SyncManager] Starting history push for topic {}", topic_id);
        let _ = perform_history_push(app, client, http_url, topic_id).await; 
    }
    
    if !to_pull_ids.is_empty() {
        // 先检查 topic 是否存在于本地，不存在则跳过（等待 topic 元数据同步后再处理）
        let topic_exists = sqlx::query("SELECT 1 FROM topics WHERE topic_id = ?")
            .bind(topic_id)
            .fetch_optional(pool)
            .await
            .map_err(|e| e.to_string())?;
        
        if topic_exists.is_none() {
            println!("[SyncManager] WARN: Topic {} not found locally, skipping message sync (will sync after topic metadata)", topic_id);
            return Ok(());
        }
        
        let metadata_row = sqlx::query("SELECT owner_id, owner_type FROM topics WHERE topic_id = ?")
            .bind(topic_id)
            .fetch_optional(pool)
            .await
            .map_err(|e| e.to_string())?;

        let (agent_id, owner_type) = match metadata_row {
            Some(row) => {
                use sqlx::Row;
                (row.get::<String, _>("owner_id"), row.get::<String, _>("owner_type"))
            },
            None => {
                println!("[SyncManager] DEBUG: Topic metadata for {} disappeared during delta sync, skipping.", topic_id);
                return Ok(());
            }
        };

        let settings = crate::vcp_modules::settings_manager::read_settings(app.clone(), app.state()).await?;
        let url = format!("{}/api/mobile-sync/download-messages", http_url);
        
        println!("[SyncManager] Downloading {} messages from {}", to_pull_ids.len(), url);
        
        let res = client.post(&url).header("x-sync-token", &settings.sync_token).json(&json!({ "topicId": topic_id, "msgIds": to_pull_ids })).send().await.map_err(|e| e.to_string())?;
        if res.status().is_success() {
            let messages: Vec<Value> = res.json().await.map_err(|e| e.to_string())?;
            println!("[SyncManager] Downloaded {} messages", messages.len());
            
            for m_val in messages {
                if let Ok(msg) = serde_json::from_value::<crate::vcp_modules::chat_manager::ChatMessage>(m_val) {
                    crate::vcp_modules::message_service::patch_single_message(app.clone(), pool, &agent_id, &owner_type, topic_id.to_string(), msg, true).await?;
                }
            }
            let msg_count: i32 = sqlx::query_scalar::<sqlx::Sqlite, i64>("SELECT COUNT(*) FROM messages WHERE topic_id = ? AND deleted_at IS NULL")
                .bind(topic_id).fetch_optional(pool).await.map_err(|e| e.to_string())?.unwrap_or(0) as i32;
            sqlx::query("UPDATE topics SET msg_count = ? WHERE topic_id = ?").bind(msg_count).bind(topic_id).execute(pool).await.map_err(|e| e.to_string())?;
            println!("[SyncManager] Updated topic {} msg_count to {}", topic_id, msg_count);
        } else {
            println!("[SyncManager] ERROR: Download messages failed with status: {}", res.status());
        }
    }
    
    println!("[SyncManager] History delta sync completed for topic {}", topic_id);
    Ok(())
}

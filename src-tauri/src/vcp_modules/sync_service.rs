use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::sync_types::{EntityState, SyncDataType, SyncManifest};
use crate::vcp_modules::sync_dto::{AgentSyncDTO, GroupSyncDTO, AgentTopicSyncDTO, GroupTopicSyncDTO, UserMessageSyncDTO, AgentMessageSyncDTO, GroupMessageSyncDTO};
use crate::vcp_modules::agent_service::{self, AgentConfigState};
use crate::vcp_modules::group_service::{self, GroupManagerState};
use crate::vcp_modules::hash_aggregator::HashAggregator;
use crate::vcp_modules::sync_retry::RetryPolicy;
use crate::vcp_modules::sync_metrics::SyncMetrics;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, Runtime, State};
use tokio::sync::{mpsc, RwLock, Semaphore};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use dashmap::DashMap;

/// =================================================================
/// vcp_modules/sync_service.rs - 手机端同步调度中心 (3-Phase Pipeline)
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
            semaphore: Arc::new(Semaphore::new(10)), // Default 10
            network_type: Arc::new(RwLock::new(NetworkType::Unknown)),
            success_streak: AtomicU32::new(0),
            failure_streak: AtomicU32::new(0),
        }
    }

    pub async fn acquire(&self) -> tokio::sync::SemaphorePermit<'_> {
        self.semaphore.acquire().await.unwrap()
    }

    pub fn on_success(&self) {
        self.success_streak.fetch_add(1, Ordering::Relaxed);
        self.failure_streak.store(0, Ordering::Relaxed);
    }

    pub fn on_failure(&self) {
        self.success_streak.store(0, Ordering::Relaxed);
        self.failure_streak.fetch_add(1, Ordering::Relaxed);
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
    let _ = app_handle.emit("vcp-sync-status", json!({ "status": next_status }));
}

pub fn init_sync_service(app_handle: AppHandle) -> SyncState {
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
        
        let mut pipeline_agents = std::collections::HashSet::<String>::new();
        let mut pipeline_groups = std::collections::HashSet::<String>::new();
        let mut pipeline_topics = std::collections::HashSet::<String>::new();
        let mut pipeline_messages = std::collections::HashMap::<String, Vec<Value>>::new();
        let mut pipeline_active = false;
        let mut last_pipeline_update = Instant::now();

        loop {
            let (ws_url, http_url) = {
                let settings_state = handle_clone.state::<crate::vcp_modules::settings_manager::SettingsState>();
                match crate::vcp_modules::settings_manager::read_settings(handle_clone.clone(), settings_state).await {
                    Ok(s) => {
                        if s.sync_server_url.is_empty() || s.sync_http_url.is_empty() {
                            tokio::time::sleep(Duration::from_secs(10)).await;
                            continue;
                        }
                        let ws_addr = if let Ok(mut u) = url::Url::parse(&s.sync_server_url) {
                            u.set_query(Some(&format!("token={}", s.sync_token)));
                            u.to_string()
                        } else {
                            format!("ws://127.0.0.1:5975?token={}", s.sync_token)
                        };
                        (ws_addr, s.sync_http_url.clone())
                    }
                    Err(_) => {
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        continue;
                    }
                }
            };

            publish_sync_status(&handle_clone, &connection_status_for_task, "connecting").await;

            match connect_async(&ws_url).await {
                Ok((mut ws_stream, _)) => {
                    println!("[SyncService] WebSocket Connected.");
                    publish_sync_status(&handle_clone, &connection_status_for_task, "connected").await;
                    
                    if let Ok(manifests) = generate_initial_manifests(&handle_clone).await {
                        for manifest in manifests {
                            let msg = json!({ "type": "SYNC_MANIFEST", "data": manifest.items, "dataType": manifest.data_type });
                            let _ = ws_stream.send(Message::Text(msg.to_string().into())).await;
                        }
                    }

                    if let Ok(topic_ids) = get_all_active_topic_ids(&handle_clone).await {
                        for tid in topic_ids {
                            let msg = json!({ "type": "GET_MESSAGE_MANIFEST", "topicId": tid });
                            let _ = ws_stream.send(Message::Text(msg.to_string().into())).await;
                        }
                    }

                    loop {
                        tokio::select! {
                            _ = tokio::time::sleep(Duration::from_millis(500)), if pipeline_active => {
                                if last_pipeline_update.elapsed() > Duration::from_millis(2000) {
                                    pipeline_active = false;
                                    let agents: Vec<String> = pipeline_agents.drain().collect();
                                    let groups: Vec<String> = pipeline_groups.drain().collect();
                                    let topics: Vec<String> = pipeline_topics.drain().collect();
                                    let mut messages = std::collections::HashMap::<String, Vec<Value>>::new();
                                    std::mem::swap(&mut messages, &mut pipeline_messages);
                                    
                                    let h_inner = handle_clone.clone();
                                    let c_inner = http_client.clone();
                                    let base_inner = http_url.clone();
                                    let tx_pipe = tx_internal.clone();
                                    let metrics_inner = metrics_task.clone();
                                    let sem_inner = semaphore_task.clone();
                                    let op_tracker_inner = op_tracker_task.clone();

                                    tauri::async_runtime::spawn(async move {
                                        println!("[SyncPipeline] Executing. A:{}, G:{}, T:{}, M:{}", agents.len(), groups.len(), topics.len(), messages.len());
                                        
                                        metrics_inner.record_start();
                                        let _permit = sem_inner.acquire().await;

                                        // Phase 1: Agents & Groups
                                        for id in &agents {
                                            let _ = perform_pull(&h_inner, &c_inner, &base_inner, id, "agent", true).await;
                                        }
                                        for id in &groups {
                                            let _ = perform_pull(&h_inner, &c_inner, &base_inner, id, "group", true).await;
                                        }

                                        // Phase 2: Topics
                                        for id in &topics {
                                            if perform_pull(&h_inner, &c_inner, &base_inner, id, "topic", true).await.is_ok() {
                                                let _ = tx_pipe.send(SyncCommand::RequestMessageManifest { topic_id: id.clone() });
                                            }
                                        }

                                        // Phase 3: Messages
                                        for (topic_id, msgs) in &messages {
                                            let _ = perform_history_delta_sync(&h_inner, &c_inner, &base_inner, topic_id, msgs, true).await;
                                        }

                                        // Phase 4: Batch Hash Recalculation
                                        let db = h_inner.state::<DbState>();
                                        let pool = &db.pool;
                                        for id in &topics {
                                            if let Ok(mut tx) = pool.begin().await {
                                                let _ = HashAggregator::bubble_from_topic(&mut tx, id).await;
                                                let _ = tx.commit().await;
                                            }
                                        }
                                        for (topic_id, _) in &messages {
                                            if !topics.contains(topic_id) {
                                                if let Ok(mut tx) = pool.begin().await {
                                                    let _ = HashAggregator::bubble_from_topic(&mut tx, topic_id).await;
                                                    let _ = tx.commit().await;
                                                }
                                            }
                                        }
                                        for id in &agents {
                                            if let Ok(mut tx) = pool.begin().await {
                                                let _ = HashAggregator::bubble_agent_hash(&mut tx, id).await;
                                                let _ = tx.commit().await;
                                            }
                                        }
                                        for id in &groups {
                                            if let Ok(mut tx) = pool.begin().await {
                                                let _ = HashAggregator::bubble_group_hash(&mut tx, id).await;
                                                let _ = tx.commit().await;
                                            }
                                        }
                                        
                                        metrics_inner.record_success();
                                        sem_inner.on_success();
                                        metrics_inner.emit_to_frontend(&h_inner);
                                        println!("[SyncPipeline] Completed.");
                                    });
                                }
                            }
                            Some(cmd) = rx.recv() => {
                                match cmd {
                                    SyncCommand::NotifyLocalChange { id, data_type, hash, ts } => {
                                        let msg = json!({ "type": "SYNC_ENTITY_UPDATE", "id": id, "dataType": data_type, "hash": hash, "ts": ts });
                                        let _ = ws_stream.send(Message::Text(msg.to_string().into())).await;
                                    },
                                    SyncCommand::StartFullSync => {
                                        if let Ok(manifests) = generate_initial_manifests(&handle_clone).await {
                                            for manifest in manifests {
                                                let _ = ws_stream.send(Message::Text(json!({"type":"SYNC_MANIFEST","data":manifest.items,"dataType":manifest.data_type}).to_string().into())).await;
                                            }
                                        }
                                    },
                                    SyncCommand::RequestMessageManifest { topic_id } => {
                                        let msg = json!({ "type": "GET_MESSAGE_MANIFEST", "topicId": topic_id });
                                        let _ = ws_stream.send(Message::Text(msg.to_string().into())).await;
                                    }
                                }
                            }
                            Some(Ok(msg)) = ws_stream.next() => {
                                if let Message::Text(text) = msg {
                                    let payload: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
                                    if payload.is_null() { continue; }
                                    
                                    let h = handle_clone.clone();
                                    let c = http_client.clone();
                                    let base = http_url.clone();
                                    let sem = semaphore_task.clone();
                                    let _met = metrics_task.clone();
                                    let _opt = op_tracker_task.clone();
                                    
                                    match payload["type"].as_str() {
                                        Some("SYNC_ENTITY_UPDATE") => {
                                            let id = payload["id"].as_str().unwrap_or_default().to_string();
                                            let Some(data_type) = parse_sync_data_type(&payload["dataType"]) else { continue; };
                                            if data_type == SyncDataType::Message {
                                                let _ = ws_stream.send(Message::Text(json!({ "type": "GET_MESSAGE_MANIFEST", "topicId": id }).to_string().into())).await;
                                            } else {
                                                let et_str = stringify_sync_data_type(&data_type);
                                                tauri::async_runtime::spawn(async move {
                                                    let _permit = sem.acquire().await;
                                                    let _ = perform_pull(&h, &c, &base, &id, &et_str, false).await;
                                                });
                                            }
                                        },
                                        Some("SYNC_DIFF_RESULTS") => {
                                            if let Some(items) = payload["data"].as_array() {
                                                let Some(data_type) = parse_sync_data_type(&payload["dataType"]) else { continue; };
                                                let et_str = stringify_sync_data_type(&data_type);
                                                
                                                pipeline_active = true;
                                                last_pipeline_update = Instant::now();
                                                
                                                for item in items {
                                                    let id = item["id"].as_str().unwrap_or_default().to_string();
                                                    let action = item["action"].as_str().unwrap_or_default();
                                                    if action == "PULL" {
                                                        match data_type {
                                                            SyncDataType::Agent => { pipeline_agents.insert(id); },
                                                            SyncDataType::Group => { pipeline_groups.insert(id); },
                                                            SyncDataType::Topic => { pipeline_topics.insert(id); },
                                                            _ => {
                                                                let h_in = h.clone(); let c_in = c.clone(); let b_in = base.clone(); let s_in = sem.clone(); let e_in = et_str.clone();
                                                                tauri::async_runtime::spawn(async move {
                                                                    let _permit = s_in.acquire().await;
                                                                    let _ = perform_pull(&h_in, &c_in, &b_in, &id, &e_in, false).await;
                                                                });
                                                            }
                                                        }
                                                    } else if action == "PUSH" {
                                                        let h_in = h.clone(); let c_in = c.clone(); let b_in = base.clone(); let s_in = sem.clone(); let e_in = et_str.clone();
                                                        tauri::async_runtime::spawn(async move {
                                                            let _permit = s_in.acquire().await;
                                                            let _ = perform_push(&h_in, &c_in, &b_in, &id, &e_in).await;
                                                        });
                                                    }
                                                }
                                            }
                                        },
                                        Some("MESSAGE_MANIFEST_RESULTS") => {
                                            let topic_id = payload["topicId"].as_str().unwrap_or_default().to_string();
                                            if let Some(remote_msgs) = payload["messages"].as_array() {
                                                pipeline_active = true;
                                                last_pipeline_update = Instant::now();
                                                pipeline_messages.insert(topic_id, remote_msgs.clone());
                                            }
                                        },
                                        _ => {}
                                    }
                                }
                            }
                            else => break,
                        }
                    }
                }
                Err(_) => {
                    publish_sync_status(&handle_clone, &connection_status_for_task, "disconnected").await;
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            }
        }
    });

    SyncState { ws_sender: tx, connection_status, op_tracker, metrics, network_semaphore }
}

#[tauri::command]
pub async fn get_sync_status(state: State<'_, SyncState>) -> Result<String, String> {
    Ok(state.connection_status.read().await.clone())
}

async fn generate_initial_manifests(app: &AppHandle) -> Result<Vec<SyncManifest>, String> {
    let db_state = app.state::<DbState>();
    let pool = &db_state.pool;
    let mut manifests = Vec::new();

    // Agents
    let mut agent_items = Vec::new();
    let rows = sqlx::query("SELECT agent_id, config_hash, content_hash, updated_at FROM agents WHERE deleted_at IS NULL").fetch_all(pool).await.map_err(|e| e.to_string())?;
    for r in rows {
        use sqlx::Row;
        let h = HashAggregator::aggregate_agent_manifest_hash(r.get("config_hash"), r.get("content_hash"));
        agent_items.push(EntityState { id: r.get("agent_id"), hash: h, ts: r.get("updated_at") });
    }
    manifests.push(SyncManifest { data_type: SyncDataType::Agent, items: agent_items });

    // Groups
    let mut group_items = Vec::new();
    let rows = sqlx::query("SELECT group_id, config_hash, content_hash, updated_at FROM groups WHERE deleted_at IS NULL").fetch_all(pool).await.map_err(|e| e.to_string())?;
    for r in rows {
        use sqlx::Row;
        let h = HashAggregator::aggregate_group_manifest_hash(r.get("config_hash"), r.get("content_hash"));
        group_items.push(EntityState { id: r.get("group_id"), hash: h, ts: r.get("updated_at") });
    }
    manifests.push(SyncManifest { data_type: SyncDataType::Group, items: group_items });

    // Avatars
    let mut avatar_items = Vec::new();
    let rows = sqlx::query("SELECT owner_id, owner_type, avatar_hash, updated_at FROM avatars").fetch_all(pool).await.map_err(|e| e.to_string())?;
    for r in rows {
        use sqlx::Row;
        avatar_items.push(EntityState { id: format!("{}:{}", r.get::<String, _>("owner_type"), r.get::<String, _>("owner_id")), hash: r.get("avatar_hash"), ts: r.get("updated_at") });
    }
    manifests.push(SyncManifest { data_type: SyncDataType::Avatar, items: avatar_items });

    // Topics
    let mut topic_items = Vec::new();
    let rows = sqlx::query("SELECT topic_id, title, created_at, locked, unread, content_hash, updated_at, owner_id, owner_type FROM topics WHERE deleted_at IS NULL").fetch_all(pool).await.map_err(|e| e.to_string())?;
    for r in rows {
        use sqlx::Row;
        let id: String = r.get("topic_id");
        let owner_type: String = r.get("owner_type");
        let hash = if owner_type == "group" {
            let dto = GroupTopicSyncDTO {
                id: id.clone(),
                name: r.get("title"),
                created_at: r.get("created_at"),
                owner_id: r.get("owner_id"),
            };
            HashAggregator::aggregate_topic_manifest_hash(&HashAggregator::compute_group_topic_metadata_hash(&dto), r.get("content_hash"))
        } else {
            let dto = AgentTopicSyncDTO {
                id: id.clone(),
                name: r.get("title"),
                created_at: r.get("created_at"),
                locked: r.get::<i64, _>("locked") != 0,
                unread: r.get::<i64, _>("unread") != 0,
                owner_id: r.get("owner_id"),
            };
            HashAggregator::aggregate_topic_manifest_hash(&HashAggregator::compute_agent_topic_metadata_hash(&dto), r.get("content_hash"))
        };
        topic_items.push(EntityState { id, hash, ts: r.get("updated_at") });
    }
    manifests.push(SyncManifest { data_type: SyncDataType::Topic, items: topic_items });

    Ok(manifests)
}

async fn get_all_active_topic_ids(app: &AppHandle) -> Result<Vec<String>, String> {
    let db_state = app.state::<DbState>();
    let rows = sqlx::query("SELECT topic_id FROM topics WHERE deleted_at IS NULL").fetch_all(&db_state.pool).await.map_err(|e| e.to_string())?;
    Ok(rows.into_iter().map(|r| { use sqlx::Row; r.get(0) }).collect())
}

async fn perform_pull<R: Runtime>(app: &AppHandle<R>, client: &reqwest::Client, http_url: &str, id: &str, entity_type: &str, skip_bubble: bool) -> Result<(), String> {
    let settings_state = app.state::<crate::vcp_modules::settings_manager::SettingsState>();
    let settings = crate::vcp_modules::settings_manager::read_settings(app.clone(), settings_state).await?;
    let url = format!("{}/api/mobile-sync/download-entity?id={}&type={}", http_url, id, entity_type);
    let res = client.get(&url).header("x-sync-token", &settings.sync_token).send().await.map_err(|e| e.to_string())?;
    if !res.status().is_success() { return Err(format!("Pull failed: {}", res.status())); }

    if entity_type == "agent" {
        let dto: AgentSyncDTO = res.json().await.map_err(|e| e.to_string())?;
        agent_service::apply_sync_update(app, &app.state::<AgentConfigState>(), id, dto, skip_bubble).await?;
    } else if entity_type == "group" {
        let dto: GroupSyncDTO = res.json().await.map_err(|e| e.to_string())?;
        group_service::apply_sync_update(app, &app.state::<GroupManagerState>(), id, dto, skip_bubble).await?;
    } else if entity_type == "avatar" {
        let parts: Vec<&str> = id.split(':').collect();
        if parts.len() == 2 {
            let (ot, oi) = (parts[0], parts[1]);
            let url = format!("{}/api/mobile-sync/download-avatar?id={}", http_url, oi);
            if let Ok(resp) = client.get(&url).header("x-sync-token", &settings.sync_token).send().await {
                if let Ok(bytes) = resp.bytes().await {
                    let hash = HashAggregator::compute_avatar_hash(&bytes);
                    sqlx::query("INSERT INTO avatars (owner_type, owner_id, avatar_hash, mime_type, image_data, updated_at) VALUES (?, ?, ?, 'image/png', ?, ?) ON CONFLICT(owner_type, owner_id) DO UPDATE SET avatar_hash=excluded.avatar_hash, image_data=excluded.image_data, updated_at=excluded.updated_at")
                        .bind(ot).bind(oi).bind(&bytes[..]).bind(chrono::Utc::now().timestamp_millis()).execute(&app.state::<DbState>().pool).await.ok();
                }
            }
        }
    } else if entity_type == "agent_topic" || entity_type == "topic" {
        let dto: AgentTopicSyncDTO = res.json().await.map_err(|e| e.to_string())?;
        sqlx::query("INSERT INTO topics (topic_id, title, owner_id, owner_type, created_at, locked, unread, updated_at) VALUES (?, ?, ?, 'agent', ?, ?, ?, ?) ON CONFLICT(topic_id) DO UPDATE SET title=excluded.title, locked=excluded.locked, unread=excluded.unread, updated_at=excluded.updated_at")
            .bind(id).bind(&dto.name).bind(&dto.owner_id).bind(dto.created_at).bind(if dto.locked {1} else {0}).bind(if dto.unread {1} else {0}).bind(chrono::Utc::now().timestamp_millis()).execute(&app.state::<DbState>().pool).await.map_err(|e| e.to_string())?;
    } else if entity_type == "group_topic" {
        let dto: GroupTopicSyncDTO = res.json().await.map_err(|e| e.to_string())?;
        sqlx::query("INSERT INTO topics (topic_id, title, owner_id, owner_type, created_at, locked, unread, updated_at) VALUES (?, ?, ?, 'group', ?, 1, 0, ?) ON CONFLICT(topic_id) DO UPDATE SET title=excluded.title, updated_at=excluded.updated_at")
            .bind(id).bind(&dto.name).bind(&dto.owner_id).bind(dto.created_at).bind(chrono::Utc::now().timestamp_millis()).execute(&app.state::<DbState>().pool).await.map_err(|e| e.to_string())?;
    }
    Ok(())
}

async fn perform_push(app: &AppHandle, client: &reqwest::Client, http_url: &str, id: &str, entity_type: &str) -> Result<(), String> {
    let db = app.state::<DbState>();
    let settings = crate::vcp_modules::settings_manager::read_settings(app.clone(), app.state()).await?;
    let payload = if entity_type == "agent_topic" || entity_type == "topic" {
        let r = sqlx::query("SELECT topic_id, title, created_at, locked, unread, owner_id FROM topics WHERE topic_id = ?").bind(id).fetch_optional(&db.pool).await.map_err(|e| e.to_string())?;
        if let Some(row) = r {
            use sqlx::Row;
            let dto = AgentTopicSyncDTO {
                id: row.get("topic_id"),
                name: row.get("title"),
                created_at: row.get("created_at"),
                locked: row.get::<i64, _>("locked") != 0,
                unread: row.get::<i64, _>("unread") != 0,
                owner_id: row.get("owner_id"),
            };
            json!({ "id": id, "type": "agent_topic", "data": dto })
        } else { return Ok(()); }
    } else if entity_type == "group_topic" {
        let r = sqlx::query("SELECT topic_id, title, created_at, owner_id FROM topics WHERE topic_id = ?").bind(id).fetch_optional(&db.pool).await.map_err(|e| e.to_string())?;
        if let Some(row) = r {
            use sqlx::Row;
            let dto = GroupTopicSyncDTO {
                id: row.get("topic_id"),
                name: row.get("title"),
                created_at: row.get("created_at"),
                owner_id: row.get("owner_id"),
            };
            json!({ "id": id, "type": "group_topic", "data": dto })
        } else { return Ok(()); }
    } else if entity_type == "agent" {
        let config = agent_service::read_agent_config(app.clone(), app.state(), id.to_string(), None).await?;
        json!({ "id": id, "type": "agent", "data": AgentSyncDTO::from(&config) })
    } else if entity_type == "group" {
        let config = group_service::read_group_config(app.clone(), app.state(), id.to_string()).await?;
        json!({ "id": id, "type": "group", "data": GroupSyncDTO::from(&config) })
    } else if entity_type == "avatar" {
        let parts: Vec<&str> = id.split(':').collect();
        if parts.len() == 2 {
            if let Ok(Some(row)) = sqlx::query("SELECT image_data, mime_type FROM avatars WHERE owner_id = ? AND owner_type = ?").bind(parts[1]).bind(parts[0]).fetch_optional(&db.pool).await {
                use sqlx::Row;
                let _ = client.post(&format!("{}/api/mobile-sync/upload-avatar", http_url)).header("x-sync-token", &settings.sync_token).header("Content-Type", row.get::<String, _>("mime_type")).query(&[("id", parts[1]), ("type", parts[0])]).body(row.get::<Vec<u8>, _>("image_data")).send().await;
            }
        }
        return Ok(());
    } else { return Ok(()); };

    let _ = client.post(&format!("{}/api/mobile-sync/upload-entity", http_url)).header("x-sync-token", &settings.sync_token).header("x-idempotency-key", generate_idempotency_key("push", entity_type, id)).json(&payload).send().await;
    if entity_type == "message" { let _ = perform_history_push(app, client, http_url, id).await; }
    Ok(())
}

async fn perform_history_push(app: &AppHandle, client: &reqwest::Client, http_url: &str, topic_id: &str) -> Result<(), String> {
    let db = app.state::<DbState>();
    if let Ok(Some(row)) = sqlx::query("SELECT owner_id, owner_type FROM topics WHERE topic_id = ?").bind(topic_id).fetch_optional(&db.pool).await {
        use sqlx::Row;
        let (oi, ot) = (row.get::<String, _>("owner_id"), row.get::<String, _>("owner_type"));
        let history = crate::vcp_modules::message_service::load_chat_history_internal(app, &oi, &ot, topic_id, Some(1000), None).await?;
        let settings = crate::vcp_modules::settings_manager::read_settings(app.clone(), app.state()).await?;
        
        let dto_messages = build_message_dtos(app, &history, &ot).await;
        
        let _ = client.post(&format!("{}/api/mobile-sync/upload-messages", http_url))
            .header("x-sync-token", &settings.sync_token)
            .json(&json!({ "topicId": topic_id, "messages": dto_messages }))
            .send().await;
    }
    Ok(())
}

async fn build_message_dtos(app: &AppHandle, history: &[crate::vcp_modules::chat_manager::ChatMessage], owner_type: &str) -> Vec<Value> {
    let db = app.state::<DbState>();
    let mut results = Vec::new();
    
    for msg in history {
        let msg_value = if msg.role == "user" {
            let dto = UserMessageSyncDTO::from(msg);
            serde_json::to_value(dto).ok()
        } else if owner_type == "group" {
            let avatar_color = query_avatar_color(&db.pool, &msg.agent_id.clone().unwrap_or_default()).await;
            let dto = GroupMessageSyncDTO::from_message(msg, avatar_color);
            serde_json::to_value(dto).ok()
        } else {
            let avatar_color = query_avatar_color(&db.pool, &msg.agent_id.clone().unwrap_or_default()).await;
            let dto = AgentMessageSyncDTO::from_message(msg, avatar_color);
            serde_json::to_value(dto).ok()
        };
        
        if let Some(v) = msg_value {
            results.push(v);
        }
    }
    
    results
}

async fn query_avatar_color(pool: &sqlx::SqlitePool, agent_id: &str) -> String {
    if agent_id.is_empty() {
        return "rgb(128, 128, 128)".to_string();
    }
    
    sqlx::query_scalar::<sqlx::Sqlite, Option<String>>(
        "SELECT dominant_color FROM avatars WHERE owner_id = ? AND owner_type = 'agent' AND deleted_at IS NULL"
    )
    .bind(agent_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .flatten()
    .unwrap_or_else(|| "rgb(128, 128, 128)".to_string())
}

async fn perform_history_delta_sync(app: &AppHandle, client: &reqwest::Client, http_url: &str, topic_id: &str, remote_msgs: &Vec<Value>, skip_bubble: bool) -> Result<(), String> {
    let db = app.state::<DbState>();
    let pool = &db.pool;
    let rows = sqlx::query("SELECT m.msg_id, m.content, m.updated_at, a.hash as att_hash FROM messages m LEFT JOIN message_attachments ma ON m.msg_id = ma.msg_id LEFT JOIN attachments a ON ma.hash = a.hash WHERE m.topic_id = ? AND m.deleted_at IS NULL").bind(topic_id).fetch_all(pool).await.map_err(|e| e.to_string())?;
    let mut local_map = std::collections::HashMap::<String, (String, i64, Vec<String>)>::new();
    for r in rows {
        use sqlx::Row;
        let id: String = r.get("msg_id");
        let entry = local_map.entry(id).or_insert((r.get("content"), r.get("updated_at"), Vec::new()));
        if let Some(h) = r.get::<Option<String>, _>("att_hash") { entry.2.push(h); }
    }

    let mut to_pull_ids = Vec::new();
    let mut to_push = false;
    let mut remote_ids = std::collections::HashSet::new();
    for rm in remote_msgs {
        let rid = rm["msg_id"].as_str().unwrap_or_default().to_string();
        remote_ids.insert(rid.clone());
        if let Some((lcontent, lts, latts)) = local_map.get(&rid) {
            if HashAggregator::compute_message_fingerprint(lcontent, latts) != rm["content_hash"].as_str().unwrap_or_default() {
                if rm["updated_at"].as_i64().unwrap_or(0) > *lts { to_pull_ids.push(rid); } else { to_push = true; }
            }
        } else { to_pull_ids.push(rid); }
    }
    for lid in local_map.keys() { if !remote_ids.contains(lid) { to_push = true; break; } }

    if to_push { let _ = perform_history_push(app, client, http_url, topic_id).await; }
    if !to_pull_ids.is_empty() {
        if let Ok(Some(row)) = sqlx::query("SELECT owner_id, owner_type FROM topics WHERE topic_id = ?").bind(topic_id).fetch_optional(pool).await {
            use sqlx::Row;
            let (oi, ot) = (row.get::<String, _>("owner_id"), row.get::<String, _>("owner_type"));
            let settings = crate::vcp_modules::settings_manager::read_settings(app.clone(), app.state()).await?;
            if let Ok(res) = client.post(&format!("{}/api/mobile-sync/download-messages", http_url)).header("x-sync-token", &settings.sync_token).json(&json!({ "topicId": topic_id, "msgIds": to_pull_ids })).send().await {
                if res.status().is_success() {
                    if let Ok(messages) = res.json::<Vec<Value>>().await {
                        for m_val in messages {
                            if let Ok(msg) = serde_json::from_value::<crate::vcp_modules::chat_manager::ChatMessage>(m_val) {
                                let _ = crate::vcp_modules::message_service::patch_single_message(app.clone(), pool, &oi, &ot, topic_id.to_string(), msg, skip_bubble).await;
                            }
                        }
                        let count: i32 = sqlx::query_scalar::<sqlx::Sqlite, i64>("SELECT COUNT(*) FROM messages WHERE topic_id = ? AND deleted_at IS NULL").bind(topic_id).fetch_optional(pool).await.ok().flatten().unwrap_or(0) as i32;
                        let _ = sqlx::query("UPDATE topics SET msg_count = ? WHERE topic_id = ?").bind(count).bind(topic_id).execute(pool).await;
                    }
                }
            }
        }
    }
    Ok(())
}

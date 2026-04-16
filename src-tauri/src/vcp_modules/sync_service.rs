use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::sync_types::SyncDataType;
use crate::vcp_modules::sync_pipeline::{SyncPipeline, Phase1Metadata, Phase3Message};
use crate::vcp_modules::sync_hash::{HashAggregator, HashInitializer};
use crate::vcp_modules::sync_executor::{PullExecutor, PushExecutor};
use crate::vcp_modules::sync_manifest::ManifestBuilder;
use crate::vcp_modules::sync_utils::{RetryPolicy, SyncMetrics};
use crate::vcp_modules::db_write_queue::DbWriteQueue;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, Runtime, State};
use tokio::sync::{mpsc, RwLock, Semaphore};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use dashmap::DashMap;

pub struct SyncState {
    pub ws_sender: mpsc::UnboundedSender<SyncCommand>,
    pub connection_status: Arc<RwLock<String>>,
    pub op_tracker: Arc<SyncOperationTracker>,
    pub metrics: Arc<SyncMetrics>,
    pub network_semaphore: Arc<NetworkAwareSemaphore>,
    pub pipeline: Arc<SyncPipeline>,
    pub uploaded_hashes: Arc<RwLock<HashSet<String>>>,
    pub write_queue: Arc<DbWriteQueue>,
    pub pending_tasks: Arc<AtomicU32>,
}

pub struct SyncOperationTracker {
    in_progress: DashMap<String, Instant>,
    ttl: Duration,
}

impl SyncOperationTracker {
    pub fn new() -> Self {
        Self {
            in_progress: DashMap::new(),
            ttl: Duration::from_secs(300),
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
    WiFi,
    Cell5G,
    Cell4G,
    Unknown,
}

impl NetworkAwareSemaphore {
    pub fn new() -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(10)),
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

pub enum SyncCommand {
    NotifyLocalChange { id: String, data_type: SyncDataType, hash: String, ts: i64 },
    StartFullSync,
    RequestMessageManifest { topic_id: String },
    Phase1Completed,
    Phase2Completed,
    Phase3Completed,
    NotifyDelete { data_type: SyncDataType, id: String },
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
    let (pipeline_tx, mut pipeline_rx) = mpsc::unbounded_channel::<crate::vcp_modules::sync_pipeline::pipeline::PipelineCommand>();
    
    let handle_clone = app_handle.clone();
    let tx_internal = tx.clone();
    let connection_status = Arc::new(RwLock::new(String::from("connecting")));
    let connection_status_for_task = connection_status.clone();
    let op_tracker = Arc::new(SyncOperationTracker::new());
    let metrics = Arc::new(SyncMetrics::new());
    let network_semaphore = Arc::new(NetworkAwareSemaphore::new());
    let pipeline = Arc::new(SyncPipeline::new(pipeline_tx));
    let pending_tasks = Arc::new(AtomicU32::new(0));
    
    let db = app_handle.state::<DbState>();
    let write_queue = Arc::new(DbWriteQueue::new(db.pool.clone()));
    
    let op_tracker_task = op_tracker.clone();
    let metrics_task = metrics.clone();
    let semaphore_task = network_semaphore.clone();
    let pipeline_task = pipeline.clone();
    let write_queue_task = write_queue.clone();
    let pending_tasks_task = pending_tasks.clone();

    tauri::async_runtime::spawn(async move {
        let http_client = reqwest::Client::new();
        let retry_policy = RetryPolicy::default();

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

                    let db = handle_clone.state::<DbState>();
                    if let Err(e) = HashInitializer::ensure_all_agent_hashes(&db.pool).await {
                        println!("[SyncService] Hash init error: {}", e);
                    }
                    if let Err(e) = HashInitializer::ensure_all_group_hashes(&db.pool).await {
                        println!("[SyncService] Hash init error: {}", e);
                    }

                    if let Ok(manifests) = Phase1Metadata::build_all_manifests(&db.pool).await {
                        for manifest in manifests {
                            let msg = json!({ "type": "SYNC_MANIFEST", "data": manifest.items, "dataType": manifest.data_type, "phase": "metadata" });
                            let _ = ws_stream.send(Message::Text(msg.to_string().into())).await;
                        }
                    }

                    if let Ok(topic_ids) = Phase3Message::get_all_active_topic_ids(&db.pool).await {
                        for tid in topic_ids {
                            let msg = json!({ "type": "GET_MESSAGE_MANIFEST", "topicId": tid });
                            let _ = ws_stream.send(Message::Text(msg.to_string().into())).await;
                        }
                    }

                    loop {
                        tokio::select! {
                            Some(cmd) = pipeline_rx.recv() => {
                                match cmd {
                                    crate::vcp_modules::sync_pipeline::pipeline::PipelineCommand::StartFullSync => {
                                        let _ = ws_stream.send(Message::Text(json!({ "type": "PHASE_START", "phase": "metadata" }).to_string().into())).await;
                                    },
                                    crate::vcp_modules::sync_pipeline::pipeline::PipelineCommand::Phase1Completed => {
                                        let _ = ws_stream.send(Message::Text(json!({ "type": "PHASE_START", "phase": "topic" }).to_string().into())).await;
                                    },
                                    crate::vcp_modules::sync_pipeline::pipeline::PipelineCommand::Phase2Completed => {
                                        let _ = ws_stream.send(Message::Text(json!({ "type": "PHASE_START", "phase": "message" }).to_string().into())).await;
                                    },
                                    crate::vcp_modules::sync_pipeline::pipeline::PipelineCommand::Phase3Completed => {
                                        let _ = ws_stream.send(Message::Text(json!({ "type": "PHASE_COMPLETED" }).to_string().into())).await;
                                    },
                                    crate::vcp_modules::sync_pipeline::pipeline::PipelineCommand::PhaseFailed { error, phase } => {
                                        println!("[SyncPipeline] Phase {} failed: {}", phase, error);
                                    },
                                }
                            },
                            Some(cmd) = rx.recv() => {
                                match cmd {
                                    SyncCommand::NotifyLocalChange { id, data_type, hash, ts } => {
                                        let msg = json!({ "type": "SYNC_ENTITY_UPDATE", "id": id, "dataType": data_type, "hash": hash, "ts": ts });
                                        let _ = ws_stream.send(Message::Text(msg.to_string().into())).await;
                                    },
                                    SyncCommand::StartFullSync => {
                                        let db = handle_clone.state::<DbState>();
                                        if let Ok(manifests) = ManifestBuilder::build_all_manifests(&db.pool).await {
                                            for manifest in manifests {
                                                let _ = ws_stream.send(Message::Text(json!({"type":"SYNC_MANIFEST","data":manifest.items,"dataType":manifest.data_type}).to_string().into())).await;
                                            }
                                        }
                                    },
                                    SyncCommand::RequestMessageManifest { topic_id } => {
                                        let msg = json!({ "type": "GET_MESSAGE_MANIFEST", "topicId": topic_id });
                                        let _ = ws_stream.send(Message::Text(msg.to_string().into())).await;
                                    },
                                    SyncCommand::Phase1Completed => {
                                        let db = handle_clone.state::<DbState>();
                                        let _ = pipeline_task.on_phase1_completed(&db.pool).await;
                                    },
                                    SyncCommand::Phase2Completed => {
                                        let _ = pipeline_task.on_phase2_completed().await;
                                    },
                                    SyncCommand::Phase3Completed => {
                                        let _ = pipeline_task.on_phase3_completed().await;
                                    },
                                    SyncCommand::NotifyDelete { data_type, id } => {
                                        let msg = json!({ "type": "SYNC_DELETE_NOTIFY", "id": id, "dataType": data_type });
                                        let _ = ws_stream.send(Message::Text(msg.to_string().into())).await;
                                    },
                                }
                            },
                            Some(Ok(msg)) = ws_stream.next() => {
                                if let Message::Text(text) = msg {
                                    let payload: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
                                    if payload.is_null() { continue; }

                                    let h = handle_clone.clone();
                                    let c = http_client.clone();
                                    let base = http_url.clone();
                                    let sem = semaphore_task.clone();
                                    let wq = write_queue_task.clone();

                                    match payload["type"].as_str() {
                                        Some("SYNC_ENTITY_UPDATE") => {
                                            let id = payload["id"].as_str().unwrap_or_default().to_string();
                                            let owner_type = payload["ownerType"].as_str().unwrap_or("agent").to_string();
                                            let Some(data_type) = parse_sync_data_type(&payload["dataType"]) else { continue; };
                                            if data_type == SyncDataType::Message {
                                                let _ = ws_stream.send(Message::Text(json!({ "type": "GET_MESSAGE_MANIFEST", "topicId": id }).to_string().into())).await;
                                            } else {
                                                tauri::async_runtime::spawn(async move {
                                                    let _permit = sem.acquire().await;
                                                    let settings = crate::vcp_modules::settings_manager::read_settings(h.clone(), h.state()).await.unwrap_or_default();
                                                    match data_type {
                                                        SyncDataType::Agent => { let _ = PullExecutor::pull_agent(&h, &c, &base, &settings.sync_token, &id, &wq).await; },
                                                        SyncDataType::Group => { let _ = PullExecutor::pull_group(&h, &c, &base, &settings.sync_token, &id, &wq).await; },
                                                        SyncDataType::Topic => {
                                                            if owner_type == "group" {
                                                                let _ = PullExecutor::pull_group_topic(&h, &c, &base, &settings.sync_token, &id, &wq).await;
                                                            } else {
                                                                let _ = PullExecutor::pull_agent_topic(&h, &c, &base, &settings.sync_token, &id, &wq).await;
                                                            }
                                                        },
                                                        _ => {}
                                                    }
                                                });
                                            }
                                        },
                                        Some("SYNC_DELETE_NOTIFY") => {
                                            use crate::vcp_modules::sync_executor::delete_executor::DeleteExecutor;
                                            let id = payload["id"].as_str().unwrap_or_default().to_string();
                                            let Some(data_type) = parse_sync_data_type(&payload["dataType"]) else { continue; };
                                            tauri::async_runtime::spawn(async move {
                                                match data_type {
                                                    SyncDataType::Agent => { let _ = DeleteExecutor::soft_delete_agent(&h, &id).await; },
                                                    SyncDataType::Group => { let _ = DeleteExecutor::soft_delete_group(&h, &id).await; },
                                                    SyncDataType::Topic => { let _ = DeleteExecutor::soft_delete_topic(&h, &id).await; },
                                                    SyncDataType::Avatar => {
                                                        let parts: Vec<&str> = id.split(':').collect();
                                                        if parts.len() == 2 {
                                                            let _ = DeleteExecutor::soft_delete_avatar(&h, parts[0], parts[1]).await;
                                                        }
                                                    },
                                                    _ => {}
                                                }
                                            });
                                        },
                                        Some("SYNC_DIFF_RESULTS") => {
                                            println!("[SyncService] Received SYNC_DIFF_RESULTS for dataType={}", payload["dataType"]);
                                            if let Some(items) = payload["data"].as_array() {
                                                let Some(data_type) = parse_sync_data_type(&payload["dataType"]) else {
                                                    println!("[SyncService] Failed to parse dataType: {:?}", payload["dataType"]);
                                                    continue;
                                                };
                                                println!("[SyncService] Parsed dataType: {:?}, items count: {}", data_type, items.len());
                                                let settings = crate::vcp_modules::settings_manager::read_settings(h.clone(), h.state()).await.unwrap_or_default();
                                                let items_clone: Vec<serde_json::Value> = items.clone();
                                                let pull_count = items_clone.iter().filter(|i| i["action"] == "PULL").count() as u32;
                                                pending_tasks_task.fetch_add(pull_count, Ordering::SeqCst);
                                                
                                                for item in items_clone {
                                                    let id = item["id"].as_str().unwrap_or_default().to_string();
                                                    let action = item["action"].as_str().unwrap_or_default().to_string();
                                                    
                                                    let h_in = h.clone();
                                                    let c_in = c.clone();
                                                    let b_in = base.clone();
                                                    let s_in = sem.clone();
                                                    let token = settings.sync_token.clone();
                                                    let data_type_clone = data_type.clone();
                                                    let wq_in = wq.clone();
                                                    let pending = pending_tasks_task.clone();
                                                    let tx_internal_in = tx_internal.clone();
                                                    
                                                    tauri::async_runtime::spawn(async move {
                                                        println!("[SyncService] Spawn task started: action={} dataType={:?} id={}", action, data_type_clone, id);
                                                        let _permit = s_in.acquire().await;
                                                        println!("[SyncService] Semaphore acquired for {}", id);
                                                        let mut should_decrement = false;
                                                        if action == "PULL" {
                                                            should_decrement = true;
                                                            match data_type_clone {
                                                                SyncDataType::Agent => {
                                                                    println!("[SyncService] Calling pull_agent for {}", id);
                                                                    match PullExecutor::pull_agent(&h_in, &c_in, &b_in, &token, &id, &wq_in).await {
                                                                        Ok(_) => println!("[SyncService] pull_agent success: {}", id),
                                                                        Err(e) => println!("[SyncService] pull_agent error: {} - {}", id, e),
                                                                    }
                                                                },
                                                                SyncDataType::Group => {
                                                                    println!("[SyncService] Calling pull_group for {}", id);
                                                                    match PullExecutor::pull_group(&h_in, &c_in, &b_in, &token, &id, &wq_in).await {
                                                                        Ok(_) => println!("[SyncService] pull_group success: {}", id),
                                                                        Err(e) => println!("[SyncService] pull_group error: {} - {}", id, e),
                                                                    }
                                                                },
                                                                SyncDataType::Topic => {
                                                                    match PullExecutor::pull_agent_topic(&h_in, &c_in, &b_in, &token, &id, &wq_in).await {
                                                                        Ok(_) => println!("[SyncService] pull_agent_topic success: {}", id),
                                                                        Err(e) => println!("[SyncService] pull_agent_topic error: {} - {}", id, e),
                                                                    }
                                                                },
                                                                SyncDataType::Avatar => {
                                                                    let parts: Vec<&str> = id.split(':').collect();
                                                                    if parts.len() == 2 {
                                                                        match PullExecutor::pull_avatar(&h_in, &c_in, &b_in, &token, parts[0], parts[1], &wq_in).await {
                                                                            Ok(_) => println!("[SyncService] pull_avatar success: {}", id),
                                                                            Err(e) => println!("[SyncService] pull_avatar error: {} - {}", id, e),
                                                                        }
                                                                    }
                                                                },
                                                                _ => {}
                                                            }
                                                        } else if action == "PUSH" {
                                                            match data_type_clone {
                                                                SyncDataType::Agent => { let _ = PushExecutor::push_agent(&h_in, &c_in, &b_in, &token, &id).await; },
                                                                SyncDataType::Group => { let _ = PushExecutor::push_group(&h_in, &c_in, &b_in, &token, &id).await; },
                                                                SyncDataType::Topic => { let _ = PushExecutor::push_agent_topic(&h_in, &c_in, &b_in, &token, &id).await; },
                                                                SyncDataType::Avatar => {
                                                                    let parts: Vec<&str> = id.split(':').collect();
                                                                    if parts.len() == 2 { let _ = PushExecutor::push_avatar(&h_in, &c_in, &b_in, &token, parts[0], parts[1]).await; }
                                                                },
                                                                _ => {}
                                                            }
                                                        } else if action == "DELETE" {
                                                            use crate::vcp_modules::sync_executor::delete_executor::DeleteExecutor;
                                                            match data_type_clone {
                                                                SyncDataType::Agent => { let _ = DeleteExecutor::soft_delete_agent(&h_in, &id).await; },
                                                                SyncDataType::Group => { let _ = DeleteExecutor::soft_delete_group(&h_in, &id).await; },
                                                                SyncDataType::Topic => { let _ = DeleteExecutor::soft_delete_topic(&h_in, &id).await; },
                                                                SyncDataType::Avatar => {
                                                                    let parts: Vec<&str> = id.split(':').collect();
                                                                    if parts.len() == 2 {
                                                                        let _ = DeleteExecutor::soft_delete_avatar(&h_in, parts[0], parts[1]).await;
                                                                    }
                                                                },
                                                                _ => {}
                                                            }
                                                        } else if action == "PUSH_DELETE" {
                                                            let _ = tx_internal_in.send(SyncCommand::NotifyDelete { 
                                                                data_type: data_type_clone, 
                                                                id: id.clone() 
                                                            });
                                                        }
                                                        
                                                        if should_decrement {
                                                            let remaining = pending.fetch_sub(1, Ordering::SeqCst);
                                                            println!("[SyncService] Task completed, remaining: {}", remaining - 1);
                                                            if remaining == 1 {
                                                                println!("[SyncService] All Phase 1 tasks completed, triggering Phase1Completed");
                                                                let _ = tx_internal_in.send(SyncCommand::Phase1Completed);
                                                            }
                                                        }
                                                    });
                                                }
                                            }
                                        },
                                        Some("MESSAGE_MANIFEST_RESULTS") => {
                                            let topic_id = payload["topicId"].as_str().unwrap_or_default().to_string();
                                            if let Some(remote_msgs) = payload["messages"].as_array() {
                                                let settings = crate::vcp_modules::settings_manager::read_settings(h.clone(), h.state()).await.unwrap_or_default();
                                                let db = h.state::<DbState>();
                                                
                                                let (to_pull_ids, to_push) = Phase3Message::compute_message_diff(&db.pool, &topic_id, remote_msgs).await.unwrap_or((Vec::new(), false));
                                                
                                                if to_push {
                                                    let h_in = h.clone();
                                                    let c_in = c.clone();
                                                    let b_in = base.clone();
                                                    let token = settings.sync_token.clone();
                                                    let tid = topic_id.clone();
                                                    let sync_state = h.state::<SyncState>();
                                                    let uploaded_hashes = sync_state.uploaded_hashes.clone();
                                                    tauri::async_runtime::spawn(async move {
                                                        let _ = PushExecutor::push_messages(&h_in, &c_in, &b_in, &token, &tid, Some(uploaded_hashes)).await;
                                                    });
                                                }
                                                
                                                if !to_pull_ids.is_empty() {
                                                    let h_in = h.clone();
                                                    let c_in = c.clone();
                                                    let b_in = base.clone();
                                                    let token = settings.sync_token.clone();
                                                    let tid = topic_id.clone();
                                                    let wq_msg = wq.clone();
                                                    tauri::async_runtime::spawn(async move {
                                                        let _ = PullExecutor::pull_messages(&h_in, &c_in, &b_in, &token, &tid, &to_pull_ids, &wq_msg).await;
                                                        
                                                        let db = h_in.state::<DbState>();
                                                        if let Ok(mut tx) = db.pool.begin().await {
                                                            let _ = HashAggregator::bubble_from_topic(&mut tx, &tid).await;
                                                            let _ = tx.commit().await;
                                                        }
                                                    });
                                                }
                                            }
                                        },
                                        Some("PHASE_MANIFESTS") => {
                                            println!("[SyncService] Received PHASE_MANIFESTS");
                                            if let Some(manifests) = payload["manifests"].as_array() {
                                                for manifest in manifests {
                                                    let data_type_str = manifest["dataType"].as_str().unwrap_or_default();
                                                    let items = manifest["items"].as_array().cloned().unwrap_or_default();
                                                    
                                                    println!("[SyncService] PHASE_MANIFESTS dataType={} items={}", data_type_str, items.len());
                                                    
                                                    if data_type_str == "topic" {
                                                        let settings = crate::vcp_modules::settings_manager::read_settings(h.clone(), h.state()).await.unwrap_or_default();
                                                        let db = h.state::<DbState>();
                                                        
                                                        let local_topics: Vec<(String, String, Option<String>, i64)> = sqlx::query_as(
                                                            "SELECT topic_id, owner_type, config_hash, updated_at FROM topics WHERE deleted_at IS NULL"
                                                        )
                                                        .fetch_all(&db.pool)
                                                        .await
                                                        .unwrap_or_default();
                                                        
                                                        let local_map: std::collections::HashMap<String, (String, Option<String>, i64)> = local_topics
                                                            .into_iter()
                                                            .map(|(id, owner_type, hash, ts)| (id, (owner_type, hash, ts)))
                                                            .collect();
                                                        
                                                        let mut pull_agent_topics = Vec::new();
                                                        let mut pull_group_topics = Vec::new();
                                                        let mut push_agent_topics = Vec::new();
                                                        let mut push_group_topics = Vec::new();
                                                        
                                                        for remote in &items {
                                                            let id = remote["id"].as_str().unwrap_or_default().to_string();
                                                            let remote_owner_type = remote["ownerType"].as_str().unwrap_or("agent");
                                                            let remote_hash = remote["hash"].as_str().map(|s| s.to_string());
                                                            let remote_ts = remote["ts"].as_i64().unwrap_or(0);
                                                            
                                                            if let Some((local_owner_type, local_hash, local_ts)) = local_map.get(&id) {
                                                                if local_hash != &remote_hash {
                                                                    if remote_ts > *local_ts {
                                                                        if remote_owner_type == "group" {
                                                                            pull_group_topics.push(id);
                                                                        } else {
                                                                            pull_agent_topics.push(id);
                                                                        }
                                                                    } else {
                                                                        if local_owner_type == "group" {
                                                                            push_group_topics.push(id);
                                                                        } else {
                                                                            push_agent_topics.push(id);
                                                                        }
                                                                    }
                                                                }
                                                            } else {
                                                                if remote_owner_type == "group" {
                                                                    pull_group_topics.push(id);
                                                                } else {
                                                                    pull_agent_topics.push(id);
                                                                }
                                                            }
                                                        }
                                                        
                                                        for (id, (owner_type, _, _)) in local_map.iter() {
                                                            if !items.iter().any(|r| r["id"].as_str() == Some(id.as_str())) {
                                                                if owner_type == "group" {
                                                                    push_group_topics.push(id.clone());
                                                                } else {
                                                                    push_agent_topics.push(id.clone());
                                                                }
                                                            }
                                                        }
                                                        
                                                        println!("[SyncService] Topic diff: pull_agent={} pull_group={} push_agent={} push_group={}", 
                                                            pull_agent_topics.len(), pull_group_topics.len(), push_agent_topics.len(), push_group_topics.len());
                                                        
                                                        let total_pull = (pull_agent_topics.len() + pull_group_topics.len()) as u32;
                                                        let is_empty = total_pull == 0;
                                                        pending_tasks_task.fetch_add(total_pull, Ordering::SeqCst);
                                                        
                                                        for topic_id in pull_agent_topics {
                                                            let h_in = h.clone();
                                                            let c_in = c.clone();
                                                            let b_in = base.clone();
                                                            let token = settings.sync_token.clone();
                                                            let wq_in = wq.clone();
                                                            let pending = pending_tasks_task.clone();
                                                            let tx_internal_in = tx_internal.clone();
                                                            
                                                            tauri::async_runtime::spawn(async move {
                                                                let _ = PullExecutor::pull_agent_topic(&h_in, &c_in, &b_in, &token, &topic_id, &wq_in).await;
                                                                
                                                                let remaining = pending.fetch_sub(1, Ordering::SeqCst);
                                                                if remaining == 1 {
                                                                    let _ = tx_internal_in.send(SyncCommand::Phase2Completed);
                                                                }
                                                            });
                                                        }
                                                        
                                                        for topic_id in pull_group_topics {
                                                            let h_in = h.clone();
                                                            let c_in = c.clone();
                                                            let b_in = base.clone();
                                                            let token = settings.sync_token.clone();
                                                            let wq_in = wq.clone();
                                                            let pending = pending_tasks_task.clone();
                                                            let tx_internal_in = tx_internal.clone();
                                                            
                                                            tauri::async_runtime::spawn(async move {
                                                                let _ = PullExecutor::pull_group_topic(&h_in, &c_in, &b_in, &token, &topic_id, &wq_in).await;
                                                                
                                                                let remaining = pending.fetch_sub(1, Ordering::SeqCst);
                                                                if remaining == 1 {
                                                                    let _ = tx_internal_in.send(SyncCommand::Phase2Completed);
                                                                }
                                                            });
                                                        }
                                                        
                                                        for topic_id in push_agent_topics {
                                                            let h_in = h.clone();
                                                            let c_in = c.clone();
                                                            let b_in = base.clone();
                                                            let token = settings.sync_token.clone();
                                                            
                                                            tauri::async_runtime::spawn(async move {
                                                                let _ = PushExecutor::push_agent_topic(&h_in, &c_in, &b_in, &token, &topic_id).await;
                                                            });
                                                        }
                                                        
                                                        for topic_id in push_group_topics {
                                                            let h_in = h.clone();
                                                            let c_in = c.clone();
                                                            let b_in = base.clone();
                                                            let token = settings.sync_token.clone();
                                                            
                                                            tauri::async_runtime::spawn(async move {
                                                                let _ = PushExecutor::push_group_topic(&h_in, &c_in, &b_in, &token, &topic_id).await;
                                                            });
                                                        }
                                                        
                                                        if is_empty {
                                                            let _ = tx_internal.send(SyncCommand::Phase2Completed);
                                                        }
                                                    }
                                                }
                                            }
                                        },
                                        Some("PHASE_COMPLETED") => {
                                            println!("[SyncService] Desktop phase completed");
                                        },
                                        _ => {}
                                    }
                                }
                            },
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

    {
        let h = app_handle.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(86400)).await;
                use crate::vcp_modules::sync_executor::delete_executor::DeleteExecutor;
                let _ = DeleteExecutor::cleanup_old_deleted_records(&h, 30).await;
            }
        });
    }

    SyncState {
        ws_sender: tx,
        connection_status,
        op_tracker,
        metrics,
        network_semaphore,
        pipeline,
        uploaded_hashes: Arc::new(RwLock::new(HashSet::new())),
        write_queue,
        pending_tasks,
    }
}

#[tauri::command]
pub async fn get_sync_status(state: State<'_, SyncState>) -> Result<String, String> {
    Ok(state.connection_status.read().await.clone())
}

#[tauri::command]
pub async fn get_pipeline_status(state: State<'_, SyncState>) -> Result<String, String> {
    let phase = state.pipeline.state().read().await.clone();
    Ok(serde_json::to_string(&phase).unwrap_or_default())
}

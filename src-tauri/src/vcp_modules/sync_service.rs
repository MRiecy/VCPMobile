use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::db_write_queue::DbWriteQueue;
use crate::vcp_modules::sync_executor::{PullExecutor, PushExecutor};
use crate::vcp_modules::sync_hash::{HashAggregator, HashInitializer};
use crate::vcp_modules::sync_logger::{LogLevel, SyncLogger};
use crate::vcp_modules::sync_manifest::ManifestBuilder;
use crate::vcp_modules::sync_pipeline::{Phase1Metadata, Phase3Message, SyncPipeline};
use crate::vcp_modules::sync_types::{SyncDataType, SyncManifest};
use crate::vcp_modules::sync_utils::SyncMetrics;
use dashmap::DashMap;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, Runtime, State};
use tokio::sync::{mpsc, RwLock, Semaphore};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};

pub struct SyncState {
    pub ws_sender: mpsc::UnboundedSender<SyncCommand>,
    pub connection_status: Arc<RwLock<String>>,
    #[allow(dead_code)]
    pub op_tracker: Arc<SyncOperationTracker>,
    #[allow(dead_code)]
    pub metrics: Arc<SyncMetrics>,
    #[allow(dead_code)]
    pub network_semaphore: Arc<NetworkAwareSemaphore>,
    #[allow(dead_code)]
    pub pipeline: Arc<SyncPipeline>,
    pub uploaded_hashes: Arc<RwLock<HashSet<String>>>,
    #[allow(dead_code)]
    pub write_queue: Arc<DbWriteQueue>,
    #[allow(dead_code)]
    pub pending_tasks: Arc<AtomicU32>,
    #[allow(dead_code)]
    pub pending_message_topics: Arc<AtomicU32>,
    #[allow(dead_code)]
    pub sync_logger: Arc<Mutex<SyncLogger>>,
}

#[allow(dead_code)]
pub struct SyncOperationTracker {
    in_progress: DashMap<String, Instant>,
    ttl: Duration,
}

#[allow(dead_code)]
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

#[allow(dead_code)]
pub struct NetworkAwareSemaphore {
    semaphore: Arc<Semaphore>,
    network_type: Arc<RwLock<NetworkType>>,
    success_streak: AtomicU32,
    failure_streak: AtomicU32,
}

#[derive(Clone, Copy, PartialEq)]
#[allow(dead_code)]
pub enum NetworkType {
    WiFi,
    Cell5G,
    Cell4G,
    Unknown,
}

#[allow(dead_code)]
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
    NotifyLocalChange {
        id: String,
        data_type: SyncDataType,
        hash: String,
        ts: i64,
    },
    #[allow(dead_code)]
    RequestMessageManifest {
        topic_id: String,
    },
    Phase1,
    Phase2,
    Phase3,
    NotifyDelete {
        data_type: SyncDataType,
        id: String,
    },
}

fn parse_sync_data_type(value: &Value) -> Option<SyncDataType> {
    serde_json::from_value::<SyncDataType>(value.clone()).ok()
}

async fn publish_sync_status<R: Runtime>(
    app_handle: &AppHandle<R>,
    status: &Arc<RwLock<String>>,
    next_status: &str,
    message: &str,
) {
    {
        let mut guard = status.write().await;
        if guard.as_str() == next_status {
            return;
        }
        *guard = next_status.to_string();
    }

    // 统一使用 vcp-system-event 发射，type 为明确的 vcp-sync-status
    let _ = app_handle.emit(
        "vcp-system-event",
        json!({
            "type": "vcp-sync-status",
            "status": next_status,
            "message": message,
            "source": "Sync"
        }),
    );
}

pub fn init_sync_service(app_handle: AppHandle) -> SyncState {
    let (tx, mut rx) = mpsc::unbounded_channel::<SyncCommand>();
    let (pipeline_tx, mut pipeline_rx) =
        mpsc::unbounded_channel::<crate::vcp_modules::sync_pipeline::pipeline::PipelineCommand>();

    let handle_clone = app_handle.clone();
    let tx_internal = tx.clone();
    let connection_status = Arc::new(RwLock::new(String::from("connecting")));
    let connection_status_for_task = connection_status.clone();
    let op_tracker = Arc::new(SyncOperationTracker::new());
    let metrics = Arc::new(SyncMetrics::new());
    let network_semaphore = Arc::new(NetworkAwareSemaphore::new());
    let pipeline = Arc::new(SyncPipeline::new(pipeline_tx));
    let pending_tasks = Arc::new(AtomicU32::new(0));
    let pending_message_topics = Arc::new(AtomicU32::new(0));

    let db = app_handle.state::<DbState>();
    let mut write_queue = DbWriteQueue::new(db.pool.clone());

    // Initialize sync logger with default log level
    let sync_log_level = LogLevel::Info;
    let sync_logger = Arc::new(Mutex::new(SyncLogger::new_session(sync_log_level)));
    write_queue.set_logger(sync_logger.clone());
    let write_queue = Arc::new(write_queue);

    let _metrics_task = metrics.clone();
    let semaphore_task = network_semaphore.clone();
    let pipeline_task = pipeline.clone();
    let write_queue_task = write_queue.clone();
    let pending_tasks_task = pending_tasks.clone();
    let pending_msg_topics_task = pending_message_topics.clone();
    let sync_logger_task = sync_logger.clone();

    tauri::async_runtime::spawn(async move {
        let http_client = reqwest::Client::new();

        loop {
            let (ws_url, http_url) = {
                let settings_state =
                    handle_clone.state::<crate::vcp_modules::settings_manager::SettingsState>();
                match crate::vcp_modules::settings_manager::read_settings(
                    handle_clone.clone(),
                    settings_state,
                )
                .await
                {
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

            publish_sync_status(
                &handle_clone,
                &connection_status_for_task,
                "connecting",
                "同步服务连接中...",
            )
            .await;

            match connect_async(&ws_url).await {
                Ok((mut ws_stream, _)) => {
                    if let Ok(mut logger) = sync_logger_task.lock() {
                        logger.start_phase("metadata", 0);
                        logger.log(LogLevel::Info, "metadata", "=== Phase 1: Metadata ===");
                    }
                    publish_sync_status(
                        &handle_clone,
                        &connection_status_for_task,
                        "open",
                        "同步服务已连接",
                    )
                    .await;

                    // 增加同步成功卡片
                    let _ = handle_clone.emit(
                        "vcp-system-event",
                        json!({
                            "type": "vcp-log-message",
                            "data": {
                                "id": "vcp_sync_connection_status",
                                "status": "success",
                                "tool_name": "Sync",
                                "content": "✅ 同步服务已建立长连接。准备执行元数据校对。",
                                "source": "Sync"
                            }
                        }),
                    );
                    let db = handle_clone.state::<DbState>();
                    if let Err(e) = HashInitializer::ensure_all_agent_hashes(&db.pool).await {
                        if let Ok(logger) = sync_logger_task.lock() {
                            logger.log(
                                LogLevel::Error,
                                "metadata",
                                &format!("Failed to initialize agent hashes: {}", e),
                            );
                        }

                        // 发射逻辑错误通知卡片
                        let _ = handle_clone.emit(
        "vcp-system-event",
        json!({
            "type": "vcp-log-message",
            "data": {
                "id": "vcp_sync_connection_status",
                "status": "error",
                "tool_name": "同步初始化失败",
                "content": format!("❌ 数据库就绪失败: {}\n\n提示：尝试重新启动应用，如果问题持续，请检查存储权限。", e),
                "source": "Sync"
            }
        }),
    );
                        break; // 退出本次循环
                    }
                    if let Err(e) = HashInitializer::ensure_all_group_hashes(&db.pool).await {
                        if let Ok(logger) = sync_logger_task.lock() {
                            logger.log(
                                LogLevel::Error,
                                "metadata",
                                &format!("Hash init error: {}", e),
                            );
                        }

                        // 发射逻辑错误通知卡片
                        let _ = handle_clone.emit(
                            "vcp-system-event",
                            json!({
                                "type": "vcp-log-message",
                                "data": {
                                    "id": "vcp_sync_connection_status",
                                    "status": "error",
                                    "tool_name": "同步初始化失败",
                                    "content": format!("❌ 群组数据库就绪失败: {}\n\n提示：检查存储权限或数据库完整性。", e),
                                    "source": "Sync"
                                }
                            }),
                        );
                        break;
                    }
                    if let Ok(logger) = sync_logger_task.lock() {
                        logger.log(
                            LogLevel::Info,
                            "metadata",
                            "Phase 1 manifests sent, waiting for diff results...",
                        );
                    }

                    if let Ok(manifests) = Phase1Metadata::build_all_manifests(&db.pool).await {
                        for manifest in manifests {
                            let msg = json!({ "type": "SYNC_MANIFEST", "data": manifest.items, "dataType": manifest.data_type, "phase": "metadata" });
                            let _ = ws_stream.send(Message::Text(msg.to_string().into())).await;
                        }
                    }

                    loop {
                        tokio::select! {
                                                                    Some(cmd) = pipeline_rx.recv() => {
                                                                        match cmd {
                                                                            crate::vcp_modules::sync_pipeline::pipeline::PipelineCommand::Phase1 => {
                                                                                if let Ok(mut logger) = sync_logger_task.lock() {
                                                                                    logger.start_phase("topic", 0);
                                                                                    logger.log(LogLevel::Info, "topic", "=== Phase 2: Topics ===");
                                                                                }
                                                                                let _ = ws_stream.send(Message::Text(json!({ "type": "PHASE_START", "phase": "topic" }).to_string().into())).await;
                                                                            },
                                                                            crate::vcp_modules::sync_pipeline::pipeline::PipelineCommand::Phase2 => {
                                                                                if let Ok(mut logger) = sync_logger_task.lock() {
                                                                                    logger.start_phase("message", 0);
                                                                                    logger.log(LogLevel::Info, "message", "=== Phase 3: Messages ===");
                                                                                }
                                                                                let _ = ws_stream.send(Message::Text(json!({ "type": "PHASE_START", "phase": "message" }).to_string().into())).await;

                                                                                let db = handle_clone.state::<DbState>();
                                                                                if let Ok(topic_ids) = Phase3Message::get_all_active_topic_ids(&db.pool).await {
                                                                                    let topic_count = topic_ids.len() as u32;
                                                                                    pending_msg_topics_task.store(topic_count, Ordering::SeqCst);
                                                                                    if topic_count > 0 {
                                                                                        if let Ok(logger) = sync_logger_task.lock() {
                                                                                            logger.log(LogLevel::Info, "message",
                                                                                                &format!("Phase 3: requesting manifests for {} topics", topic_count));
                                                                                        }
                                                                                    }
                                                                                    for tid in topic_ids {
                                                                                        let msg = json!({ "type": "GET_MESSAGE_MANIFEST", "topicId": tid });
                                                                                        let _ = ws_stream.send(Message::Text(msg.to_string().into())).await;
                                                                                    }
                                                                                    if topic_count == 0 {
                                                                                        if let Ok(logger) = sync_logger_task.lock() {
                                                                                            logger.log(LogLevel::Info, "message", "Phase 3 completed (no topics)");
                                                                                            logger.complete_phase("message");
                                                                                        }
                                                                                        let _ = tx_internal.send(SyncCommand::Phase3);
                                                                                    }
                                                                                }
                                                                            },
                                                                            crate::vcp_modules::sync_pipeline::pipeline::PipelineCommand::Phase3 => {
                                                                                 if let Ok(logger) = sync_logger_task.lock() {
                                                                                     logger.log(LogLevel::Info, "sync", "=== Sync Complete ===");
                                                                                     logger.complete_phase("sync");
                                                                                     (*logger).end_session();
                                                                                 }
                                                                                let _ = ws_stream.send(Message::Text(json!({ "type": "PHASE_COMPLETED" }).to_string().into())).await;
                                                                            },
                                                                        }
                                                                    },
                                                                    Some(cmd) = rx.recv() => {
                                                                        match cmd {
                                                                            SyncCommand::NotifyLocalChange { id, data_type, hash, ts } => {
                                                                                let msg = json!({ "type": "SYNC_ENTITY_UPDATE", "id": id, "dataType": data_type, "hash": hash, "ts": ts });
                                                                                let _ = ws_stream.send(Message::Text(msg.to_string().into())).await;
                                                                            },
                                                                            SyncCommand::RequestMessageManifest { topic_id } => {
                                                                                let msg = json!({ "type": "GET_MESSAGE_MANIFEST", "topicId": topic_id });
                                                                                let _ = ws_stream.send(Message::Text(msg.to_string().into())).await;
                                                                            },
                                                                            SyncCommand::Phase1 => {
                                                                                let db = handle_clone.state::<DbState>();
                                                                                if let Err(e) = pipeline_task.on_phase1_completed(&db.pool).await {
                                                                                    let _ = handle_clone.emit(
                                                                                        "vcp-system-event",
                                                                                        json!({
                                                                                            "type": "vcp-log-message",
                                                                                            "data": {
                                                                                                "id": "vcp_sync_connection_status",
                                                                                                "status": "error",
                                                                                                "tool_name": "同步阶段 1 失败",
                                                                                                "content": format!("❌ 无法进入 Topic 同步阶段: {}", e),
                                                                                                "source": "Sync"
                                                                                            }
                                                                                        }),
                                                                                    );
                                                                                }
                                                                            },
                                                                            SyncCommand::Phase2 => {
                                                                                if let Err(e) = pipeline_task.on_phase2_completed().await {
                                                                                    let _ = handle_clone.emit(
                                                                                        "vcp-system-event",
                                                                                        json!({
                                                                                            "type": "vcp-log-message",
                                                                                            "data": {
                                                                                                "id": "vcp_sync_connection_status",
                                                                                                "status": "error",
                                                                                                "tool_name": "同步阶段 2 失败",
                                                                                                "content": format!("❌ 无法进入 Message 同步阶段: {}", e),
                                                                                                "source": "Sync"
                                                                                            }
                                                                                        }),
                                                                                    );
                                                                                }
                                                                            },
                                                                            SyncCommand::Phase3 => {
                                                                                if let Err(e) = pipeline_task.on_phase3_completed().await {
                                                                                    let _ = handle_clone.emit(
                                                                                        "vcp-system-event",
                                                                                        json!({
                                                                                            "type": "vcp-log-message",
                                                                                            "data": {
                                                                                                "id": "vcp_sync_connection_status",
                                                                                                "status": "error",
                                                                                                "tool_name": "同步阶段 3 失败",
                                                                                                "content": format!("❌ 无法结束同步任务: {}", e),
                                                                                                "source": "Sync"
                                                                                            }
                                                                                        }),
                                                                                    );
                                                                                }
                                                                            },                                                                            SyncCommand::NotifyDelete { data_type, id } => {
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
                                                            if let Some(items) = payload["data"].as_array() {
                                                                let Some(data_type) = parse_sync_data_type(&payload["dataType"]) else { continue; };
                                                                let settings = crate::vcp_modules::settings_manager::read_settings(h.clone(), h.state()).await.unwrap_or_default();
                                                                let items_clone: Vec<serde_json::Value> = items.clone();
                                                                let pull_count = items_clone.iter().filter(|i| i["action"] == "PULL").count() as u32;
                                                                let push_count = items_clone.iter().filter(|i| i["action"] == "PUSH").count() as u32;
                                                                let delete_count = items_clone.iter().filter(|i| i["action"] == "DELETE").count() as u32;
                                                                let push_delete_count = items_clone.iter().filter(|i| i["action"] == "PUSH_DELETE").count() as u32;
                                                                let total_ops = pull_count + push_count + delete_count + push_delete_count;
                            if total_ops > 0 {
                                if let Ok(mut logger) = sync_logger_task.lock() {
                                    logger.log_operation("metadata", &data_type.to_string(), "manifest", true,
                                        Some(&format!("pull={} push={} delete={} push_delete={}", pull_count, push_count, delete_count, push_delete_count)));
                                }
                            }
                                                                pending_tasks_task.fetch_add(total_ops, Ordering::SeqCst);

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
                            let sync_logger_in = sync_logger_task.clone();

                            tauri::async_runtime::spawn(async move {
                                                                        let _permit = s_in.acquire().await;
                                                                        let should_decrement = true;
                                                                        if action == "PULL" {
                                                                            match data_type_clone {
                                                                                SyncDataType::Agent => {
                                                                                    match PullExecutor::pull_agent(&h_in, &c_in, &b_in, &token, &id, &wq_in).await {
                                            Err(e) => {
                                                if let Ok(mut logger) = sync_logger_in.lock() {
                                                    logger.log_operation("metadata", "agent", &id, false,
                                                        Some(&format!("pull_agent error: {}", e)));
                                                }
                                            },
                                            _ => {
                                                if let Ok(mut logger) = sync_logger_in.lock() {
                                                    logger.log_operation("metadata", "agent", &id, true, Some("pulled from server"));
                                                }
                                            }
                                                                                    }
                                                                                },
                                                                                SyncDataType::Group => {
                                                                                    match PullExecutor::pull_group(&h_in, &c_in, &b_in, &token, &id, &wq_in).await {
                                            Err(e) => {
                                                if let Ok(mut logger) = sync_logger_in.lock() {
                                                    logger.log_operation("metadata", "group", &id, false,
                                                        Some(&format!("pull_group error: {}", e)));
                                                }
                                            },
                                            _ => {
                                                if let Ok(mut logger) = sync_logger_in.lock() {
                                                    logger.log_operation("metadata", "group", &id, true, Some("pulled from server"));
                                                }
                                            }
                                                                                    }
                                                                                },
                                                                                SyncDataType::Avatar => {
                                                                                    let parts: Vec<&str> = id.split(':').collect();
                                                                                    if parts.len() == 2 {
                                            match PullExecutor::pull_avatar(&h_in, &c_in, &b_in, &token, parts[0], parts[1], &wq_in).await {
                                                Err(e) => {
                                                    if let Ok(mut logger) = sync_logger_in.lock() {
                                                        logger.log_operation("metadata", "avatar", &id, false,
                                                            Some(&format!("pull_avatar error: {}", e)));
                                                    }
                                                },
                                                _ => {
                                                    if let Ok(mut logger) = sync_logger_in.lock() {
                                                        logger.log_operation("metadata", "avatar", &id, true, Some("pulled from server"));
                                                    }
                                                }
                                            }
                                                                                    }
                                                                                },
                                                                                _ => {}
                                                                            }
                                                                        } else if action == "PUSH" {
                                                                            match data_type_clone {
                                                                                SyncDataType::Agent => { let _ = PushExecutor::push_agent(&h_in, &c_in, &b_in, &token, &id).await; },
                                                                                SyncDataType::Group => { let _ = PushExecutor::push_group(&h_in, &c_in, &b_in, &token, &id).await; },
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
                                                                    if remaining == 1 {
                                                                        if let Ok(logger) = sync_logger_in.lock() {
                                                                            logger.log(LogLevel::Info, "metadata", "Phase 1 completed");
                                                                            logger.complete_phase("metadata");
                                                                        }
                                                                        let _ = tx_internal_in.send(SyncCommand::Phase1);
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

                                                                let has_pull = !to_pull_ids.is_empty();
                                                                let has_push = to_push;

                                                                if to_push {
                                                                    let h_in = h.clone();
                                                                    let c_in = c.clone();
                                                                    let b_in = base.clone();
                                                                    let token = settings.sync_token.clone();
                                                                    let tid = topic_id.clone();
                            let sync_state = h.state::<SyncState>();
                            let uploaded_hashes = sync_state.uploaded_hashes.clone();
                            let pending_msg = pending_msg_topics_task.clone();
                            let tx_internal_msg = tx_internal.clone();
                            let sync_logger_msg = sync_logger_task.clone();
                            tauri::async_runtime::spawn(async move {
                                let _ = PushExecutor::push_messages(&h_in, &c_in, &b_in, &token, &tid, Some(uploaded_hashes)).await;

                                let db = h_in.state::<DbState>();
                                if let Ok(mut tx) = db.pool.begin().await {
                                    let _ = HashAggregator::bubble_topic_hash(&mut tx, &tid).await;
                                    let _ = tx.commit().await;
                                }

                                let remaining = pending_msg.fetch_sub(1, Ordering::SeqCst);
                                                                if remaining == 1 {
                                                                    if let Ok(logger) = sync_logger_msg.lock() {
                                                                        logger.log(LogLevel::Info, "message", "Phase 3 completed");
                                                                        logger.complete_phase("message");
                                                                    }
                                                                    let _ = tx_internal_msg.send(SyncCommand::Phase3);
                                                                }
                            });
                                                                 }

                                                                 if !to_pull_ids.is_empty() {
                                                                    let h_in = h.clone();
                                                                    let c_in = c.clone();
                                                                    let b_in = base.clone();
                                                                    let token = settings.sync_token.clone();
                                                                    let tid = topic_id.clone();
                                                                    let wq_msg = wq.clone();
                                                                    let pending_msg = pending_msg_topics_task.clone();
                                                                    let tx_internal_msg = tx_internal.clone();
                                                                    tauri::async_runtime::spawn(async move {
                                                                        let _ = PullExecutor::pull_messages(&h_in, &c_in, &b_in, &token, &tid, &to_pull_ids, &wq_msg).await;

                                                                        let db = h_in.state::<DbState>();
                                                                        if let Ok(mut tx) = db.pool.begin().await {
                                                                            let _ = HashAggregator::bubble_from_topic(&mut tx, &tid).await;
                                                                            let _ = tx.commit().await;
                                                                        }

                                                                        let remaining = pending_msg.fetch_sub(1, Ordering::SeqCst);
                                                                        if remaining == 1 {
                                                                            println!("[SyncService] Phase 3 completed");
                                                                            let _ = tx_internal_msg.send(SyncCommand::Phase3);
                                                                        }
                                                                    });
                                                                }

                                                                if !has_pull && !has_push {
                                                                    let remaining = pending_msg_topics_task.fetch_sub(1, Ordering::SeqCst);
                                                                    if remaining == 1 {
                                                                        if let Ok(logger) = sync_logger_task.lock() {
                                                                            logger.log(LogLevel::Info, "message", "Phase 3 completed");
                                                                            logger.complete_phase("message");
                                                                        }
                                                                        let _ = tx_internal.send(SyncCommand::Phase3);
                                                                    }
                                                                }
                                                            }
                                                        },
                                            Some("PHASE_MANIFESTS") => {
                                                if let Some(manifests) = payload["manifests"].as_array() {
                                                    for manifest in manifests {
                                                        let data_type_str = manifest["dataType"].as_str().unwrap_or_default();
                                                        let remote_items = manifest["items"].as_array().cloned().unwrap_or_default();

                                                        if data_type_str == "topic" {
                                                            let settings = crate::vcp_modules::settings_manager::read_settings(h.clone(), h.state()).await.unwrap_or_default();
                                                            let db = h.state::<DbState>();

                                                            // 使用 build_topic_manifest 生成本地清单（包含正确的哈希计算）
                                                            let local_manifest = ManifestBuilder::build_topic_manifest(&db.pool).await.unwrap_or_else(|_| SyncManifest { data_type: SyncDataType::Topic, items: Vec::new() });
                                                            let local_items = local_manifest.items;

                                                            // 建立本地清单的 Map
                                                            let local_map: std::collections::HashMap<String, (String, Option<String>, i64)> = local_items
                                                                .into_iter()
                                                                .filter(|i| i.deleted_at.is_none())
                                                                .map(|i| (i.id.clone(), (i.owner_type.unwrap_or_default(), Some(i.hash), i.ts)))
                                                                .collect();

                                                            let mut pull_agent_topics = Vec::new();
                                                            let mut pull_group_topics = Vec::new();
                                                            let mut push_agent_topics = Vec::new();
                                                            let mut push_group_topics = Vec::new();

                                                            for remote in &remote_items {
                                                                let id = remote["id"].as_str().unwrap_or_default().to_string();
                                                                let remote_owner_type = remote["ownerType"].as_str().unwrap_or("agent");
                                                                let remote_hash = remote["hash"].as_str().map(|s| s.to_string());
                                                                let remote_ts = remote["ts"].as_i64().unwrap_or(0);

                                                                if let Some((local_owner_type, local_hash, local_ts)) = local_map.get(&id) {
                                                                    if let Some(ref lh) = local_hash {
                                                                        if let Some(ref rh) = remote_hash {
                                                                            if lh != rh {
                                                                                if remote_ts > *local_ts {
                                                                                    if remote_owner_type == "group" {
                                                                                        pull_group_topics.push(id);
                                                                                    } else {
                                                                                        pull_agent_topics.push(id);
                                                                                    }
                                                                                 } else if local_owner_type == "group" {
                                                                                     push_group_topics.push(id);
                                                                                 } else {
                                                                                     push_agent_topics.push(id);
                                                                                 }
                                                                            }
                                                                        }
                                                                    }
                                                                 } else if remote_owner_type == "group" {
                                                                     pull_group_topics.push(id);
                                                                 } else {
                                                                     pull_agent_topics.push(id);
                                                                 }
                                                            }

                                                            // 本地有但服务端没有的，需要 push
                                                            for (id, (owner_type, _, _)) in local_map.iter() {
                                                                if !remote_items.iter().any(|r| r["id"].as_str() == Some(id.as_str())) {
                                                                    if owner_type == "group" {
                                                                        push_group_topics.push(id.clone());
                                                                    } else {
                                                                        push_agent_topics.push(id.clone());
                                                                    }
                                                                }
                                                            }

                                                            let total_pull = (pull_agent_topics.len() + pull_group_topics.len()) as u32;
                                                            let total_push = (push_agent_topics.len() + push_group_topics.len()) as u32;
                                                            if total_pull > 0 || total_push > 0 {
                                                                println!("[SyncService] Topic diff: pull={} push={}", total_pull, total_push);
                                                            }
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
                        let sync_logger_topic = sync_logger_task.clone();

                        tauri::async_runtime::spawn(async move {
                            let _ = PullExecutor::pull_agent_topic(&h_in, &c_in, &b_in, &token, &topic_id, &wq_in).await;

                            let remaining = pending.fetch_sub(1, Ordering::SeqCst);
                                if remaining == 1 {
                                                                if let Ok(logger) = sync_logger_topic.lock() {
                                                                    logger.log(LogLevel::Info, "topic", "Phase 2 completed");
                                                                    logger.complete_phase("topic");
                                                                }
                                                                let _ = tx_internal_in.send(SyncCommand::Phase2);
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
                            let sync_logger_topic = sync_logger_task.clone();

                            tauri::async_runtime::spawn(async move {
                                let _ = PullExecutor::pull_group_topic(&h_in, &c_in, &b_in, &token, &topic_id, &wq_in).await;

                                let remaining = pending.fetch_sub(1, Ordering::SeqCst);
                            if remaining == 1 {
                                                                if let Ok(logger) = sync_logger_topic.lock() {
                                                                    logger.log(LogLevel::Info, "topic", "Phase 2 completed");
                                                                    logger.complete_phase("topic");
                                                                }
                                                                let _ = tx_internal_in.send(SyncCommand::Phase2);
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
                                                            if let Ok(logger) = sync_logger_task.lock() {
                                                                logger.log(LogLevel::Info, "topic", "Phase 2 completed (no topics to sync)");
                                                                logger.complete_phase("topic");
                                                            }
                                                            let _ = tx_internal.send(SyncCommand::Phase2);
                                                        }
                                                                                            }
                                                                                        }
                                                                                    }
                                                                                },
                                                                                Some("PHASE_COMPLETED") => {
                                                                                    println!("[SyncService] Sync completed");
                                                                                    // 发送同步完成卡片
                                                                                    let _ = handle_clone.emit(
                                                                                        "vcp-system-event",
                                                                                        json!({
                                                                                            "type": "vcp-log-message",
                                                                                            "data": {
                                                                                                "id": "vcp_sync_connection_status",
                                                                                                "status": "success",
                                                                                                "tool_name": "Sync",
                                                                                                "content": "✅ 同步任务已全部完成。本地数据库与桌面端已对齐。",
                                                                                                "source": "Sync"
                                                                                            }
                                                                                        }),
                                                                                    );
                                                                                },                                                                                _ => {}
                                                                            }
                                                                        }
                                                                    },
                                                                    else => break,
                                                                }
                    }
                }
                Err(e) => {
                    publish_sync_status(
                        &handle_clone,
                        &connection_status_for_task,
                        "error",
                        "同步服务连接失败",
                    )
                    .await;

                    // 发射错误通知卡片
                    let _ = handle_clone.emit(
                        "vcp-system-event",
                        json!({
                            "type": "vcp-log-message",
                            "data": {
                                "id": "vcp_sync_connection_status",
                                "status": "error",
                                "tool_name": "同步服务失败",
                                "content": format!("❌ 无法连接到同步服务: {}\n\n提示：\n1. 请检查桌面端 VCPMobileSync 插件是否已启动并启用分布式服务。\n2. 确保手机与桌面端处于同一局域网内。", e),
                                "source": "Sync"
                            }
                        }),
                    );
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
        pending_message_topics: Arc::new(AtomicU32::new(0)),
        sync_logger,
    }
}

#[tauri::command]
pub async fn get_sync_status(state: State<'_, SyncState>) -> Result<String, String> {
    Ok(state.connection_status.read().await.clone())
}

#[tauri::command]
#[allow(dead_code)]
pub async fn get_pipeline_status(state: State<'_, SyncState>) -> Result<String, String> {
    let phase = state.pipeline.state().read().await.clone();
    Ok(serde_json::to_string(&phase).unwrap_or_default())
}

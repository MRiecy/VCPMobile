use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::db_write_queue::DbWriteQueue;
use crate::vcp_modules::sync_executor::{PullExecutor, PushExecutor};
use crate::vcp_modules::sync_hash::{HashAggregator, HashInitializer};
use crate::vcp_modules::sync_logger::{LogLevel, SyncLogger};
use crate::vcp_modules::sync_manifest::ManifestBuilder;
use crate::vcp_modules::sync_pipeline::{Phase1Metadata, Phase3Message, SyncPipeline};
use crate::vcp_modules::sync_types::{SyncDataType, SyncManifest};
use dashmap::DashMap;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use sqlx::Row;
use std::collections::HashSet;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, Runtime, State};
use tokio::sync::{mpsc, RwLock, Semaphore};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};

pub struct SyncState {
    pub ws_sender: mpsc::UnboundedSender<SyncCommand>,
    pub connection_status: Arc<RwLock<String>>,
    pub uploaded_hashes: Arc<RwLock<HashSet<String>>>,
    pub avatar_color_cache: Arc<DashMap<String, String>>,
}

/// 追踪 Phase 3 中已处理完成的 topic，替代 AtomicU32 避免双重递减下溢
struct Phase3Tracker {
    completed: tokio::sync::Mutex<HashSet<String>>,
    total: std::sync::atomic::AtomicUsize,
}

impl Phase3Tracker {
    /// 标记某个 topic 已完成。如果是首次标记，返回 true；否则返回 false。
    /// 当所有 topic 都完成时，触发 complete_phase 和 Phase3 命令。
    async fn mark_completed(
        &self,
        topic_id: &str,
        logger: &Arc<Mutex<SyncLogger>>,
        tx: &mpsc::UnboundedSender<SyncCommand>,
        app_handle: &AppHandle,
    ) -> bool {
        let mut completed = self.completed.lock().await;
        let is_new = completed.insert(topic_id.to_string());
        if is_new {
            let done = completed.len();
            let total = self.total.load(Ordering::SeqCst);
            println!(
                "[Sync] [message] Topic {} completed ({}/{})",
                topic_id, done, total
            );

            // 发送实时进度事件
            let _ = app_handle.emit(
                "vcp-sync-progress",
                json!({
                    "phase": "message",
                    "total": total,
                    "completed": done,
                    "message": format!("Syncing Messages: {}/{}", done, total)
                }),
            );

            if done == total {
                if let Ok(logger) = logger.lock() {
                    logger.complete_phase("message");
                }
                let _ = tx.send(SyncCommand::Phase3);
            }
            true
        } else {
            false
        }
    }
}

pub struct NetworkAwareSemaphore {
    semaphore: Arc<Semaphore>,
}

impl NetworkAwareSemaphore {
    pub fn new() -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(30)),
        }
    }

    pub async fn acquire(&self) -> tokio::sync::SemaphorePermit<'_> {
        self.semaphore.acquire().await.unwrap()
    }
}

pub enum SyncCommand {
    NotifyLocalChange {
        id: String,
        data_type: SyncDataType,
        hash: String,
        ts: i64,
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
    let network_semaphore = Arc::new(NetworkAwareSemaphore::new());
    let pipeline = Arc::new(SyncPipeline::new(pipeline_tx));
    let pending_tasks = Arc::new(AtomicU32::new(0));
    let total_tasks = Arc::new(AtomicU32::new(0));
    let pending_message_topics = Arc::new(Phase3Tracker {
        completed: tokio::sync::Mutex::new(HashSet::new()),
        total: std::sync::atomic::AtomicUsize::new(0),
    });

    let db = app_handle.state::<DbState>();
    let mut write_queue = DbWriteQueue::new(db.pool.clone());

    // Initialize sync logger with default log level
    let sync_log_level = LogLevel::Info;
    let sync_logger = Arc::new(Mutex::new(SyncLogger::new_session(sync_log_level)));
    write_queue.set_logger(sync_logger.clone());
    let write_queue = Arc::new(write_queue);

    let semaphore_task = network_semaphore.clone();
    let pipeline_task = pipeline.clone();
    let write_queue_task = write_queue.clone();
    let pending_tasks_task = pending_tasks.clone();
    let total_tasks_task = total_tasks.clone();
    let pending_msg_topics_task = pending_message_topics.clone();
    let _pending_msg_topics_for_manifests = pending_message_topics.clone();
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
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        continue;
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
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        continue;
                    }

                    if let Ok(manifests) = Phase1Metadata::build_all_manifests(&db.pool).await {
                        for manifest in manifests {
                            let msg = json!({ "type": "SYNC_MANIFEST", "data": manifest.items, "dataType": manifest.data_type, "phase": "metadata" });
                            let _ = ws_stream.send(Message::Text(msg.to_string().into())).await;
                        }
                    }

                    // Phase3 分批 diff 的待发送批次队列
                    let pending_diff_batches: Arc<
                        tokio::sync::Mutex<
                            std::collections::VecDeque<serde_json::Map<String, serde_json::Value>>,
                        >,
                    > = Arc::new(tokio::sync::Mutex::new(std::collections::VecDeque::new()));

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
                                        // 使用批量 diff 协议：查询所有 topic 的本地 hash，发送给桌面端计算
                                        match Phase3Message::get_all_topic_message_hashes(&db.pool).await {
                                            Ok(topic_states) => {
                                                let topic_count = topic_states.len();
                                                pending_msg_topics_task.total.store(topic_count, Ordering::SeqCst);
                                                {
                                                    let mut completed = pending_msg_topics_task.completed.lock().await;
                                                    completed.clear();
                                                }

                                                if topic_count == 0 {
                                                    if let Ok(logger) = sync_logger_task.lock() {
                                                        logger.complete_phase("message");
                                                    }
                                                    let _ = tx_internal.send(SyncCommand::Phase3);
                                                } else {
                                                    // 按消息数量分批，每批最多 3000 条消息，避免超大 WS payload
                                                    let batches = build_diff_batches(topic_states);
                                                    let batch_count = batches.len();
                                                    println!("[SyncService] Phase3 diff split into {} batches (max 3000 msgs/batch)", batch_count);

                                                    let mut first_batch = None;
                                                    {
                                                        let mut pending = pending_diff_batches.lock().await;
                                                        if !batches.is_empty() {
                                                            first_batch = Some(batches[0].clone());
                                                            *pending = batches.into_iter().skip(1).collect();
                                                        }
                                                    }

                                                    if let Some(batch) = first_batch {
                                                        let msg = json!({
                                                            "type": "SYNC_MESSAGE_DIFF_BATCH",
                                                            "topics": batch,
                                                        });
                                                        let _ = ws_stream.send(Message::Text(msg.to_string().into())).await;
                                                    }
                                                }
                                            }
                                            Err(e) => {
                                                println!("[SyncService] Failed to get topic message hashes: {}", e);
                                                let _ = tx_internal.send(SyncCommand::Phase3);
                                            }
                                        }
                                    },
                                    crate::vcp_modules::sync_pipeline::pipeline::PipelineCommand::Phase3 => {
                                            if let Ok(logger) = sync_logger_task.lock() {
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
                                    SyncCommand::Phase1 => {
                                        write_queue_task.flush().await;
                                        if let Err(e) = pipeline_task.on_phase1_completed().await {
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
                                        write_queue_task.flush().await;
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
                                        write_queue_task.flush().await;

                                        // 统一重新计算所有 topic/agent/group 的 content_hash
                                        let db = handle_clone.state::<DbState>();
                                        if let Ok(topic_ids) = Phase3Message::get_all_active_topic_ids(&db.pool).await {
                                            if let Ok(mut tx) = db.pool.begin().await {
                                                for tid in &topic_ids {
                                                    if let Err(e) = HashAggregator::bubble_topic_hash(&mut tx, tid).await {
                                                        println!("[SyncService] Phase3 bubble_topic_hash failed for {}: {}", tid, e);
                                                    }
                                                }
                                                let _ = tx.commit().await;
                                            }
                                        }
                                        if let Ok(mut tx) = db.pool.begin().await {
                                            let agent_rows = sqlx::query("SELECT agent_id FROM agents WHERE deleted_at IS NULL")
                                                .fetch_all(&mut *tx).await.unwrap_or_default();
                                            for row in agent_rows {
                                                let aid: String = row.get("agent_id");
                                                let _ = HashAggregator::bubble_agent_hash(&mut tx, &aid).await;
                                            }
                                            let group_rows = sqlx::query("SELECT group_id FROM groups WHERE deleted_at IS NULL")
                                                .fetch_all(&mut *tx).await.unwrap_or_default();
                                            for row in group_rows {
                                                let gid: String = row.get("group_id");
                                                let _ = HashAggregator::bubble_group_hash(&mut tx, &gid).await;
                                            }
                                            let _ = tx.commit().await;
                                        }

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
                                                total_tasks_task.fetch_add(total_ops, Ordering::SeqCst);

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
                                                    let total_tasks_in = total_tasks_task.clone();
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
                                                            let _ = tx_internal_in.send(SyncCommand::NotifyDelete {
                                                                data_type: data_type_clone,
                                                                id: id.clone()
                                                            });
                                                        }

                                                        if should_decrement {
                                                            let remaining = pending.fetch_sub(1, Ordering::SeqCst);
                                                            let total = total_tasks_in.load(Ordering::SeqCst);
                                                            let done = total.saturating_sub(remaining.saturating_sub(1));

                                                            // 发送进度
                                                            let _ = h_in.emit("vcp-sync-progress", json!({
                                                                "phase": "metadata",
                                                                "total": total,
                                                                "completed": done,
                                                                "message": format!("Syncing Metadata: {}/{}", done, total)
                                                            }));

                                                            if remaining == 1 {
                                                                if let Ok(logger) = sync_logger_in.lock() {
                                                                    logger.complete_phase("metadata");
                                                                }
                                                                let _ = tx_internal_in.send(SyncCommand::Phase1);
                                                            }
                                                        }
                                                    });
                                                }
                                            }
                                        },
                                        Some("SYNC_DIFF_RESULTS_BATCH") => {
                                            if let Some(results) = payload["results"].as_object() {
                                                let settings = crate::vcp_modules::settings_manager::read_settings(h.clone(), h.state()).await.unwrap_or_default();

                                                for (topic_id, result) in results {
                                                    let to_pull_ids: Vec<String> = result["toPull"]
                                                        .as_array()
                                                        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
                                                        .unwrap_or_default();
                                                    let to_push = result["toPush"].as_bool().unwrap_or(false);

                                                    let has_pull = !to_pull_ids.is_empty();
                                                    let has_push = to_push;

                                                    if let Ok(logger) = sync_logger_task.lock() {
                                                        logger.log(LogLevel::Info, "message", &format!("Topic {} diff: pull={} push={}", topic_id, to_pull_ids.len(), if has_push { 1 } else { 0 }));
                                                    }

                                                    if to_push {
                                                        let h_in = h.clone();
                                                        let c_in = c.clone();
                                                        let b_in = base.clone();
                                                        let s_in = sem.clone();
                                                        let token = settings.sync_token.clone();
                                                        let tid = topic_id.clone();
                                                        let sync_state = h.state::<SyncState>();
                                                        let uploaded_hashes = sync_state.uploaded_hashes.clone();
                                                        let tracker = pending_msg_topics_task.clone();
                                                        let tx_internal_msg = tx_internal.clone();
                                                        let sync_logger_msg = sync_logger_task.clone();
                                                        tauri::async_runtime::spawn(async move {
                                                            let _permit = s_in.acquire().await;
                                                            let _ = PushExecutor::push_messages(&h_in, &c_in, &b_in, &token, &tid, Some(uploaded_hashes)).await;

                                                            let db = h_in.state::<DbState>();
                                                            if let Ok(mut tx) = db.pool.begin().await {
                                                                let _ = HashAggregator::bubble_topic_hash(&mut tx, &tid).await;
                                                                let _ = tx.commit().await;
                                                            }

                                                            tracker.mark_completed(&tid, &sync_logger_msg, &tx_internal_msg, &h_in).await;
                                                        });
                                                    }

                                                    if has_pull {
                                                        let h_in = h.clone();
                                                        let c_in = c.clone();
                                                        let b_in = base.clone();
                                                        let s_in = sem.clone();
                                                        let token = settings.sync_token.clone();
                                                        let tid = topic_id.clone();
                                                        let tracker = pending_msg_topics_task.clone();
                                                        let tx_internal_msg = tx_internal.clone();
                                                        let sync_logger_msg = sync_logger_task.clone();
                                                        tauri::async_runtime::spawn(async move {
                                                            let _permit = s_in.acquire().await;
                                                            let _ = PullExecutor::pull_messages(&h_in, &c_in, &b_in, &token, &tid, &to_pull_ids).await;
                                                            tracker.mark_completed(&tid, &sync_logger_msg, &tx_internal_msg, &h_in).await;
                                                        });
                                                    }

                                                    if !has_pull && !has_push {
                                                        pending_msg_topics_task.mark_completed(topic_id, &sync_logger_task, &tx_internal, &handle_clone).await;
                                                    }
                                                }

                                                // 当前批次处理完毕，发送下一批（如果还有）
                                                let mut pending = pending_diff_batches.lock().await;
                                                if let Some(next_batch) = pending.pop_front() {
                                                    println!("[SyncService] Sending next diff batch, {} remaining", pending.len());
                                                    let msg = json!({
                                                        "type": "SYNC_MESSAGE_DIFF_BATCH",
                                                        "topics": next_batch,
                                                    });
                                                    let _ = ws_stream.send(Message::Text(msg.to_string().into())).await;
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
                                                        if let Ok(logger) = sync_logger_task.lock() {
                                                            logger.log(LogLevel::Info, "topic", &format!("Topic diff: pull={} push={}", total_pull, total_push));
                                                        }
                                                        let is_empty = total_pull == 0;
                                                        pending_tasks_task.fetch_add(total_pull, Ordering::SeqCst);
                                                        total_tasks_task.store(total_pull, Ordering::SeqCst);

                                                        for topic_id in pull_agent_topics {
                                                            let h_in = h.clone();
                                                            let c_in = c.clone();
                                                            let b_in = base.clone();
                                                            let token = settings.sync_token.clone();
                                                            let wq_in = wq.clone();
                                                            let pending = pending_tasks_task.clone();
                                                            let total_tasks_task_in = total_tasks_task.clone();
                                                            let tx_internal_in = tx_internal.clone();
                                                            let sync_logger_topic = sync_logger_task.clone();

                                                            tauri::async_runtime::spawn(async move {
                                                                let _ = PullExecutor::pull_agent_topic(&h_in, &c_in, &b_in, &token, &topic_id, &wq_in).await;

                                                                let remaining = pending.fetch_sub(1, Ordering::SeqCst);
                                                                let total = total_tasks_task_in.load(Ordering::SeqCst);
                                                                let done = total.saturating_sub(remaining.saturating_sub(1));
                                                                let _ = h_in.emit("vcp-sync-progress", json!({
                                                                    "phase": "topic",
                                                                    "total": total,
                                                                    "completed": done,
                                                                    "message": format!("Syncing Topics: {}/{}", done, total)
                                                                }));

                                                                if remaining == 1 {
                                                                    if let Ok(logger) = sync_logger_topic.lock() {
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
                                                            let total_tasks_task_in = total_tasks_task.clone();
                                                            let tx_internal_in = tx_internal.clone();
                                                            let sync_logger_topic = sync_logger_task.clone();

                                                            tauri::async_runtime::spawn(async move {
                                                                let _ = PullExecutor::pull_group_topic(&h_in, &c_in, &b_in, &token, &topic_id, &wq_in).await;

                                                                let remaining = pending.fetch_sub(1, Ordering::SeqCst);
                                                                let total = total_tasks_task_in.load(Ordering::SeqCst);
                                                                let done = total.saturating_sub(remaining.saturating_sub(1));
                                                                let _ = h_in.emit("vcp-sync-progress", json!({
                                                                    "phase": "topic",
                                                                    "total": total,
                                                                    "completed": done,
                                                                    "message": format!("Syncing Topics: {}/{}", done, total)
                                                                }));

                                                                if remaining == 1 {
                                                                    if let Ok(logger) = sync_logger_topic.lock() {
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
                                                                logger.complete_phase("topic");
                                                            }
                                                            let _ = tx_internal.send(SyncCommand::Phase2);
                                                        }
                                                    }
                                                }
                                            }
                                        },
                                        Some("PHASE_COMPLETED") => {
                                            write_queue_task.flush().await;

                                            // 清除 agent/group 内存缓存，确保前端 fetchAgents() 读到最新数据
                                            if let Some(agent_state) = handle_clone.try_state::<crate::vcp_modules::agent_service::AgentConfigState>() {
                                                agent_state.caches.clear();
                                                println!("[SyncService] AgentConfigState cache cleared after sync");
                                            }
                                            if let Some(group_state) = handle_clone.try_state::<crate::vcp_modules::group_service::GroupManagerState>() {
                                                group_state.caches.clear();
                                                println!("[SyncService] GroupManagerState cache cleared after sync");
                                            }

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
                                            // 发射前端刷新事件
                                            let _ = handle_clone.emit(
                                                "vcp-sync-completed",
                                                json!({
                                                    "source": "Sync",
                                                    "agentsChanged": true,
                                                    "groupsChanged": true,
                                                    "topicsChanged": true,
                                                    "messagesChanged": true,
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

    /// 每批最多包含的消息数，控制单次 WS payload 大小（约 3000 条消息 ≈ 400-500KB JSON）
    const MAX_MESSAGES_PER_BATCH: usize = 3000;

    fn build_diff_batches(
        topic_states: std::collections::HashMap<
            String,
            crate::vcp_modules::sync_pipeline::phase3_message::TopicLocalState,
        >,
    ) -> std::collections::VecDeque<serde_json::Map<String, serde_json::Value>> {
        let mut batches = std::collections::VecDeque::new();
        let mut current_batch = serde_json::Map::new();
        let mut current_msg_count = 0usize;

        for (topic_id, state) in topic_states {
            let msg_count = state.messages.len();
            // 如果当前批次非空且加入此 topic 会超限，先结算当前批次
            if current_msg_count > 0 && current_msg_count + msg_count > MAX_MESSAGES_PER_BATCH {
                batches.push_back(current_batch);
                current_batch = serde_json::Map::new();
                current_msg_count = 0;
            }

            let mut msg_map = serde_json::Map::new();
            for (msg_id, hash) in state.messages {
                msg_map.insert(msg_id, serde_json::Value::String(hash));
            }
            let topic_obj = serde_json::json!({
                "topicHash": state.topic_hash,
                "messages": msg_map,
            });
            current_batch.insert(topic_id, topic_obj);
            current_msg_count += msg_count;
        }

        if !current_batch.is_empty() {
            batches.push_back(current_batch);
        }

        batches
    }

    SyncState {
        ws_sender: tx,
        connection_status,
        uploaded_hashes: Arc::new(RwLock::new(HashSet::new())),
        avatar_color_cache: Arc::new(DashMap::new()),
    }
}

#[tauri::command]
pub async fn get_sync_status(state: State<'_, SyncState>) -> Result<String, String> {
    Ok(state.connection_status.read().await.clone())
}

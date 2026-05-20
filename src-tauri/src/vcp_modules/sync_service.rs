use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::db_write_queue::DbWriteQueue;
use crate::vcp_modules::sync_executor::{BatchPullResult, PullExecutor, PushExecutor};
use crate::vcp_modules::sync_hash::{HashAggregator, HashInitializer};
use crate::vcp_modules::sync_logger::{LogLevel, SyncLogger};
use crate::vcp_modules::sync_pipeline::{Phase1Metadata, Phase3Message, SyncPipeline};
use crate::vcp_modules::sync_types::SyncDataType;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use sqlx::Row;
use std::collections::HashSet;
use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, Runtime, State};
use tokio::sync::{mpsc, RwLock, Semaphore};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};

const EXPECTED_PLUGIN_VERSION: &str = "0.9.14";
const VERSION_CHECK_TIMEOUT: Duration = Duration::from_secs(5);

pub struct SyncState {
    pub ws_sender: mpsc::UnboundedSender<SyncCommand>,
    pub connection_status: Arc<RwLock<String>>,
    pub uploaded_hashes: Arc<RwLock<HashSet<String>>>,
    pub is_syncing: Arc<std::sync::atomic::AtomicBool>,
    pub current_log_path: Arc<RwLock<Option<String>>>,
}

/// 追踪 Phase 3 中已处理完成的 topic，替代 AtomicU32 避免双重递减下溢
struct Phase3Tracker {
    completed: tokio::sync::Mutex<HashSet<String>>,
    modified: tokio::sync::Mutex<HashSet<String>>,
    total: std::sync::atomic::AtomicUsize,
}

impl Phase3Tracker {
    /// 标记某个 topic 为数据已修改（实际发生了 pull/push）
    async fn mark_modified(&self, topic_id: &str) {
        let mut modified = self.modified.lock().await;
        modified.insert(topic_id.to_string());
    }

    /// 标记某个 topic 已完成。如果是首次标记，返回 true；否则返回 false。
    /// 当所有 topic 都完成时，触发 complete_phase 和 Phase3 命令。
    async fn mark_completed(
        &self,
        topic_id: &str,
        logger: &Arc<Mutex<SyncLogger>>,
        tx: &mpsc::UnboundedSender<SyncCommand>,
        app_handle: &AppHandle,
        quiet: bool,
    ) -> bool {
        let mut completed = self.completed.lock().await;
        let is_new = completed.insert(topic_id.to_string());
        if is_new {
            let done = completed.len();
            let total = self.total.load(Ordering::SeqCst);

            if !quiet {
                if let Ok(mut logger) = logger.lock() {
                    logger.log_operation("messages", "topic", topic_id, true, None);
                }
            }

            // 发送实时进度事件
            let _ = app_handle.emit(
                "vcp-sync-progress",
                json!({
                    "phase": "messages",
                    "total": total,
                    "completed": done,
                    "message": format!("Syncing Messages: {}/{}", done, total)
                }),
            );

            if done == total {
                if let Ok(mut logger) = logger.lock() {
                    logger.complete_phase("messages");
                }
                let _ = tx.send(SyncCommand::Finalize);
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
        // [Evolution] 动态并发控制：根据核心数动态调整
        // 核心数 * 1.5 是 IO 密集型任务的平衡点，但在移动端需严格限制上限以保护 UI 响应
        let cores = std::thread::available_parallelism()
            .map(|p| p.get())
            .unwrap_or(4);

        let concurrency = ((cores as f32) * 1.5).clamp(6.0, 12.0) as usize;
        println!(
            "[Sync] Auto-optimized concurrency set to {} (cores: {})",
            concurrency, cores
        );

        Self {
            semaphore: Arc::new(Semaphore::new(concurrency)),
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
    StartTopicMetadata,   // Phase 2 start
    StartTopicValidation, // Phase 2.5 start
    StartMessages,        // Phase 3 start
    Finalize,
    NotifyDelete {
        data_type: SyncDataType,
        id: String,
    },
    StartManualSync,
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

    // 直接发射前端 syncSession 监听的 vcp-sync-status
    let _ = app_handle.emit(
        "vcp-sync-status",
        json!({
            "status": next_status,
            "message": message,
            "source": "Sync"
        }),
    );

    // 同步发射到 Mini Log Terminal
    let level = match next_status {
        "open" => "success",
        "error" => "error",
        "connecting" => "info",
        _ => "info",
    };
    emit_sync_log(app_handle, level, message);
}

pub fn init_sync_service(_app_handle: AppHandle) -> SyncState {
    let (tx, _rx) = mpsc::unbounded_channel::<SyncCommand>();
    SyncState {
        ws_sender: tx,
        connection_status: Arc::new(RwLock::new(String::from("disconnected"))),
        uploaded_hashes: Arc::new(RwLock::new(HashSet::new())),
        is_syncing: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        current_log_path: Arc::new(RwLock::new(None)),
    }
}

async fn run_sync_session(
    app_handle: AppHandle,
    tx: mpsc::UnboundedSender<SyncCommand>,
    mut rx: mpsc::UnboundedReceiver<SyncCommand>,
    connection_status: Arc<RwLock<String>>,
) {
    let handle_clone = app_handle.clone();
    let tx_internal = tx.clone();
    let connection_status_for_task = connection_status.clone();

    let http_client = reqwest::Client::new();
    let mut retry_count = 0u32;
    const MAX_RETRIES: u32 = 3;
    let mut retry_delay = Duration::from_millis(500);

    let db = app_handle.state::<DbState>();
    let mut write_queue = DbWriteQueue::new(db.pool.clone(), db.path.clone());
    let sync_log_level = LogLevel::Info;
    let log_dir = app_handle
        .path()
        .app_log_dir()
        .ok()
        .map(|d| d.join("sync_logs"));
    let sync_logger = Arc::new(Mutex::new(SyncLogger::new_session(sync_log_level, log_dir)));
    {
        let sync_state = app_handle.state::<SyncState>();
        let log_path = {
            let logger = sync_logger.lock();
            logger.ok().and_then(|l| l.log_path().cloned())
        };
        if let Some(path) = log_path {
            let mut guard = sync_state.current_log_path.write().await;
            *guard = Some(path.to_string_lossy().to_string());
        }
    }
    write_queue.set_logger(sync_logger.clone());
    let write_queue = Arc::new(write_queue);

    let network_semaphore = Arc::new(NetworkAwareSemaphore::new());
    let (pipeline_tx, mut pipeline_rx) =
        mpsc::unbounded_channel::<crate::vcp_modules::sync_pipeline::pipeline::PipelineCommand>();
    let pipeline = Arc::new(SyncPipeline::new(pipeline_tx));
    let pending_tasks = Arc::new(AtomicU32::new(0));
    let total_tasks = Arc::new(AtomicU32::new(0));
    let pending_message_topics = Arc::new(Phase3Tracker {
        completed: tokio::sync::Mutex::new(HashSet::new()),
        modified: tokio::sync::Mutex::new(HashSet::new()),
        total: std::sync::atomic::AtomicUsize::new(0),
    });

    let semaphore_task = network_semaphore.clone();
    let pipeline_task = pipeline.clone();
    let write_queue_task = write_queue.clone();
    let pending_tasks_task = pending_tasks.clone();
    let total_tasks_task = total_tasks.clone();
    let pending_msg_topics_task = pending_message_topics.clone();
    let sync_logger_task = sync_logger.clone();

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
                        emit_sync_log(&handle_clone, "error", "同步服务 URL 未配置，请检查设置");
                        publish_sync_status(
                            &handle_clone,
                            &connection_status_for_task,
                            "error",
                            "同步服务 URL 未配置",
                        )
                        .await;
                        break;
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
                    emit_sync_log(&handle_clone, "error", "无法读取同步配置");
                    publish_sync_status(
                        &handle_clone,
                        &connection_status_for_task,
                        "error",
                        "无法读取同步配置",
                    )
                    .await;
                    break;
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

        let phase_gate: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));

        match connect_async(&ws_url).await {
            Ok((mut ws_stream, _)) => {
                retry_count = 0;
                retry_delay = Duration::from_millis(500);

                // ── 版本验证握手 ──
                {
                    let version_req = json!({
                        "type": "VERSION_CHECK",
                        "mobileVersion": env!("CARGO_PKG_VERSION")
                    });
                    let _ = ws_stream
                        .send(Message::Text(version_req.to_string().into()))
                        .await;
                    emit_sync_log(&handle_clone, "info", "正在验证桌面端插件版本...");

                    let version_ok = tokio::time::timeout(VERSION_CHECK_TIMEOUT, async {
                        while let Some(Ok(msg)) = ws_stream.next().await {
                            if let Message::Text(text) = msg {
                                if let Ok(payload) = serde_json::from_str::<Value>(&text) {
                                    if payload.get("type").and_then(|v| v.as_str())
                                        == Some("VERSION_ACK")
                                    {
                                        return payload
                                            .get("version")
                                            .and_then(|v| v.as_str())
                                            .map(|s| s.to_string());
                                    }
                                }
                            }
                        }
                        None
                    })
                    .await
                    .ok()
                    .flatten();

                    match version_ok {
                        Some(plugin_version) => {
                            if plugin_version == EXPECTED_PLUGIN_VERSION {
                                emit_sync_log(
                                    &handle_clone,
                                    "success",
                                    &format!("桌面端插件版本 v{} 验证通过", plugin_version),
                                );
                            } else {
                                publish_sync_status(
                                    &handle_clone,
                                    &connection_status_for_task,
                                    "error",
                                    &format!(
                                        "桌面端插件版本 v{} 与期望版本 v{} 不兼容",
                                        plugin_version, EXPECTED_PLUGIN_VERSION
                                    ),
                                )
                                .await;
                                emit_sync_log(&handle_clone, "error", "请前往 https://github.com/MRiecy/VCPMobile/releases 下载最新同步插件");
                                break;
                            }
                        }
                        None => {
                            publish_sync_status(
                                &handle_clone,
                                &connection_status_for_task,
                                "error",
                                "版本验证超时，桌面端插件可能过旧或不支持版本校验",
                            )
                            .await;
                            break;
                        }
                    }
                }

                if let Ok(mut logger) = sync_logger_task.lock() {
                    logger.start_phase("owner_metadata", 0);
                }
                emit_sync_log(&handle_clone, "info", "=== Phase 1: Owner Metadata ===");
                let _ = ws_stream
                    .send(Message::Text(
                        json!({ "type": "PHASE_START", "phase": "owner_metadata" })
                            .to_string()
                            .into(),
                    ))
                    .await;
                publish_sync_status(
                    &handle_clone,
                    &connection_status_for_task,
                    "open",
                    "同步服务已连接",
                )
                .await;

                // 同步连接成功提示
                let _ = handle_clone.emit(
                    "vcp-system-event",
                    json!({
                        "type": "vcp-log-message",
                        "data": {
                            "id": "vcp_sync_connection_status",
                            "status": "success",
                            "tool_name": "Sync",
                            "content": "已连接桌面端",
                            "source": "Sync"
                        }
                    }),
                );
                let db = handle_clone.state::<DbState>();
                if let Err(e) = HashInitializer::ensure_all_agent_hashes(&db.pool).await {
                    if let Ok(mut logger) = sync_logger_task.lock() {
                        logger.log(
                            LogLevel::Error,
                            "owner_metadata",
                            &format!("Failed to initialize agent hashes: {}", e),
                        );
                    }

                    // 同步初始化失败提示
                    let _ = handle_clone.emit(
                        "vcp-system-event",
                        json!({
                            "type": "vcp-log-message",
                            "data": {
                                "id": "vcp_sync_connection_status",
                                "status": "error",
                                "tool_name": "同步初始化失败",
                                "content": "数据库初始化失败",
                                "source": "Sync"
                            }
                        }),
                    );
                    tokio::time::sleep(retry_delay).await;
                    retry_delay = (retry_delay * 2).min(Duration::from_secs(60));
                    continue;
                }
                if let Err(e) = HashInitializer::ensure_all_group_hashes(&db.pool).await {
                    if let Ok(mut logger) = sync_logger_task.lock() {
                        logger.log(
                            LogLevel::Error,
                            "owner_metadata",
                            &format!("Hash init error: {}", e),
                        );
                    }

                    // 同步初始化失败提示
                    let _ = handle_clone.emit(
                        "vcp-system-event",
                        json!({
                            "type": "vcp-log-message",
                            "data": {
                                "id": "vcp_sync_connection_status",
                                "status": "error",
                                "tool_name": "同步初始化失败",
                                "content": "数据库初始化失败",
                                "source": "Sync"
                            }
                        }),
                    );
                    tokio::time::sleep(retry_delay).await;
                    retry_delay = (retry_delay * 2).min(Duration::from_secs(60));
                    continue;
                }

                // Phase3 分批 diff 的待发送批次队列
                let pending_diff_batches: Arc<
                    tokio::sync::Mutex<
                        std::collections::VecDeque<serde_json::Map<String, serde_json::Value>>,
                    >,
                > = Arc::new(tokio::sync::Mutex::new(std::collections::VecDeque::new()));

                // Phase 2 筛选出的需要消息同步的 topic 列表
                let changed_topics: Arc<tokio::sync::Mutex<Vec<String>>> =
                    Arc::new(tokio::sync::Mutex::new(Vec::new()));

                // V2: Phase 1 筛选出的内容有变动的 owner (Agent/Group) 列表
                let changed_owners: Arc<tokio::sync::Mutex<HashSet<String>>> =
                    Arc::new(tokio::sync::Mutex::new(HashSet::new()));

                // 用于跟踪 manifest diff 结果是否全部收到，防止 total_ops=0 时 Phase 1 卡住
                let expected_manifest_count = Arc::new(AtomicU32::new(0));
                let manifest_responses_received = Arc::new(AtomicU32::new(0));
                // 1: 基础 Metadata (agent, group, avatar), 2: Topic Metadata
                let manifest_phase = Arc::new(AtomicU8::new(1));
                let mut sync_success = false;

                loop {
                    tokio::select! {
                        Some(cmd) = pipeline_rx.recv() => {
                            match cmd {
                                crate::vcp_modules::sync_pipeline::pipeline::PipelineCommand::StartTopicMetadata => {
                                    // Phase 2: 拉取缺失的 Topic Configs
                                    let db = handle_clone.state::<DbState>();
                                    let owners = {
                                        let guard = changed_owners.lock().await;
                                        guard.iter().cloned().collect::<Vec<String>>()
                                    };

                                    if owners.is_empty() {
                                        let _ = tx_internal.send(SyncCommand::StartTopicValidation);
                                    } else {
                                        if let Ok(manifest) = Phase1Metadata::build_targeted_topic_manifest(&db.pool, &owners).await {
                                            manifest_phase.store(2, Ordering::SeqCst);
                                            expected_manifest_count.store(1, Ordering::SeqCst);
                                            manifest_responses_received.store(0, Ordering::SeqCst);
                                            pending_tasks_task.store(0, Ordering::SeqCst);
                                            total_tasks_task.store(0, Ordering::SeqCst);

                                            if let Ok(mut logger) = sync_logger_task.lock() {
                                                logger.start_phase("topic_metadata", 1);
                                                logger.log(LogLevel::Info, "topic_metadata", "=== Phase 2: Pulling Topic Metadata ===");
                                            }
                                            emit_sync_log(&handle_clone, "info", "=== Phase 2: Pulling Topic Metadata ===");
                                            let _ = ws_stream.send(Message::Text(json!({ "type": "PHASE_START", "phase": "topic_metadata" }).to_string().into())).await;

                                            let msg = json!({
                                                "type": "SYNC_MANIFEST",
                                                "data": manifest.items,
                                                "dataType": manifest.data_type,
                                                "phase": 2, // Use explicit Phase ID 2
                                                "targetedOwners": owners
                                            });
                                            let _ = ws_stream.send(Message::Text(msg.to_string().into())).await;
                                        } else {
                                            let _ = tx_internal.send(SyncCommand::StartTopicValidation);
                                        }
                                    }
                                },
                                crate::vcp_modules::sync_pipeline::pipeline::PipelineCommand::StartTopicValidation => {
                                    // Phase 2.5: 双哈希批量比对
                                    if let Ok(mut logger) = sync_logger_task.lock() {
                                        logger.log(LogLevel::Info, "topic_metadata", "=== Phase 2.5: Validating Topic Hashes ===");
                                    }
                                    emit_sync_log(&handle_clone, "info", "=== Phase 2.5: Validating Topic Hashes ===");

                                    let db = handle_clone.state::<DbState>();
                                    let owners = {
                                        let guard = changed_owners.lock().await;
                                        guard.iter().cloned().collect::<Vec<String>>()
                                    };

                                    match Phase3Message::get_targeted_topic_hashes(&db.pool, &owners).await {
                                        Ok(topic_hashes) => {
                                            let mut hash_map = serde_json::Map::new();
                                            for (topic_id, (conf_h, cont_h)) in topic_hashes {
                                                hash_map.insert(topic_id, json!({
                                                    "configHash": conf_h,
                                                    "contentHash": cont_h
                                                }));
                                            }
                                            let msg = json!({
                                                "type": "SYNC_TOPIC_HASH_BATCH_V2",
                                                "hashes": hash_map,
                                            });
                                            let _ = ws_stream.send(Message::Text(msg.to_string().into())).await;
                                        }
                                        Err(e) => {
                                            println!("[SyncService] Failed to get targeted topic hashes: {}", e);
                                            let _ = tx_internal.send(SyncCommand::StartMessages);
                                        }
                                    }
                                },
                                crate::vcp_modules::sync_pipeline::pipeline::PipelineCommand::StartMessages => {
                                    if let Ok(mut logger) = sync_logger_task.lock() {
                                        logger.start_phase("messages", 0);
                                        logger.log(LogLevel::Info, "messages", "=== Phase 3: Messages ===");
                                    }
                                    emit_sync_log(&handle_clone, "info", "=== Phase 3: Messages ===");
                                    let _ = ws_stream.send(Message::Text(json!({ "type": "PHASE_START", "phase": "messages" }).to_string().into())).await;

                                    let db = handle_clone.state::<DbState>();
                                    let changed_ids = {
                                        let guard = changed_topics.lock().await;
                                        guard.clone()
                                    };

                                    if changed_ids.is_empty() {
                                        if let Ok(mut logger) = sync_logger_task.lock() {
                                            logger.complete_phase("messages");
                                        }
                                        emit_sync_log(&handle_clone, "success", "Message phase skipped (no changed topics), proceeding to hash alignment");
                                        let _ = tx_internal.send(SyncCommand::Finalize);
                                    } else {
                                        match Phase3Message::get_topic_message_hashes(&db.pool, &changed_ids).await {
                                            Ok(topic_states) => {
                                                let topic_count = topic_states.len();
                                                pending_msg_topics_task.total.store(topic_count, Ordering::SeqCst);
                                                {
                                                    let mut completed = pending_msg_topics_task.completed.lock().await;
                                                    completed.clear();
                                                }

                                                // 清空可能残留的旧批次，防止断线重连后发送过时数据
                                                {
                                                    let mut pending = pending_diff_batches.lock().await;
                                                    pending.clear();
                                                }
                                                // 按消息数量分批，每批最多 10000 条消息，避免超大 WS payload
                                                let batches = build_diff_batches(topic_states);
                                                let batch_count = batches.len();
                                                println!("[SyncService] Phase3 diff split into {} batches (max {} msgs/batch)", batch_count, MAX_MESSAGES_PER_BATCH);

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
                                            Err(e) => {
                                                println!("[SyncService] Failed to get topic message hashes: {}", e);
                                                let _ = tx_internal.send(SyncCommand::Finalize);
                                            }
                                        }
                                    }
                                },
                                crate::vcp_modules::sync_pipeline::pipeline::PipelineCommand::Finalize => {
                                    if let Ok(mut logger) = sync_logger_task.lock() {
                                        logger.complete_phase("sync");
                                        (*logger).end_session();
                                    }

                                    // [修复] 移动端主动关闭 WS 前，先通知前端同步已完成
                                    // （服务器返回的 PHASE_COMPLETED 永远不会被收到，因此不能依赖 WS 响应处理器触发完成）
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
                                    {
                                        let mut guard = connection_status_for_task.write().await;
                                        *guard = "completed".to_string();
                                    }
                                    let _ = handle_clone.emit(
                                        "vcp-sync-status",
                                        json!({ "status": "completed", "message": "同步完成", "source": "Sync" }),
                                    );

                                    sync_success = true;
                                    let _ = ws_stream.send(Message::Text(json!({ "type": "PHASE_COMPLETED" }).to_string().into())).await;
                                    let _ = ws_stream.close(None).await;
                                    break;
                                },
                            }
                        },
                        Some(cmd) = rx.recv() => {
                            match cmd {
                                SyncCommand::NotifyLocalChange { id, data_type, hash, ts } => {
                                    let msg = json!({ "type": "SYNC_ENTITY_UPDATE", "id": id, "dataType": data_type, "hash": hash, "ts": ts });
                                    let _ = ws_stream.send(Message::Text(msg.to_string().into())).await;
                                },
                                SyncCommand::StartTopicMetadata => {
                                    let should_flush = {
                                        if let Ok(mut gate) = phase_gate.lock() {
                                            gate.insert("topic_metadata".to_string())
                                        } else {
                                            false
                                        }
                                    };
                                    if should_flush {
                                        // 1. 通知桌面端前一相位已完成
                                        let _ = ws_stream.send(Message::Text(json!({ "type": "PHASE_COMPLETED", "phase": "owner_metadata" }).to_string().into())).await;

                                        // 2. 强制落盘并触发 Pipeline 钩子
                                        write_queue_task.flush().await;
                                        let _ = pipeline_task.on_owner_metadata_done().await;
                                    }
                                },
                                SyncCommand::StartTopicValidation => {
                                    let should_flush = {
                                        if let Ok(mut gate) = phase_gate.lock() {
                                            gate.insert("topic_validation".to_string())
                                        } else {
                                            false
                                        }
                                    };
                                    if should_flush {
                                        // 通知桌面端前一相位已完成
                                        let _ = ws_stream.send(Message::Text(json!({ "type": "PHASE_COMPLETED", "phase": "topic_metadata" }).to_string().into())).await;

                                        write_queue_task.flush().await;
                                        let _ = pipeline_task.on_topic_metadata_pull_done().await;
                                    }
                                },
                                SyncCommand::StartMessages => {
                                    let should_flush = {
                                        if let Ok(mut gate) = phase_gate.lock() {
                                            gate.insert("messages".to_string())
                                        } else {
                                            false
                                        }
                                    };
                                    if should_flush {
                                        write_queue_task.flush().await;
                                        let _ = pipeline_task.on_topic_validation_done().await;
                                    }
                                },
                                SyncCommand::Finalize => {
                                    let should_flush = {
                                        if let Ok(mut gate) = phase_gate.lock() {
                                            gate.insert("finalize".to_string())
                                        } else {
                                            false
                                        }
                                    };
                                    if should_flush {
                                        // 通知桌面端消息相位已完成
                                        let _ = ws_stream.send(Message::Text(json!({ "type": "PHASE_COMPLETED", "phase": "messages" }).to_string().into())).await;

                                        write_queue_task.flush().await;

                                        // 全局 Hash 冒泡
                                        let db = handle_clone.state::<DbState>();
                                        let modified_topics = {
                                            let guard = pending_msg_topics_task.modified.lock().await;
                                            guard.clone()
                                        };
                                        if !modified_topics.is_empty() {
                                            println!("[SyncService] Finalizing {} modified topics (recalculating hashes)...", modified_topics.len());
                                            emit_sync_log(&handle_clone, "info", &format!("正在校验 {} 个话题的一致性...", modified_topics.len()));

                                            if let Ok(mut tx) = db.pool.begin().await {
                                                // 1. [Batch Optimization] 一条 SQL 更新所有受影响话题的消息计数和时间戳
                                                let placeholders = modified_topics.iter().map(|_| "?").collect::<Vec<_>>().join(",");
                                                let sql = format!(
                                                    "UPDATE topics SET
                                                        msg_count = (SELECT COUNT(*) FROM messages WHERE messages.topic_id = topics.topic_id AND deleted_at IS NULL),
                                                        updated_at = ?
                                                     WHERE topic_id IN ({})", placeholders
                                                );
                                                let mut query = sqlx::query(&sql).bind(chrono::Utc::now().timestamp_millis());
                                                for tid in &modified_topics { query = query.bind(tid); }
                                                let _ = query.execute(&mut *tx).await;

                                                // 2. 逐话题计算指纹（涉及 MerkleRoot，必须逐个处理但现在已无其他 SQL 负担）
                                                let mut affected_agents: HashSet<String> = HashSet::new();
                                                let mut affected_groups: HashSet<String> = HashSet::new();

                                                for tid in &modified_topics {
                                                    if let Err(e) = HashAggregator::bubble_topic_hash(&mut tx, tid).await {
                                                        println!("[SyncService] bubble_topic_hash failed for {}: {}", tid, e);
                                                    }
                                                    if let Ok(row) = sqlx::query("SELECT owner_id, owner_type FROM topics WHERE topic_id = ?").bind(tid).fetch_one(&mut *tx).await {
                                                        let owner_id: String = row.get("owner_id");
                                                        let owner_type: String = row.get("owner_type");
                                                        if owner_type == "agent" { affected_agents.insert(owner_id); }
                                                        else if owner_type == "group" { affected_groups.insert(owner_id); }
                                                    }
                                                }
                                                for aid in affected_agents { let _ = HashAggregator::bubble_agent_hash(&mut tx, &aid).await; }
                                                for gid in affected_groups { let _ = HashAggregator::bubble_group_hash(&mut tx, &gid).await; }
                                                let _ = tx.commit().await;
                                            }
                                        }
                                        let _ = pipeline_task.on_messages_done().await;
                                    }
                                },
                                SyncCommand::NotifyDelete { data_type, id } => {
                                    let msg = json!({ "type": "SYNC_ENTITY_DELETE", "id": id, "dataType": data_type });
                                    let _ = ws_stream.send(Message::Text(msg.to_string().into())).await;
                                },
                                SyncCommand::StartManualSync => {
                                    let db = handle_clone.state::<DbState>();
                                    manifest_phase.store(1, Ordering::SeqCst);
                                    if let Ok(manifests) = Phase1Metadata::build_phase1_manifests(&db.pool).await {
                                        let count = manifests.len() as u32;
                                        expected_manifest_count.store(count, Ordering::SeqCst);
                                        manifest_responses_received.store(0, Ordering::SeqCst);

                                        if let Ok(mut logger) = sync_logger_task.lock() {
                                            logger.set_phase_expected("owner_metadata", count);
                                        }
                                        for manifest in manifests {
                                            let msg = json!({
                                                "type": "SYNC_MANIFEST",
                                                "data": manifest.items,
                                                "dataType": manifest.data_type,
                                                "phase": 1 // Explicit Phase ID
                                            });
                                            let _ = ws_stream.send(Message::Text(msg.to_string().into())).await;
                                        }
                                    }
                                },
                            }
                        },
                        res = ws_stream.next() => {
                            match res {
                                Some(Ok(msg)) => {
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
                                    Some("SYNC_ERROR") => {
                                        let message = payload["message"].as_str().unwrap_or("Unknown desktop error");
                                        let code = payload["code"].as_u64().unwrap_or(500);
                                        let err_msg = format!("Desktop Error ({}): {}", code, message);
                                        println!("[SyncService] {}", err_msg);
                                        emit_sync_log(&handle_clone, "error", &err_msg);
                                        publish_sync_status(&handle_clone, &connection_status_for_task, "error", &err_msg).await;
                                        // 致命错误，建议断开或重试
                                    },
                                    Some("SYNC_DIFF_RESULTS") => {
                                        if let Some(items) = payload["data"].as_array() {
                                            let Some(data_type) = parse_sync_data_type(&payload["dataType"]) else { continue; };
                                            let settings = crate::vcp_modules::settings_manager::read_settings(h.clone(), h.state()).await.unwrap_or_default();
                                            let items_clone: Vec<serde_json::Value> = items.clone();

                                            // 统计有效操作数（排除 SKIP）
                                            let pull_count = items_clone.iter().filter(|i| i["action"] == "PULL").count() as u32;
                                            let push_count = items_clone.iter().filter(|i| i["action"] == "PUSH").count() as u32;
                                            let delete_count = items_clone.iter().filter(|i| i["action"] == "DELETE").count() as u32;
                                            let push_delete_count = items_clone.iter().filter(|i| i["action"] == "PUSH_DELETE").count() as u32;
                                            let total_ops = pull_count + push_count + delete_count + push_delete_count;

                                            if total_ops > 0 {
                                                let phase_tag = match data_type {
                                                    SyncDataType::Agent | SyncDataType::Group | SyncDataType::Avatar => "owner_metadata",
                                                    SyncDataType::Topic => "topic_metadata",
                                                    SyncDataType::Message => "messages",
                                                };
                                                let msg = format!("[{}] Diff: pull={} push={} delete={} push_delete={}",
                                                    data_type, pull_count, push_count, delete_count, push_delete_count);
                                                println!("[Sync] [{}] {}", phase_tag, msg);
                                                emit_sync_log(&handle_clone, "info", &msg);

                                                if let Ok(mut logger) = sync_logger_task.lock() {
                                                    logger.log_operation(phase_tag, &data_type.to_string(), "manifest", true,
                                                        Some(&format!("pull={} push={} delete={} push_delete={}", pull_count, push_count, delete_count, push_delete_count)));
                                                }
                                            }
                                            pending_tasks_task.fetch_add(total_ops, Ordering::SeqCst);
                                            total_tasks_task.fetch_add(total_ops, Ordering::SeqCst);

                                            let received = manifest_responses_received.fetch_add(1, Ordering::SeqCst) + 1;
                                            let expected = expected_manifest_count.load(Ordering::SeqCst);
                                            let current_phase = manifest_phase.load(Ordering::SeqCst);
                                            let msg_phase = payload["phase"].as_u64().unwrap_or(0) as u8;

                                            if received == expected && (msg_phase == current_phase || msg_phase == 0) {
                                                let current_pending = pending_tasks_task.load(Ordering::SeqCst);
                                                println!("[SyncService] All manifests received for Phase {}: dataType={}, pending={}",
                                                    current_phase, data_type, current_pending);

                                                if current_pending == 0 {
                                                    if current_phase == 1 { let _ = tx_internal.send(SyncCommand::StartTopicMetadata); }
                                                    else if current_phase == 2 { let _ = tx_internal.send(SyncCommand::StartTopicValidation); }
                                                } else {
                                                    let tx_internal_wd = tx_internal.clone();
                                                    let current_phase_wd = current_phase;
                                                    let manifest_phase_wd = manifest_phase.clone();
                                                    let pending_wd = pending_tasks_task.clone();
                                                    let handle_clone_wd = handle_clone.clone();

                                                    tauri::async_runtime::spawn(async move {
                                                        let mut last_pending = pending_wd.load(Ordering::SeqCst);
                                                        let mut stuck_count = 0;
                                                        loop {
                                                            tokio::time::sleep(Duration::from_secs(10)).await;
                                                            if manifest_phase_wd.load(Ordering::SeqCst) != current_phase_wd { break; }
                                                            let current_pending = pending_wd.load(Ordering::SeqCst);
                                                            if current_pending == 0 { break; }

                                                            if current_pending == last_pending {
                                                                stuck_count += 1;
                                                                println!("[SyncService] WATCHDOG: Phase {} pending count stuck at {} ({} ticks)",
                                                                    current_phase_wd, current_pending, stuck_count);
                                                            } else {
                                                                stuck_count = 0;
                                                                last_pending = current_pending;
                                                            }

                                                            if stuck_count >= 6 {
                                                                println!("[SyncService] WATCHDOG FATAL: Phase {} DEADLOCK detected. Forcing transition...", current_phase_wd);
                                                                emit_sync_log(&handle_clone_wd, "warn", &format!("检测到同步停滞 (Phase {})，正在尝试强制恢复...", current_phase_wd));
                                                                if current_phase_wd == 1 { let _ = tx_internal_wd.send(SyncCommand::StartTopicMetadata); }
                                                                else if current_phase_wd == 2 { let _ = tx_internal_wd.send(SyncCommand::StartTopicValidation); }
                                                                break;
                                                            } else if stuck_count >= 1 {
                                                                emit_sync_log(&handle_clone_wd, "warn", &format!("同步进度缓慢 (Phase {})，剩余任务: {}...", current_phase_wd, current_pending));
                                                            }
                                                        }
                                                    });
                                                }
                                            }

                                            // 归类任务
                                            let mut batch_pull_requests = Vec::new();
                                            let mut push_topics_to_fetch = Vec::new();
                                            let mut other_items = Vec::new();

                                            for item in items_clone {
                                                let id = item["id"].as_str().unwrap_or_default().to_string();
                                                let action = item["action"].as_str().unwrap_or_default().to_string();

                                                // V2: Populate changed_owners for Phase 2 Topic Sync
                                                if data_type == SyncDataType::Agent || data_type == SyncDataType::Group {
                                                    let is_mismatched = item["mismatchedContent"].as_bool().unwrap_or(false);
                                                    if action == "PUSH" || action == "PULL" || is_mismatched {
                                                        let mut owners = changed_owners.lock().await;
                                                        owners.insert(id.clone());
                                                    }
                                                }

                                                if action == "PULL" && (data_type == SyncDataType::Topic || data_type == SyncDataType::Agent || data_type == SyncDataType::Group) {
                                                    let type_str = match data_type {
                                                        SyncDataType::Topic => if item["ownerType"] == "group" { "group_topic" } else { "agent_topic" },
                                                        SyncDataType::Agent => "agent",
                                                        SyncDataType::Group => "group",
                                                        _ => unreachable!(),
                                                    };
                                                    batch_pull_requests.push(json!({ "id": id, "type": type_str }));
                                                } else if action == "PUSH" && data_type == SyncDataType::Topic {
                                                    let owner_id = item["ownerId"].as_str().unwrap_or_default().to_string();
                                                    let owner_type = item["ownerType"].as_str().unwrap_or("agent").to_string();
                                                    push_topics_to_fetch.push((id, owner_id, owner_type));
                                                } else {
                                                    other_items.push(item);
                                                }
                                            }

                                            // 派发任务
                                            if !batch_pull_requests.is_empty() {
                                                let h_in = h.clone(); let c_in = c.clone(); let b_in = base.clone();
                                                let token = settings.sync_token.clone(); let wq_in = wq.clone();
                                                let pending = pending_tasks_task.clone(); let total_tasks_in = total_tasks_task.clone();
                                                let tx_internal_in = tx_internal.clone();
                                                let manifest_received_in = manifest_responses_received.clone();
                                                let manifest_expected_in = expected_manifest_count.clone();
                                                let manifest_phase_in = manifest_phase.clone();
                                                let data_type_inner = data_type.clone();

                                                tauri::async_runtime::spawn(async move {
                                                    let chunk_size = match data_type_inner { SyncDataType::Agent | SyncDataType::Group => 50, SyncDataType::Topic => 1000, _ => 100 };
                                                    for chunk in batch_pull_requests.chunks(chunk_size) {
                                                        let sub_batch = chunk.to_vec();
                                                        let sub_count = sub_batch.len() as u32;
                                                        let _ = PullExecutor::pull_entities_batch(&h_in, &c_in, &b_in, &token, sub_batch, &wq_in).await;
                                                        pending.fetch_sub(sub_count, Ordering::SeqCst);
                                                        let current_pending = pending.load(Ordering::SeqCst);
                                                        let total = total_tasks_in.load(Ordering::SeqCst);
                                                        let done = total.saturating_sub(current_pending);
                                                        let _ = h_in.emit("vcp-sync-progress", json!({ "phase": if manifest_phase_in.load(Ordering::SeqCst) == 1 { "owner_metadata" } else { "topic_metadata" }, "total": total, "completed": done, "message": format!("Syncing: {}/{}", done, total) }));
                                                        if current_pending == 0 && manifest_received_in.load(Ordering::SeqCst) == manifest_expected_in.load(Ordering::SeqCst) {
                                                            let phase = manifest_phase_in.load(Ordering::SeqCst);
                                                            if phase == 1 { let _ = tx_internal_in.send(SyncCommand::StartTopicMetadata); }
                                                            else if phase == 2 { let _ = tx_internal_in.send(SyncCommand::StartTopicValidation); }
                                                        }
                                                    }
                                                });
                                            }

                                            if !push_topics_to_fetch.is_empty() {
                                                let h_in = h.clone(); let c_in = c.clone();
                                                let token = settings.sync_token.clone(); let pending = pending_tasks_task.clone();
                                                let total_tasks_in = total_tasks_task.clone(); let tx_internal_in = tx_internal.clone();
                                                let manifest_received_in = manifest_responses_received.clone();
                                                let manifest_expected_in = expected_manifest_count.clone();
                                                let manifest_phase_in = manifest_phase.clone();
                                                let http_url = settings.sync_http_url.clone();

                                                tauri::async_runtime::spawn(async move {
                                                    let db = h_in.state::<DbState>();
                                                    let mut batch_push_requests = Vec::new();

                                                    // 异步批量查询 Topic 元数据
                                                    for (id, _diff_owner_id, owner_type) in push_topics_to_fetch {
                                                        println!("[SyncDebug] Fetching metadata for topic: {}", id);
                                                        let row_res = sqlx::query("SELECT topic_id, title, created_at, locked, unread, owner_id FROM topics WHERE topic_id = ?")
                                                            .bind(&id)
                                                            .fetch_optional(&db.pool)
                                                            .await;

                                                        match row_res {
                                                            Ok(Some(r)) => {
                                                                let db_owner_id: String = r.get("owner_id");
                                                                let tid: String = r.get("topic_id");
                                                                println!("[SyncDebug] Found topic {} (owner: {})", tid, db_owner_id);

                                                                let type_str = if owner_type == "group" { "group_topic" } else { "agent_topic" };
                                                                let dto = if owner_type == "group" {
                                                                    json!({ "id": tid, "name": r.get::<String, _>("title"), "createdAt": r.get::<i64, _>("created_at"), "ownerId": db_owner_id })
                                                                } else {
                                                                    json!({ "id": tid, "name": r.get::<String, _>("title"), "createdAt": r.get::<i64, _>("created_at"), "locked": r.get::<i64, _>("locked") != 0, "unread": r.get::<i64, _>("unread") != 0, "ownerId": db_owner_id })
                                                                };
                                                                batch_push_requests.push(json!({ "id": id, "type": type_str, "data": dto }));
                                                            },
                                                            Ok(None) => {
                                                                println!("[SyncDebug] Topic NOT FOUND in database: {}", id);
                                                                pending.fetch_sub(1, Ordering::SeqCst);
                                                            },
                                                            Err(e) => {
                                                                println!("[SyncDebug] SQL ERROR fetching topic {}: {}", id, e);
                                                                pending.fetch_sub(1, Ordering::SeqCst);
                                                            }
                                                        }
                                                    }

                                                    println!("[SyncDebug] Prepared {} metadata push requests", batch_push_requests.len());

                                                    // 分块发送
                                                    for chunk in batch_push_requests.chunks(1000) {
                                                        let sub_batch = chunk.to_vec();
                                                        let sub_count = sub_batch.len() as u32;
                                                        println!("[SyncDebug] Sending batch of {} topics to desktop", sub_count);

                                                        let push_res = PushExecutor::push_entities_batch(&h_in, &c_in, &http_url, &token, sub_batch).await;
                                                        match push_res {
                                                            Ok(_) => println!("[SyncDebug] Successfully pushed metadata batch to desktop"),
                                                            Err(e) => println!("[SyncDebug] FAILED to push metadata batch: {}", e),
                                                        }

                                                        pending.fetch_sub(sub_count, Ordering::SeqCst);

                                                        let current_pending = pending.load(Ordering::SeqCst);
                                                        let total = total_tasks_in.load(Ordering::SeqCst);
                                                        let done = total.saturating_sub(current_pending);
                                                        let _ = h_in.emit("vcp-sync-progress", json!({ "phase": "topic_metadata", "total": total, "completed": done, "message": format!("Syncing: {}/{}", done, total) }));
                                                    }

                                                    // 信号外移：确保只要 pending 归零且 manifest 已收齐，就触发下一阶段
                                                    let current_pending = pending.load(Ordering::SeqCst);
                                                    if current_pending == 0 && manifest_received_in.load(Ordering::SeqCst) == manifest_expected_in.load(Ordering::SeqCst) {
                                                        let phase = manifest_phase_in.load(Ordering::SeqCst);
                                                        if phase == 1 { let _ = tx_internal_in.send(SyncCommand::StartTopicMetadata); }
                                                        else if phase == 2 { let _ = tx_internal_in.send(SyncCommand::StartTopicValidation); }
                                                    }
                                                });
                                            }

                                            if !other_items.is_empty() {
                                                let h_in = h.clone(); let c_in = c.clone(); let b_in = base.clone();
                                                let token = settings.sync_token.clone(); let wq_in = wq.clone();
                                                let pending = pending_tasks_task.clone(); let total_tasks_in = total_tasks_task.clone();
                                                let tx_internal_in = tx_internal.clone();
                                                let manifest_received_in = manifest_responses_received.clone();
                                                let manifest_expected_in = expected_manifest_count.clone();
                                                let manifest_phase_in = manifest_phase.clone();
                                                let data_type_base = data_type.clone();

                                                tauri::async_runtime::spawn(async move {
                                                    futures_util::stream::iter(other_items).for_each_concurrent(15, |item| {
                                                        let action = item["action"].as_str().unwrap_or_default().to_string();
                                                        let id = item["id"].as_str().unwrap_or_default().to_string();
                                                        let h_task = h_in.clone(); let c_task = c_in.clone(); let b_task = b_in.clone();
                                                        let token_task = token.clone(); let data_type_task = data_type_base.clone();
                                                        let wq_task = wq_in.clone(); let pending_task = pending.clone();
                                                        let total_tasks_task = total_tasks_in.clone(); let tx_internal_task = tx_internal_in.clone();
                                                        let manifest_received_task = manifest_received_in.clone();
                                                        let manifest_expected_task = manifest_expected_in.clone();
                                                        let manifest_phase_task = manifest_phase_in.clone();

                                                        async move {
                                                            let mut should_decrement = true;
                                                            if action == "PULL" {
                                                                if data_type_task == SyncDataType::Avatar {
                                                                    let parts: Vec<&str> = id.split(':').collect();
                                                                    if parts.len() == 2 { let _ = PullExecutor::pull_avatar(&h_task, &c_task, &b_task, &token_task, parts[0], parts[1], &wq_task).await; }
                                                                } else if data_type_task == SyncDataType::Agent { let _ = PullExecutor::pull_agent(&h_task, &c_task, &b_task, &token_task, &id, &wq_task).await; }
                                                                else if data_type_task == SyncDataType::Group { let _ = PullExecutor::pull_group(&h_task, &c_task, &b_task, &token_task, &id, &wq_task).await; }
                                                                else { should_decrement = false; }
                                                            } else if action == "PUSH" {
                                                                if data_type_task == SyncDataType::Agent { let _ = PushExecutor::push_agent(&h_task, &c_task, &b_task, &token_task, &id).await; }
                                                                else if data_type_task == SyncDataType::Group { let _ = PushExecutor::push_group(&h_task, &c_task, &b_task, &token_task, &id).await; }
                                                                else if data_type_task == SyncDataType::Avatar { let parts: Vec<&str> = id.split(':').collect(); if parts.len() == 2 { let _ = PushExecutor::push_avatar(&h_task, &c_task, &b_task, &token_task, parts[0], parts[1]).await; } }
                                                                else { should_decrement = false; }
                                                            } else if action == "DELETE" || action == "PUSH_DELETE" {
                                                                use crate::vcp_modules::sync_executor::delete_executor::DeleteExecutor;
                                                                match data_type_task {
                                                                    SyncDataType::Agent => { let _ = DeleteExecutor::soft_delete_agent(&h_task, &id).await; },
                                                                    SyncDataType::Group => { let _ = DeleteExecutor::soft_delete_group(&h_task, &id).await; },
                                                                    SyncDataType::Avatar => { let parts: Vec<&str> = id.split(':').collect(); if parts.len() == 2 { let _ = DeleteExecutor::soft_delete_avatar(&h_task, parts[0], parts[1]).await; } },
                                                                    SyncDataType::Topic => { let _ = DeleteExecutor::soft_delete_topic(&h_task, &id).await; },
                                                                    _ => {}
                                                                }
                                                                if action == "PUSH_DELETE" { let _ = tx_internal_task.send(SyncCommand::NotifyDelete { data_type: data_type_task, id: id.clone() }); }
                                                            } else { should_decrement = false; }

                                                            if should_decrement {
                                                                pending_task.fetch_sub(1, Ordering::SeqCst);
                                                                let current_pending = pending_task.load(Ordering::SeqCst);
                                                                let total = total_tasks_task.load(Ordering::SeqCst);
                                                                let done = total.saturating_sub(current_pending);
                                                                let _ = h_task.emit("vcp-sync-progress", json!({ "phase": if manifest_phase_task.load(Ordering::SeqCst) == 1 { "owner_metadata" } else { "topic_metadata" }, "total": total, "completed": done, "message": format!("Syncing: {}/{}", done, total) }));
                                                                if current_pending == 0 && manifest_received_task.load(Ordering::SeqCst) == manifest_expected_task.load(Ordering::SeqCst) {
                                                                    let phase = manifest_phase_task.load(Ordering::SeqCst);
                                                                    if phase == 1 { let _ = tx_internal_task.send(SyncCommand::StartTopicMetadata); }
                                                                    else if phase == 2 { let _ = tx_internal_task.send(SyncCommand::StartTopicValidation); }
                                                                }
                                                            }
                                                        }
                                                    }).await;
                                                });
                                            }
                                        }
                                    },
                                    Some("SYNC_DIFF_RESULTS_BATCH") => {
                                        if let Some(results) = payload["results"].as_object() {
                                            let settings = crate::vcp_modules::settings_manager::read_settings(h.clone(), h.state()).await.unwrap_or_default();

                                            // 分类 topics: push_only, push_pull, pull_only
                                            let mut push_topic_ids: Vec<String> = Vec::new();
                                            let mut pull_batch: Vec<(String, Vec<String>)> = Vec::new();

                                            for (topic_id, result) in results {
                                                let to_pull_ids: Vec<String> = result["toPull"]
                                                    .as_array()
                                                    .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
                                                    .unwrap_or_default();
                                                let to_push = result["toPush"].as_bool().unwrap_or(false);

                                                if !to_push && to_pull_ids.is_empty() {
                                                    // 无需操作，直接标记完成
                                                    pending_msg_topics_task.mark_completed(topic_id, &sync_logger_task, &tx_internal, &handle_clone, true).await;
                                                    continue;
                                                }

                                                if to_push {
                                                    push_topic_ids.push(topic_id.clone());
                                                }
                                                if !to_pull_ids.is_empty() {
                                                    pull_batch.push((topic_id.clone(), to_pull_ids));
                                                } else if to_push && to_pull_ids.is_empty() {
                                                    // push-only: 无需 pull，push 后直接标记完成
                                                }
                                            }

                                            let has_push = !push_topic_ids.is_empty();
                                            let has_pull = !pull_batch.is_empty();

                                            if has_push || has_pull {
                                                let h_in = h.clone();
                                                let c_in = c.clone();
                                                let b_in = base.clone();
                                                let token = settings.sync_token.clone();
                                                let tracker = pending_msg_topics_task.clone();
                                                let tx_internal_msg = tx_internal.clone();
                                                let sync_logger_msg = sync_logger_task.clone();
                                                let wq_in = wq.clone();
                                                let sync_state = h.state::<SyncState>();
                                                let uploaded_hashes = sync_state.uploaded_hashes.clone();
                                                // 收集所有涉及的 topic ID（去重）
                                                let mut all_topic_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
                                                for tid in &push_topic_ids { all_topic_ids.insert(tid.clone()); }
                                                for (tid, _) in &pull_batch { all_topic_ids.insert(tid.clone()); }

                                                tauri::async_runtime::spawn(async move {
                                                    // 1. Push 批量（先执行，确保 push_pull 的 topic 推送完再拉取）
                                                    if has_push {
                                                        match PushExecutor::push_messages_batch(
                                                            &h_in, &c_in, &b_in, &token, &push_topic_ids, uploaded_hashes.clone(),
                                                        ).await {
                                                            Ok(results) => {
                                                                for r in &results {
                                                                    if r.success {
                                                                        tracker.mark_modified(&r.topic_id).await;
                                                                    } else {
                                                                        let err = r.error.as_deref().unwrap_or("unknown");
                                                                        if let Ok(mut logger) = sync_logger_msg.lock() {
                                                                            logger.log_operation("messages", "topic", &r.topic_id, false, Some(err));
                                                                        }
                                                                        emit_sync_log(&h_in, "error", &format!("Push failed for {}: {}", r.topic_id, err));
                                                                    }
                                                                }
                                                            }
                                                            Err(e) => {
                                                                let err_msg = format!("Batch push messages failed: {}", e);
                                                                if let Ok(mut logger) = sync_logger_msg.lock() {
                                                                    logger.log(LogLevel::Error, "messages", &err_msg);
                                                                }
                                                                emit_sync_log(&h_in, "error", &err_msg);
                                                            }
                                                        }
                                                    }

                                                    // 2. Pull 批量（push 完成后再 pull，确保 push_pull 的 topic 数据已合并）
                                                    if has_pull {
                                                        match PullExecutor::pull_messages_batch(
                                                            &h_in, &c_in, &b_in, &token, &pull_batch, &wq_in,
                                                        ).await {
                                                            Ok(results) => {
                                                                let result_map: std::collections::HashMap<&str, &BatchPullResult> =
                                                                    results.iter().map(|r| (r.topic_id.as_str(), r)).collect();
                                                                for (tid, _) in &pull_batch {
                                                                    if let Some(r) = result_map.get(tid.as_str()) {
                                                                        if r.success {
                                                                            tracker.mark_modified(tid).await;
                                                                        } else {
                                                                            let err = r.error.as_deref().unwrap_or("unknown");
                                                                            if let Ok(mut logger) = sync_logger_msg.lock() {
                                                                                logger.log_operation("messages", "topic", tid, false, Some(err));
                                                                            }
                                                                            emit_sync_log(&h_in, "error", &format!("Pull failed for {}: {}", tid, err));
                                                                        }
                                                                    } else {
                                                                        if let Ok(mut logger) = sync_logger_msg.lock() {
                                                                            logger.log_operation("messages", "topic", tid, false, Some("not in batch response"));
                                                                        }
                                                                    }
                                                                }
                                                            }
                                                            Err(e) => {
                                                                let err_msg = format!("Batch pull messages failed: {}", e);
                                                                if let Ok(mut logger) = sync_logger_msg.lock() {
                                                                    logger.log(LogLevel::Error, "messages", &err_msg);
                                                                }
                                                                emit_sync_log(&h_in, "error", &err_msg);
                                                            }
                                                        }
                                                    }

                                                    // 3. 所有 topic 标记完成
                                                    for tid in &all_topic_ids {
                                                        tracker.mark_completed(tid, &sync_logger_msg, &tx_internal_msg, &h_in, false).await;
                                                    }

                                                    println!("[SyncService] Phase 3 batch done: push={} pull={}", push_topic_ids.len(), pull_batch.len());
                                                });
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
                                    Some("SYNC_TOPIC_HASH_RESULTS") => {
                                        manifest_phase.store(3, Ordering::SeqCst); // 进入 Phase 2.5+，旧 Phase 2 看门狗失效
                                        if let Some(changed) = payload["changedTopics"].as_array() {
                                            let changed_ids: Vec<String> = changed.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect();
                                            println!("[SyncService] Phase 2.5 results: {} topics need message sync", changed_ids.len());
                                            {
                                                let mut guard = changed_topics.lock().await;
                                                *guard = changed_ids;
                                            }
                                            let _ = tx_internal.send(SyncCommand::StartMessages);
                                        }
                                    },
                                    Some("PHASE_MANIFESTS") => {
                                        // Topic 元数据已在 Phase 1 的 SYNC_DIFF_RESULTS 中处理完毕。
                                        // 桌面端在 PHASE_START metadata/topic 时仍可能返回 PHASE_MANIFESTS，此处安全忽略。
                                    },
                                    Some("PHASE_COMPLETED") => {
                                        manifest_phase.store(0, Ordering::SeqCst); // 同步完成，所有看门狗失效
                                        write_queue_task.flush().await;

                                        emit_sync_log(&handle_clone, "success", "同步已完成，所有数据已对齐");

                                        // 发送同步完成提示
                                        let _ = handle_clone.emit(
                                            "vcp-system-event",
                                            json!({
                                                "type": "vcp-log-message",
                                                "data": {
                                                    "id": "vcp_sync_connection_status",
                                                    "status": "success",
                                                    "tool_name": "Sync",
                                                    "content": "同步完成",
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
                                    },
                                    Some("SYNC_LOG_EVENT") => {
                                        let level = payload["level"].as_str().unwrap_or("info");
                                        let message = payload["message"].as_str().unwrap_or("");
                                        emit_sync_log(&handle_clone, level, &format!("[Desktop] {}", message));
                                    },
                                    Some("DESKTOP_PHASE_START") | Some("DESKTOP_PHASE_PROGRESS") | Some("DESKTOP_PHASE_COMPLETE") => {
                                        let phase = payload["phase"].as_str().unwrap_or("unknown");
                                        let msg = match payload["type"].as_str() {
                                            Some("DESKTOP_PHASE_START") => format!("[Desktop] Phase {} started", phase),
                                            Some("DESKTOP_PHASE_COMPLETE") => format!("[Desktop] Phase {} completed", phase),
                                            _ => format!("[Desktop] Phase {} in progress", phase),
                                        };
                                        emit_sync_log(&handle_clone, "info", &msg);
                                    },
                                        _ => {}
                                }
                            }
                                }
                                Some(Err(e)) => {
                                    let err_msg = format!("WebSocket 接收发生错误: {}", e);
                                    if let Ok(mut logger) = sync_logger_task.lock() {
                                        logger.log(LogLevel::Error, "network", &err_msg);
                                    }
                                    emit_sync_log(&handle_clone, "error", &err_msg);
                                    break;
                                }
                                None => {
                                    let err_msg = "WebSocket 连接意外断开 (服务器关闭连接)";
                                    if let Ok(mut logger) = sync_logger_task.lock() {
                                        logger.log(LogLevel::Error, "network", err_msg);
                                    }
                                    emit_sync_log(&handle_clone, "error", err_msg);
                                    break;
                                }
                            }
                        }
                        else => break,
                    }
                }
                if sync_success {
                    break; // 同步完成，退出外层 loop
                } else {
                    // 同步未成功完成，但内层循环已跳出（说明中途断网）
                    retry_count += 1;
                    if retry_count >= MAX_RETRIES {
                        let err_msg = "同步中途异常断开，已达到最大重试次数";
                        publish_sync_status(
                            &handle_clone,
                            &connection_status_for_task,
                            "error",
                            err_msg,
                        )
                        .await;
                        break;
                    }
                    let backoff = retry_delay * 2u32.pow(retry_count - 1);
                    let err_msg = format!(
                        "同步中途异常断开，{:?} 后尝试重新连接... (次数: {}/{})",
                        backoff, retry_count, MAX_RETRIES
                    );
                    emit_sync_log(&handle_clone, "warn", &err_msg);
                    tokio::time::sleep(backoff).await;
                    continue; // 重新尝试连接
                }
            }
            Err(e) => {
                let err_detail = e.to_string();
                retry_count += 1;
                if retry_count >= MAX_RETRIES {
                    let err_msg = format!("连接失败，已达到最大重试次数 | {}", err_detail);
                    publish_sync_status(
                        &handle_clone,
                        &connection_status_for_task,
                        "error",
                        &err_msg,
                    )
                    .await;
                    break;
                }
                let warn_msg = format!("连接失败，第 {} 次重试 | {}", retry_count, err_detail);
                emit_sync_log(&handle_clone, "warning", &warn_msg);
                tokio::time::sleep(retry_delay).await;
                retry_delay = (retry_delay * 2).min(Duration::from_secs(5));
            }
        }
    }
}

/// 每批最多包含的消息数，控制单次 WS payload 大小（约 10000 条消息 ≈ 1.5-2MB JSON）
const MAX_MESSAGES_PER_BATCH: usize = 10000;

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

fn emit_sync_log<R: Runtime>(app_handle: &AppHandle<R>, level: &str, message: &str) {
    let _ = app_handle.emit(
        "vcp-log",
        json!({
            "id": format!("{}_{}", level, chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)),
            "level": level,
            "category": "sync",
            "message": message,
        }),
    );
}

#[tauri::command]
pub async fn get_sync_status(state: State<'_, SyncState>) -> Result<String, String> {
    Ok(state.connection_status.read().await.clone())
}

#[tauri::command]
pub async fn start_manual_sync(
    handle: AppHandle,
    state: State<'_, SyncState>,
) -> Result<(), String> {
    if state
        .is_syncing
        .swap(true, std::sync::atomic::Ordering::SeqCst)
    {
        return Err("同步已在进行中".to_string());
    }

    let (tx, rx) = mpsc::unbounded_channel::<SyncCommand>();

    let app_handle = handle.clone();
    let connection_status = state.connection_status.clone();
    let is_syncing = state.is_syncing.clone();

    let tx_cmd = tx.clone();
    tauri::async_runtime::spawn(async move {
        run_sync_session(app_handle, tx, rx, connection_status).await;
        is_syncing.store(false, std::sync::atomic::Ordering::SeqCst);
    });

    tx_cmd
        .send(SyncCommand::StartManualSync)
        .map_err(|e| e.to_string())
}

#[derive(Debug, serde::Serialize)]
pub struct SyncLogFileInfo {
    pub filename: String,
    pub created_at: u64,
    pub size_bytes: u64,
}

#[tauri::command]
pub async fn list_sync_log_files(app: AppHandle) -> Result<Vec<SyncLogFileInfo>, String> {
    let log_dir = app
        .path()
        .app_log_dir()
        .map_err(|e| e.to_string())?
        .join("sync_logs");
    if !log_dir.exists() {
        return Ok(Vec::new());
    }

    let mut entries = Vec::new();
    let mut read_dir = tokio::fs::read_dir(&log_dir)
        .await
        .map_err(|e| e.to_string())?;
    while let Ok(Some(entry)) = read_dir.next_entry().await {
        let metadata = entry.metadata().await.map_err(|e| e.to_string())?;
        if metadata.is_file() {
            let filename = entry.file_name().to_string_lossy().to_string();
            let created_at = metadata
                .created()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            entries.push(SyncLogFileInfo {
                filename,
                created_at,
                size_bytes: metadata.len(),
            });
        }
    }

    entries.sort_by_key(|b| std::cmp::Reverse(b.created_at));
    Ok(entries)
}

#[tauri::command]
pub async fn get_sync_session_log_path(
    state: State<'_, SyncState>,
) -> Result<Option<String>, String> {
    let guard = state.current_log_path.read().await;
    Ok(guard.clone())
}

#[tauri::command]
pub async fn read_sync_log_file(app: AppHandle, filename: String) -> Result<String, String> {
    let log_dir = app
        .path()
        .app_log_dir()
        .map_err(|e| e.to_string())?
        .join("sync_logs");
    let file_path = log_dir.join(&filename);

    // 安全检查：确保文件在 sync_logs 目录内
    let canonical_dir = log_dir.canonicalize().map_err(|e| e.to_string())?;
    let canonical_file = file_path.canonicalize().map_err(|e| e.to_string())?;
    if !canonical_file.starts_with(&canonical_dir) {
        return Err("Invalid file path".to_string());
    }

    let content = tokio::fs::read_to_string(&canonical_file)
        .await
        .map_err(|e| e.to_string())?;
    Ok(content)
}

#[tauri::command]
pub async fn clear_old_sync_logs(app: AppHandle, keep_days: u32) -> Result<u32, String> {
    let log_dir = app
        .path()
        .app_log_dir()
        .map_err(|e| e.to_string())?
        .join("sync_logs");
    if !log_dir.exists() {
        return Ok(0);
    }

    let cutoff =
        std::time::SystemTime::now() - std::time::Duration::from_secs(keep_days as u64 * 86400);
    let mut removed = 0u32;

    let mut read_dir = tokio::fs::read_dir(&log_dir)
        .await
        .map_err(|e| e.to_string())?;
    while let Ok(Some(entry)) = read_dir.next_entry().await {
        let metadata = entry.metadata().await.map_err(|e| e.to_string())?;
        if metadata.is_file() {
            let modified = metadata
                .modified()
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            if modified < cutoff {
                let _ = tokio::fs::remove_file(entry.path()).await;
                removed += 1;
            }
        }
    }

    Ok(removed)
}

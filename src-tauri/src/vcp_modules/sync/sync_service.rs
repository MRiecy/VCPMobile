use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::db_write_queue::DbWriteQueue;
use crate::vcp_modules::sync_executor::PullExecutor;
use crate::vcp_modules::sync_hash::HashInitializer;
use crate::vcp_modules::sync_logger::{LogLevel, SyncLogger};
use crate::vcp_modules::sync_pipeline::{Phase1Metadata, Phase3Message, SyncPipeline};
use crate::vcp_modules::sync_types::SyncDataType;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, Runtime, State};
use tokio::sync::{mpsc, RwLock, Semaphore};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};

const EXPECTED_PLUGIN_VERSION: &str = "1.0.0";
const VERSION_CHECK_TIMEOUT: Duration = Duration::from_secs(5);

pub struct SyncState {
    pub ws_sender: mpsc::UnboundedSender<SyncCommand>,
    pub connection_status: Arc<RwLock<String>>,
    pub uploaded_hashes: Arc<RwLock<HashSet<String>>>,
    pub is_syncing: Arc<std::sync::atomic::AtomicBool>,
    pub current_log_path: Arc<RwLock<Option<String>>>,
    pub current_logger: Arc<std::sync::RwLock<Option<Arc<std::sync::Mutex<SyncLogger>>>>>,
}

/// 追踪 Phase 3 中已处理完成的 topic，替代 AtomicU32 避免双重递减下溢
pub struct Phase3Tracker {
    pub completed: tokio::sync::Mutex<HashSet<String>>,
    pub modified: tokio::sync::Mutex<HashSet<String>>,
    pub total: std::sync::atomic::AtomicUsize,
}

impl Phase3Tracker {
    /// 标记某个 topic 为数据已修改（实际发生了 pull/push）
    pub async fn mark_modified(&self, topic_id: &str) {
        let mut modified = self.modified.lock().await;
        modified.insert(topic_id.to_string());
    }

    /// 标记某个 topic 已完成。如果是首次标记，返回 true；否则返回 false。
    /// 当所有 topic 都完成时，触发 complete_phase 和 Phase3 命令。
    pub async fn mark_completed(
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
    SendWsMessage(serde_json::Value),
    Cancel,
}

pub fn parse_sync_data_type(value: &Value) -> Option<SyncDataType> {
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
        current_logger: Arc::new(std::sync::RwLock::new(None)),
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
    let sync_logger = Arc::new(std::sync::Mutex::new(SyncLogger::new_session(
        sync_log_level,
        log_dir,
        Some(app_handle.clone()),
    )));
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
        let mut logger_guard = sync_state.current_logger.write().unwrap();
        *logger_guard = Some(sync_logger.clone());
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
        if !handle_clone
            .state::<SyncState>()
            .is_syncing
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            break;
        }
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
                    logger.log(LogLevel::Info, "sync", "=== Phase 1: Owner Metadata ===");
                }
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
                                SyncCommand::Cancel => {
                                    let _ = ws_stream.close(None).await;
                                    break;
                                },
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
                                        // 1. 通知桌面端消息相位已完成
                                        let _ = ws_stream.send(Message::Text(json!({ "type": "PHASE_COMPLETED", "phase": "messages" }).to_string().into())).await;

                                        // 2. 调用优雅的 SyncFinalizer 执行收尾哈希冒泡与事务
                                        let db = handle_clone.state::<DbState>();
                                        let modified_topics = {
                                            let guard = pending_msg_topics_task.modified.lock().await;
                                            guard.clone()
                                        };
                                        if let Err(e) = crate::vcp_modules::sync::sync_finalize::SyncFinalizer::execute(
                                            &handle_clone,
                                            &db,
                                            &write_queue_task,
                                            &pipeline_task,
                                            &sync_logger_task,
                                            modified_topics,
                                        ).await {
                                            println!("[SyncService] SyncFinalizer failed: {}", e);
                                        }
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
                                SyncCommand::SendWsMessage(val) => {
                                    let _ = ws_stream.send(Message::Text(val.to_string().into())).await;
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
                                let settings = crate::vcp_modules::settings_manager::read_settings(h.clone(), h.state()).await.unwrap_or_default();

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
                                        let Some(data_type) = parse_sync_data_type(&payload["dataType"]) else { continue; };
                                        if let Err(e) = crate::vcp_modules::sync_executor::diff_handler::DiffHandler::handle_diff(
                                            &h,
                                            &payload,
                                            data_type,
                                            &c,
                                            &base,
                                            &settings.sync_token,
                                            &wq,
                                            &pending_tasks_task,
                                            &total_tasks_task,
                                            &manifest_responses_received,
                                            &expected_manifest_count,
                                            &manifest_phase,
                                            &tx_internal,
                                            &changed_owners,
                                            &sync_logger_task,
                                        ).await {
                                            println!("[SyncService] DiffHandler failed: {}", e);
                                        }
                                    },
                                    Some("SYNC_DIFF_RESULTS_BATCH") => {
                                        if let Err(e) = crate::vcp_modules::sync_executor::batch_diff_handler::BatchDiffHandler::handle_diff_batch(
                                            &h,
                                            &payload,
                                            &c,
                                            &base,
                                            &settings.sync_token,
                                            &pending_msg_topics_task,
                                            &tx_internal,
                                            &sync_logger_task,
                                            &wq,
                                            &pending_diff_batches,
                                            settings.sync_prerender_enabled,
                                        ).await {
                                            println!("[SyncService] BatchDiffHandler failed: {}", e);
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
                    if !handle_clone
                        .state::<SyncState>()
                        .is_syncing
                        .load(std::sync::atomic::Ordering::SeqCst)
                    {
                        break;
                    }
                    tokio::time::sleep(backoff).await;
                    if !handle_clone
                        .state::<SyncState>()
                        .is_syncing
                        .load(std::sync::atomic::Ordering::SeqCst)
                    {
                        break;
                    }
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
                if !handle_clone
                    .state::<SyncState>()
                    .is_syncing
                    .load(std::sync::atomic::Ordering::SeqCst)
                {
                    break;
                }
                tokio::time::sleep(retry_delay).await;
                if !handle_clone
                    .state::<SyncState>()
                    .is_syncing
                    .load(std::sync::atomic::Ordering::SeqCst)
                {
                    break;
                }
                retry_delay = (retry_delay * 2).min(Duration::from_secs(5));
            }
        }
    }

    // 同步会话结束，清空 current_logger
    {
        let sync_state = app_handle.state::<SyncState>();
        let mut logger_guard = sync_state.current_logger.write().unwrap();
        *logger_guard = None;
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

pub(crate) fn emit_sync_log<R: Runtime>(app_handle: &AppHandle<R>, level: &str, message: &str) {
    let _ = app_handle.emit(
        "vcp-log",
        serde_json::json!({
            "id": format!("{}_{}", level, chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)),
            "level": level,
            "category": "sync",
            "message": message,
        }),
    );

    // 整合：写入 log 文件和控制台！
    let sync_state = app_handle.state::<SyncState>();
    if let Some(logger_arc) = sync_state
        .current_logger
        .read()
        .ok()
        .and_then(|guard| guard.clone())
    {
        if let Ok(mut logger) = logger_arc.lock() {
            let log_level = match level {
                "error" => LogLevel::Error,
                "warn" | "warning" => LogLevel::Info,
                _ => LogLevel::Info,
            };
            logger.log_direct(log_level, "sync", message);
        }
    } else {
        println!("[Sync] [{}] {}", level, message);
    }
}

#[tauri::command]
pub async fn stop_sync(state: State<'_, SyncState>) -> Result<(), String> {
    state
        .is_syncing
        .store(false, std::sync::atomic::Ordering::SeqCst);
    {
        let mut guard = state.connection_status.write().await;
        *guard = "disconnected".to_string();
    }
    let _ = state.ws_sender.send(SyncCommand::Cancel);
    Ok(())
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
                .or_else(|_| metadata.modified())
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

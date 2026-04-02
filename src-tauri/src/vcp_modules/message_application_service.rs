use crate::vcp_modules::chat_manager::ChatMessage;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::file_watcher::WatcherState;
use crate::vcp_modules::history_repository_fs;
use crate::vcp_modules::message_asset_rebaser;
use crate::vcp_modules::path_topology_service::resolve_history_path;
use crate::vcp_modules::topic_projection_service::refresh_topic_projection_from_history;
use dashmap::DashMap;
use lazy_static::lazy_static;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::AppHandle;

lazy_static! {
    /// 历史记录写入锁，防止并行追加时的竞态
    static ref HISTORY_WRITE_LOCKS: DashMap<String, Arc<tokio::sync::Mutex<()>>> = DashMap::new();
}

/// 内部辅助函数：标记一次内部保存操作
fn signal_internal_save_raw(state: &WatcherState) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    state.last_internal_save_time.store(now, Ordering::SeqCst);
    println!("[Watcher] Internal save signaled at {}", now);
}

/// 加载聊天历史记录的内部逻辑
pub async fn load_chat_history_internal(
    app_handle: &AppHandle,
    item_id: &str,
    topic_id: &str,
    limit: Option<usize>,
    offset: Option<usize>,
) -> Result<Vec<ChatMessage>, String> {
    let history_path = resolve_history_path(app_handle, item_id, topic_id);

    let full_history = history_repository_fs::read_history(&history_path)?;

    let total_len = full_history.len();
    let end = total_len.saturating_sub(offset.unwrap_or(0));
    let start = end.saturating_sub(limit.unwrap_or(total_len));

    let mut history: Vec<ChatMessage> = full_history
        .into_iter()
        .skip(start)
        .take(end - start)
        .collect();

    // 动态替换桌面端的绝对路径为手机端的绝对路径 (Path Rebasing)
    message_asset_rebaser::rebase_message_assets(app_handle, item_id, &mut history)?;

    Ok(history)
}

/// 保存聊天历史记录的内部逻辑
pub async fn save_chat_history_internal(
    app_handle: &AppHandle,
    db_state: &DbState,
    watcher_state: &WatcherState,
    item_id: &str,
    topic_id: &str,
    history: Vec<ChatMessage>,
) -> Result<(), String> {
    signal_internal_save_raw(watcher_state);

    let history_path = resolve_history_path(app_handle, item_id, topic_id);
    history_repository_fs::write_history(&history_path, &history)?;

    refresh_topic_projection_from_history(
        app_handle,
        &db_state.pool,
        item_id,
        topic_id,
        &history_path,
        &history,
    )
    .await
}

/// 线程安全地向历史记录追加单条消息 (用于并行群聊断点存盘)
pub async fn append_single_message(
    app_handle: AppHandle,
    db_pool: &sqlx::Pool<sqlx::Sqlite>,
    watcher_state: Option<&WatcherState>,
    item_id: String,
    topic_id: String,
    message: ChatMessage,
) -> Result<(), String> {
    let lock_key = format!("{}_{}", item_id, topic_id);
    let lock = HISTORY_WRITE_LOCKS
        .entry(lock_key.clone())
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone();

    let _guard = lock.lock().await;

    // 1. 加载当前全量历史
    let mut history =
        load_chat_history_internal(&app_handle, &item_id, &topic_id, None, None).await?;

    // 检查是否已存在 (防止重复追加)
    if history.iter().any(|m| m.id == message.id) {
        return Ok(());
    }

    history.push(message);

    if let Some(state) = watcher_state {
        signal_internal_save_raw(state);
    }

    let history_path = resolve_history_path(&app_handle, &item_id, &topic_id);
    history_repository_fs::write_history(&history_path, &history)?;

    refresh_topic_projection_from_history(
        &app_handle,
        db_pool,
        &item_id,
        &topic_id,
        &history_path,
        &history,
    )
    .await
}

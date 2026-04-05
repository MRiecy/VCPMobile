use crate::vcp_modules::chat_manager::ChatMessage;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::file_watcher::WatcherState;
use crate::vcp_modules::message_asset_rebaser;
use crate::vcp_modules::path_topology_service::{resolve_jsonl_path, resolve_astbin_path};
use crate::vcp_modules::topic_projection_service::refresh_topic_projection_from_history;
use crate::vcp_modules::message_log_store::MessageLogStore;
use crate::vcp_modules::message_render_compiler::MessageRenderCompiler;
use crate::vcp_modules::message_repository_db::MessageRepositoryDb;
use crate::vcp_modules::emoticon_manager::EmoticonManagerState;
use tauri::{AppHandle, Manager};

/// 内部辅助函数：标记一次内部保存操作
fn signal_internal_save_raw(_state: &WatcherState) {
    // No-op: we no longer need to signal internal saves for history.json
}

/// 加载聊天历史记录的内部逻辑
pub async fn load_chat_history_internal(
    app_handle: &AppHandle,
    item_id: &str,
    topic_id: &str,
    limit: Option<usize>,
    offset: Option<usize>,
) -> Result<Vec<ChatMessage>, String> {
    let db_state = app_handle.state::<DbState>();
    let pool = &db_state.pool;

    let limit_val = limit.unwrap_or(20) as i32;
    let offset_val = offset.unwrap_or(0) as i32;

    // Phase 2: Redirect this to DB + JSONL
    // Query message indices from SQLite
    let pointers_res: Result<Vec<sqlx::sqlite::SqliteRow>, sqlx::Error> = sqlx::query(
            "SELECT raw_byte_offset, raw_byte_length 
            FROM message_index 
            WHERE topic_id = ? AND is_deleted = 0 
            ORDER BY created_at DESC 
            LIMIT ? OFFSET ?"
        )
        .bind(topic_id)
        .bind(limit_val)
        .bind(offset_val)
        .fetch_all(pool)
        .await;

    let mut history = match pointers_res {
        Ok(mut rows) => {
            // Reverse to get ASC order (chronological)
            rows.reverse();

            let jsonl_path = resolve_jsonl_path(app_handle, item_id, topic_id);
            if !jsonl_path.exists() {
                return Ok(vec![]);
            }

            let log_store = MessageLogStore::new(jsonl_path);
            let mut messages = Vec::with_capacity(rows.len());

            for row in rows {
                use sqlx::Row;
                let r_offset: i64 = row.get("raw_byte_offset");
                let r_length: i64 = row.get("raw_byte_length");

                match log_store.read_jsonl_at(r_offset as u64, r_length as u64) {
                    Ok(json_line) => {
                        if let Ok(msg) = serde_json::from_str::<ChatMessage>(&json_line) {
                            messages.push(msg);
                        }
                    }
                    Err(e) => eprintln!("[VCPCore] JSONL read error: {}", e),
                }
            }
            messages
        }
        Err(e) => {
            eprintln!("[VCPCore] DB query error in load_chat_history_internal: {}", e);
            vec![]
        }
    };

    // 动态替换桌面端的绝对路径为手机端的绝对路径 (Path Rebasing)
    message_asset_rebaser::rebase_message_assets(app_handle, item_id, &mut history)?;

    Ok(history)
}

/// 保存聊天历史记录的内部逻辑
pub async fn save_chat_history_internal(
    app_handle: &AppHandle,
    db_state: &DbState,
    _watcher_state: &WatcherState,
    item_id: &str,
    topic_id: &str,
    history: Vec<ChatMessage>,
) -> Result<(), String> {
    // Leviathan Phase 1: Authoritative rebuild of the new core state
    rebuild_topic_core_from_history(
        app_handle,
        &db_state.pool,
        item_id,
        topic_id,
        &history,
    ).await?;

    refresh_topic_projection_from_history(
        app_handle,
        &db_state.pool,
        item_id,
        topic_id,
        &history,
    )
    .await
}

/// 权威重建 Topic 的新内核状态 (JSONL, ASTBIN, DB Indices)
pub async fn rebuild_topic_core_from_history(
    app_handle: &AppHandle,
    db_pool: &sqlx::Pool<sqlx::Sqlite>,
    item_id: &str,
    topic_id: &str,
    history: &[ChatMessage],
) -> Result<(), String> {
    // ... 原有逻辑保持不变 ...
    let jsonl_path = resolve_jsonl_path(app_handle, item_id, topic_id);
    let astbin_path = resolve_astbin_path(app_handle, item_id, topic_id);
    let log_store = MessageLogStore::new(jsonl_path);
    let astbin_store = MessageLogStore::new(astbin_path);

    log_store.truncate()?;
    astbin_store.truncate()?;
    MessageRepositoryDb::clear_topic_data(db_pool, topic_id).await?;

    let emoticon_state = app_handle.state::<EmoticonManagerState>();
    let library = emoticon_state.library.lock().await;

    let mut last_timestamp = 0;
    for msg in history {
        let msg_json = serde_json::to_string(msg).map_err(|e| e.to_string())?;
        let (raw_offset, raw_length) = log_store.append_jsonl(&msg_json)?;
        let blocks = MessageRenderCompiler::compile(&msg.content, &library);
        let render_bytes = MessageRenderCompiler::serialize(&blocks)?;
        let (render_offset, render_length) = astbin_store.append_astbin(&render_bytes)?;

        MessageRepositoryDb::upsert_message_index(
            db_pool,
            msg,
            topic_id,
            item_id,
            raw_offset,
            raw_length,
            render_offset,
            render_length
        ).await?;
        last_timestamp = msg.timestamp as i64;
    }

    MessageRepositoryDb::rebuild_topic_state(
        db_pool,
        topic_id,
        item_id,
        history.len() as i32,
        last_timestamp,
    ).await?;

    Ok(())
}

/// 增量应用来自同步端的变动 (Added, Updated, Deleted)
pub async fn apply_sync_delta(
    app_handle: &AppHandle,
    db_pool: &sqlx::Pool<sqlx::Sqlite>,
    item_id: &str,
    topic_id: &str,
    added: Vec<ChatMessage>,
    updated: Vec<ChatMessage>,
    deleted_ids: Vec<String>,
) -> Result<(), String> {
    let jsonl_path = resolve_jsonl_path(app_handle, item_id, topic_id);
    let astbin_path = resolve_astbin_path(app_handle, item_id, topic_id);
    let log_store = MessageLogStore::new(jsonl_path);
    let astbin_store = MessageLogStore::new(astbin_path);

    let emoticon_state = app_handle.state::<EmoticonManagerState>();
    let library = emoticon_state.library.lock().await;

    let mut tx = db_pool.begin().await.map_err(|e| e.to_string())?;

    // 1. 处理新增和更新 (对于追加式存储，Updated 本质上也是 Append + Pointer Update)
    let mut all_to_upsert = added;
    all_to_upsert.extend(updated);

    let mut last_timestamp = 0;
    for msg in &all_to_upsert {
        let msg_json = serde_json::to_string(msg).map_err(|e| e.to_string())?;
        let (raw_offset, raw_length) = log_store.append_jsonl(&msg_json)?;
        
        let blocks = MessageRenderCompiler::compile(&msg.content, &library);
        let render_bytes = MessageRenderCompiler::serialize(&blocks)?;
        let (render_offset, render_length) = astbin_store.append_astbin(&render_bytes)?;

        MessageRepositoryDb::upsert_message_index_tx(
            &mut tx,
            msg,
            topic_id,
            item_id,
            raw_offset,
            raw_length,
            render_offset,
            render_length
        ).await?;
        
        if (msg.timestamp as i64) > last_timestamp {
            last_timestamp = msg.timestamp as i64;
        }
    }

    // 2. 处理删除
    if !deleted_ids.is_empty() {
        let delete_query = format!(
            "UPDATE message_index SET is_deleted = 1 WHERE topic_id = ? AND msg_id IN ({})",
            deleted_ids.iter().map(|_| "?").collect::<Vec<_>>().join(", ")
        );
        let mut q = sqlx::query(&delete_query).bind(topic_id);
        for id in &deleted_ids {
            q = q.bind(id);
        }
        q.execute(&mut *tx).await.map_err(|e| e.to_string())?;
    }

    // 3. 同步更新 Topic 状态
    let msg_count: i32 = sqlx::query_scalar("SELECT COUNT(*) FROM message_index WHERE topic_id = ? AND is_deleted = 0")
        .bind(topic_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

    if last_timestamp > 0 {
        sqlx::query("UPDATE topic_state SET msg_count = ?, updated_at = ?, revision = revision + 1 WHERE topic_id = ?")
            .bind(msg_count)
            .bind(last_timestamp)
            .bind(topic_id)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
    } else {
        sqlx::query("UPDATE topic_state SET msg_count = ?, revision = revision + 1 WHERE topic_id = ?")
            .bind(msg_count)
            .bind(topic_id)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
    }

    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(())
}

/// 线程安全地向历史记录追加单条消息 (用于并行群聊断点存盘)
pub async fn append_single_message(
    app_handle: AppHandle,
    db_pool: &sqlx::Pool<sqlx::Sqlite>,
    _watcher_state: Option<&WatcherState>,
    item_id: String,
    topic_id: String,
    message: ChatMessage,
) -> Result<(), String> {
    // 1. Append to JSONL
    let jsonl_path = resolve_jsonl_path(&app_handle, &item_id, &topic_id);
    let log_store = MessageLogStore::new(jsonl_path);
    let msg_json = serde_json::to_string(&message).map_err(|e| e.to_string())?;
    let (raw_offset, raw_length) = log_store.append_jsonl(&msg_json)?;

    // 2. Compile AOT and Append to ASTBIN
    let astbin_path = resolve_astbin_path(&app_handle, &item_id, &topic_id);
    let astbin_store = MessageLogStore::new(astbin_path);
    
    let emoticon_state = app_handle.state::<EmoticonManagerState>();
    let library = emoticon_state.library.lock().await;
    let blocks = MessageRenderCompiler::compile(&message.content, &library);
    let render_bytes = MessageRenderCompiler::serialize(&blocks)?;
    let (render_offset, render_length) = astbin_store.append_astbin(&render_bytes)?;

    // 3. Update DB (Atomic-ish)
    let mut tx = db_pool.begin().await.map_err(|e| e.to_string())?;

    // INSERT OR REPLACE 会自动处理 msg_id 冲突，实现指针跳转
    MessageRepositoryDb::upsert_message_index_tx(
        &mut tx,
        &message,
        &topic_id,
        &item_id,
        raw_offset,
        raw_length,
        render_offset,
        render_length
    ).await?;

    // 更新 Topic 状态 (时间戳、预览、修订版本)
    // 注意：msg_count 现在应该基于数据库实际存在的非逻辑删除消息数
    let msg_count: i32 = sqlx::query_scalar("SELECT COUNT(*) FROM message_index WHERE topic_id = ? AND is_deleted = 0")
        .bind(&topic_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

    sqlx::query(
        "UPDATE topic_state SET 
            updated_at = ?, 
            revision = revision + 1,
            msg_count = ?
         WHERE topic_id = ?"
    )
    .bind(message.timestamp as i64)
    .bind(msg_count)
    .bind(&topic_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;

    tx.commit().await.map_err(|e| e.to_string())?;

    // 6. Refresh projection (legacy) - but use the new core for authoritative refresh
    // Note: We still refresh from legacy path for now to maintain topic_state parity
    let history = load_chat_history_internal(&app_handle, &item_id, &topic_id, None, None).await?;
    refresh_topic_projection_from_history(
        &app_handle,
        db_pool,
        &item_id,
        &topic_id,
        &history,
    )
    .await
}

/// 增量更新单条消息内容 (墓碑模式：追加新数据并更新索引指针)
pub async fn patch_single_message(
    app_handle: AppHandle,
    db_pool: &sqlx::Pool<sqlx::Sqlite>,
    item_id: String,
    topic_id: String,
    message: ChatMessage,
) -> Result<(), String> {
    // 1. 追加到 JSONL (新版本文本)
    let jsonl_path = resolve_jsonl_path(&app_handle, &item_id, &topic_id);
    let log_store = MessageLogStore::new(jsonl_path);
    let msg_json = serde_json::to_string(&message).map_err(|e| e.to_string())?;
    let (raw_offset, raw_length) = log_store.append_jsonl(&msg_json)?;

    // 2. 重新编译 AOT 并追加到 ASTBIN
    let astbin_path = resolve_astbin_path(&app_handle, &item_id, &topic_id);
    let astbin_store = MessageLogStore::new(astbin_path);
    
    let emoticon_state = app_handle.state::<EmoticonManagerState>();
    let library = emoticon_state.library.lock().await;
    let blocks = MessageRenderCompiler::compile(&message.content, &library);
    let render_bytes = MessageRenderCompiler::serialize(&blocks)?;
    let (render_offset, render_length) = astbin_store.append_astbin(&render_bytes)?;

    // 3. 更新数据库索引 (核心：INSERT OR REPLACE 将旧指针覆盖为新指针)
    MessageRepositoryDb::upsert_message_index(
        db_pool,
        &message,
        &topic_id,
        &item_id,
        raw_offset,
        raw_length,
        render_offset,
        render_length
    ).await?;

    Ok(())
}

/// 逻辑删除话题内的多条消息，并同步更新话题状态
pub async fn delete_messages(
    db_pool: &sqlx::Pool<sqlx::Sqlite>,
    topic_id: &str,
    msg_ids: Vec<String>,
) -> Result<(), String> {
    if msg_ids.is_empty() {
        return Ok(());
    }

    let mut tx = db_pool.begin().await.map_err(|e| e.to_string())?;

    // 1. 执行逻辑删除
    let delete_query = format!(
        "UPDATE message_index SET is_deleted = 1 WHERE topic_id = ? AND msg_id IN ({})",
        msg_ids.iter().map(|_| "?").collect::<Vec<_>>().join(", ")
    );
    let mut q = sqlx::query(&delete_query).bind(topic_id);
    for id in &msg_ids {
        q = q.bind(id);
    }
    q.execute(&mut *tx).await.map_err(|e| e.to_string())?;

    // 2. 重新计算话题状态
    let msg_count: i32 = sqlx::query_scalar("SELECT COUNT(*) FROM message_index WHERE topic_id = ? AND is_deleted = 0")
        .bind(topic_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

    sqlx::query(
        "UPDATE topic_state SET msg_count = ?, updated_at = ? WHERE topic_id = ?"
    )
    .bind(msg_count)
    .bind(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64)
    .bind(topic_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;

    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(())
}

/// 物理截断历史记录 (用于重新回复/时光倒流)
/// 它会删除指定时间戳之后的所有索引，并尝试物理截断文件以节省空间
pub async fn truncate_history_after_timestamp(
    app_handle: AppHandle,
    db_pool: &sqlx::Pool<sqlx::Sqlite>,
    item_id: &str,
    topic_id: &str,
    timestamp: i64,
) -> Result<(), String> {
    // 1. 查找截断点：找到该时间戳之后的第一条消息的 offset
    let first_msg_after: Option<(i64, i64)> = sqlx::query_as(
        "SELECT raw_byte_offset, render_byte_offset FROM message_index 
         WHERE topic_id = ? AND created_at > ? 
         ORDER BY created_at ASC LIMIT 1"
    )
    .bind(topic_id)
    .bind(timestamp)
    .fetch_optional(db_pool)
    .await
    .map_err(|e| e.to_string())?;

    if let Some((raw_offset, render_offset)) = first_msg_after {
        // 2. 物理截断文件 (瞬间完成 O(1))
        let jsonl_path = resolve_jsonl_path(&app_handle, item_id, topic_id);
        let astbin_path = resolve_astbin_path(&app_handle, item_id, topic_id);

        if jsonl_path.exists() {
            let f = std::fs::OpenOptions::new().write(true).open(jsonl_path).map_err(|e| e.to_string())?;
            f.set_len(raw_offset as u64).map_err(|e| e.to_string())?;
        }
        if astbin_path.exists() {
            let f = std::fs::OpenOptions::new().write(true).open(astbin_path).map_err(|e| e.to_string())?;
            f.set_len(render_offset as u64).map_err(|e| e.to_string())?;
        }
    }

    // 3. 删除数据库索引
    sqlx::query("DELETE FROM message_attachment_ref WHERE msg_id IN (SELECT msg_id FROM message_index WHERE topic_id = ? AND created_at > ?)")
        .bind(topic_id)
        .bind(timestamp)
        .execute(db_pool)
        .await
        .map_err(|e| e.to_string())?;

    sqlx::query("DELETE FROM message_index WHERE topic_id = ? AND created_at > ?")
        .bind(topic_id)
        .bind(timestamp)
        .execute(db_pool)
        .await
        .map_err(|e| e.to_string())?;

    Ok(())
}


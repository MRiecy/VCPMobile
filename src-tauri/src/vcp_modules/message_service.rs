use crate::vcp_modules::chat_manager::ChatMessage;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::emoticon_manager::EmoticonManagerState;
use crate::vcp_modules::message_asset_rebaser;
use crate::vcp_modules::message_render_compiler::MessageRenderCompiler;
use crate::vcp_modules::message_repository::MessageRepository;
use tauri::{AppHandle, Manager};

/// 加载聊天历史记录的内部逻辑
pub async fn load_chat_history_internal(
    app_handle: &AppHandle,
    owner_id: &str,
    owner_type: &str,
    topic_id: &str,
    limit: Option<usize>,
    offset: Option<usize>,
) -> Result<Vec<ChatMessage>, String> {
    let db_state = app_handle.state::<DbState>();
    let pool = &db_state.pool;

    let limit_val = limit.map(|v| v as i32).unwrap_or(20);
    let offset_val = offset.map(|v| v as i32).unwrap_or(0);

    // Direct read from SQLite messages table
    let rows: Vec<sqlx::sqlite::SqliteRow> = sqlx::query(
        "SELECT msg_id, role, name, content, timestamp, is_thinking, extra_json 
            FROM messages 
            WHERE topic_id = ? AND deleted_at IS NULL 
            ORDER BY timestamp DESC, msg_id DESC
            LIMIT ? OFFSET ?",
    )
    .bind(topic_id)
    .bind(limit_val)
    .bind(offset_val)
    .fetch_all(pool)
    .await
    .map_err(|e| e.to_string())?;

    let mut history = Vec::with_capacity(rows.len());
    for row in rows {
        use sqlx::Row;
        let id: String = row.get("msg_id");
        let role: String = row.get("role");
        let name: Option<String> = row.get("name");
        let content: String = row.get("content");
        let timestamp: i64 = row.get("timestamp");
        let is_thinking: Option<bool> = row.get("is_thinking");
        let extra_json: Option<String> = row.get("extra_json");

        let extra: serde_json::Value = if let Some(ej) = extra_json {
            serde_json::from_str(&ej).unwrap_or(serde_json::Value::Object(serde_json::Map::new()))
        } else {
            serde_json::Value::Object(serde_json::Map::new())
        };

        // Query attachments for this message
        let att_rows: Vec<sqlx::sqlite::SqliteRow> = sqlx::query(
            "SELECT a.attachment_hash, a.name, a.mime_type, a.size, a.extracted_text, a.thumbnail_path, a.src
             FROM attachments a
             JOIN message_attachments ma ON a.attachment_hash = ma.attachment_hash
             WHERE ma.msg_id = ?
             ORDER BY ma.attachment_order ASC"
        )
        .bind(&id)
        .fetch_all(pool)
        .await
        .map_err(|e| e.to_string())?;

        let attachments = if att_rows.is_empty() {
            None
        } else {
            let mut atts = Vec::with_capacity(att_rows.len());
            for ar in att_rows {
                atts.push(crate::vcp_modules::chat_manager::Attachment {
                    r#type: ar.get::<Option<String>, _>("mime_type").unwrap_or_default(),
                    src: ar.get::<Option<String>, _>("src").unwrap_or_default(),
                    name: ar.get::<Option<String>, _>("name").unwrap_or_default(),
                    size: ar.get::<i64, _>("size") as u64,
                    hash: Some(ar.get("attachment_hash")),
                    extracted_text: ar.get("extracted_text"),
                    thumbnail_path: ar.get("thumbnail_path"),
                });
            }
            Some(atts)
        };

        history.push(ChatMessage {
            id,
            role,
            name,
            content,
            timestamp: timestamp as u64,
            is_thinking,
            attachments,
            extra: serde_json::Value::Object(extra.as_object().cloned().unwrap_or_default()),
        });
    }

    // Reverse to chronological order as frontend expects
    history.reverse();
    // 动态替换桌面端的绝对路径为手机端的绝对路径 (Path Rebasing)
    message_asset_rebaser::rebase_message_assets(app_handle, owner_id, owner_type, &mut history)?;

    Ok(history)
}

/// 保存聊天历史记录的内部逻辑 (全量模式 - 仅限迁移或同步，由于历史原因保留但建议减少使用)
pub async fn save_chat_history_internal(
    _app_handle: &AppHandle,
    db_state: &DbState,
    _owner_id: &str,
    _owner_type: &str,
    topic_id: &str,
    history: &[ChatMessage],
) -> Result<(), String> {
    // 弃用全量删除再重建的逻辑。改为高效增量更新 (Upsert 模式)
    let mut tx = db_state.pool.begin().await.map_err(|e| e.to_string())?;

    let emoticon_state = _app_handle.state::<EmoticonManagerState>();
    let library = emoticon_state.library.lock().await;

    let mut last_timestamp = 0;
    for msg in history {
        let blocks = MessageRenderCompiler::compile(&msg.content, &library);
        let render_bytes = MessageRenderCompiler::serialize(&blocks)?;

        MessageRepository::upsert_message(&mut tx, msg, topic_id, "astbin", &render_bytes).await?;
        last_timestamp = msg.timestamp as i64;
    }

    let msg_count = history.len() as i32;
    sqlx::query(
        "UPDATE topics SET 
            updated_at = ?, 
            revision = revision + 1,
            msg_count = ?
         WHERE topic_id = ?",
    )
    .bind(last_timestamp)
    .bind(msg_count)
    .bind(topic_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;

    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(())
}

/// 物理删除指定时间戳之后的所有消息
pub async fn truncate_history_after_timestamp(
    _app_handle: AppHandle,
    db_pool: &sqlx::Pool<sqlx::Sqlite>,
    _owner_id: &str,
    _owner_type: &str,
    topic_id: &str,
    timestamp: i64,
) -> Result<(), String> {
    let mut tx = db_pool.begin().await.map_err(|e| e.to_string())?;

    sqlx::query("DELETE FROM message_attachments WHERE msg_id IN (SELECT msg_id FROM messages WHERE topic_id = ? AND timestamp > ?)")
        .bind(topic_id)
        .bind(timestamp)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

    sqlx::query("DELETE FROM messages WHERE topic_id = ? AND timestamp > ?")
        .bind(topic_id)
        .bind(timestamp)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

    let msg_count: i32 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM messages WHERE topic_id = ? AND deleted_at IS NULL",
    )
    .bind(topic_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;

    sqlx::query(
        "UPDATE topics SET msg_count = ?, updated_at = ?, revision = revision + 1 WHERE topic_id = ?"
    )
    .bind(msg_count)
    .bind(timestamp)
    .bind(topic_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;

    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(())
}

/// 线程安全地向历史记录追加单条消息
pub async fn append_single_message(
    app_handle: AppHandle,
    db_pool: &sqlx::Pool<sqlx::Sqlite>,
    _owner_id: &str,
    _owner_type: &str,
    topic_id: String,
    message: ChatMessage,
) -> Result<(), String> {
    let emoticon_state = app_handle.state::<EmoticonManagerState>();
    let library = emoticon_state.library.lock().await;
    let blocks = MessageRenderCompiler::compile(&message.content, &library);
    let render_bytes = MessageRenderCompiler::serialize(&blocks)?;

    let mut tx = db_pool.begin().await.map_err(|e| e.to_string())?;

    MessageRepository::upsert_message(&mut tx, &message, &topic_id, "astbin", &render_bytes)
        .await?;

    let msg_count: i32 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM messages WHERE topic_id = ? AND deleted_at IS NULL",
    )
    .bind(&topic_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;

    sqlx::query(
        "UPDATE topics SET 
            updated_at = ?, 
            revision = revision + 1,
            msg_count = ?
         WHERE topic_id = ?",
    )
    .bind(message.timestamp as i64)
    .bind(msg_count)
    .bind(&topic_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;

    tx.commit().await.map_err(|e| e.to_string())?;

    Ok(())
}

/// 增量更新单条消息内容
pub async fn patch_single_message(
    app_handle: AppHandle,
    db_pool: &sqlx::Pool<sqlx::Sqlite>,
    _owner_id: &str,
    _owner_type: &str,
    topic_id: String,
    message: ChatMessage,
) -> Result<(), String> {
    let emoticon_state = app_handle.state::<EmoticonManagerState>();
    let library = emoticon_state.library.lock().await;
    let blocks = MessageRenderCompiler::compile(&message.content, &library);
    let render_bytes = MessageRenderCompiler::serialize(&blocks)?;

    let mut tx = db_pool.begin().await.map_err(|e| e.to_string())?;
    MessageRepository::upsert_message(&mut tx, &message, &topic_id, "astbin", &render_bytes)
        .await?;

    // Patch 也要更新 topic 的 revision 和 updated_at，因为内容变了
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    sqlx::query(
        "UPDATE topics SET 
            updated_at = ?, 
            revision = revision + 1
         WHERE topic_id = ?",
    )
    .bind(now)
    .bind(&topic_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;

    tx.commit().await.map_err(|e| e.to_string())?;

    Ok(())
}

/// 逻辑删除话题内的多条消息
pub async fn delete_messages(
    db_pool: &sqlx::Pool<sqlx::Sqlite>,
    topic_id: &str,
    msg_ids: Vec<String>,
) -> Result<(), String> {
    if msg_ids.is_empty() {
        return Ok(());
    }

    let mut tx = db_pool.begin().await.map_err(|e| e.to_string())?;

    let delete_query = format!(
        "UPDATE messages SET deleted_at = ? WHERE topic_id = ? AND msg_id IN ({})",
        msg_ids.iter().map(|_| "?").collect::<Vec<_>>().join(", ")
    );
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let mut q = sqlx::query(&delete_query).bind(now).bind(topic_id);
    for id in &msg_ids {
        q = q.bind(id);
    }
    q.execute(&mut *tx).await.map_err(|e| e.to_string())?;

    let msg_count: i32 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM messages WHERE topic_id = ? AND deleted_at IS NULL",
    )
    .bind(topic_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;

    sqlx::query(
        "UPDATE topics SET msg_count = ?, updated_at = ?, revision = revision + 1 WHERE topic_id = ?"
    )
    .bind(msg_count)
    .bind(now)
    .bind(topic_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;
    tx.commit().await.map_err(|e| e.to_string())?;

    Ok(())
}

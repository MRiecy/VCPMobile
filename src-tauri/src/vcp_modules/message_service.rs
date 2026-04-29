use crate::vcp_modules::chat_manager::{Attachment, ChatMessage};
use crate::vcp_modules::message_render_compiler::MessageRenderCompiler;
use crate::vcp_modules::message_repository::MessageRepository;
use crate::vcp_modules::settings_manager;
use sqlx::Row;
use std::path::Path;
use tauri::{AppHandle, Manager};
use tokio::fs;

/// =================================================================
/// vcp_modules/message_service.rs - 消息业务逻辑中心 (含附件对齐)
/// =================================================================
pub async fn load_chat_history_internal(
    _app_handle: &AppHandle,
    _owner_id: &str,
    _owner_type: &str,
    topic_id: &str,
    limit: Option<usize>,
    offset: Option<usize>,
) -> Result<Vec<ChatMessage>, String> {
    let db_state = _app_handle.state::<crate::vcp_modules::db_manager::DbState>();
    let pool = &db_state.pool;

    let limit = limit.unwrap_or(9999);
    let offset = offset.unwrap_or(0);

    let rows = sqlx::query(
        "SELECT msg_id, role, name, agent_id, content, timestamp, is_thinking, is_group_message, group_id, finish_reason, render_content 
         FROM messages 
         WHERE topic_id = ? AND deleted_at IS NULL 
         ORDER BY timestamp DESC, rowid DESC 
         LIMIT ? OFFSET ?",
    )
    .bind(topic_id)
    .bind(limit as i64)
    .bind(offset as i64)
    .fetch_all(pool)
    .await
    .map_err(|e| e.to_string())?;

    // 收集所有 msg_id，用于批量查询附件
    let mut msg_ids = Vec::new();
    for row in &rows {
        use sqlx::Row;
        let msg_id: String = row.get("msg_id");
        msg_ids.push(msg_id);
    }

    // 批量查询所有附件（利用 message_attachments 索引表）
    let mut att_map: std::collections::HashMap<String, Vec<Attachment>> =
        std::collections::HashMap::new();
    if !msg_ids.is_empty() {
        let placeholders = msg_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let att_query = format!(
            "SELECT a.hash, a.mime_type, a.size, a.internal_path, a.extracted_text, a.image_frames, a.thumbnail_path, a.created_at,
                    ma.msg_id, ma.display_name, ma.src, ma.status
             FROM message_attachments ma
             JOIN attachments a ON ma.hash = a.hash
             WHERE ma.msg_id IN ({}) 
             ORDER BY ma.msg_id, ma.attachment_order ASC",
            placeholders
        );
        let mut q = sqlx::query(&att_query);
        for id in &msg_ids {
            q = q.bind(id);
        }
        let att_rows = q.fetch_all(pool).await.map_err(|e| e.to_string())?;

        for ar in att_rows {
            let msg_id: String = ar.get("msg_id");
            let hash: String = ar.get("hash");
            let mime_type: String = ar.get("mime_type");
            let internal_path: String = ar.get("internal_path");
            let display_name: String = ar.get("display_name");
            let size_i64: i64 = ar.get("size");
            let created_at_i64: i64 = ar.get("created_at");

            att_map.entry(msg_id).or_default().push(Attachment {
                r#type: mime_type,
                src: ar.get("src"),
                name: display_name,
                size: size_i64 as u64,
                hash: Some(hash),
                status: Some(ar.get("status")),
                internal_path,
                extracted_text: ar.get("extracted_text"),
                image_frames: ar
                    .get::<Option<String>, _>("image_frames")
                    .and_then(|s| serde_json::from_str(&s).ok()),
                thumbnail_path: ar.get("thumbnail_path"),
                created_at: Some(created_at_i64 as u64),
            });
        }
    }

    let mut history = Vec::new();
    for row in rows {
        use sqlx::Row;
        let msg_id: String = row.get("msg_id");
        let role: String = row.get("role");
        let name: Option<String> = row.get("name");
        let content: String = row.get("content");
        let timestamp: i64 = row.get("timestamp");
        let is_thinking: Option<bool> = Some(row.get::<i64, _>("is_thinking") != 0);

        let render_content: Option<Vec<u8>> = row.get("render_content");
        let blocks = if let Some(bytes) = render_content {
            serde_json::from_slice(&bytes).ok()
        } else {
            None
        };

        let attachments = att_map.remove(&msg_id);

        history.push(ChatMessage {
            id: msg_id,
            role,
            name,
            content,
            timestamp: timestamp as u64,
            is_thinking,
            agent_id: row.get("agent_id"),
            group_id: row.get("group_id"),
            topic_id: Some(topic_id.to_string()),
            is_group_message: Some(row.get::<i64, _>("is_group_message") != 0),
            finish_reason: row.get("finish_reason"),
            attachments,
            blocks,
        });
    }

    history.reverse();
    Ok(history)
}

/// 核心：确保消息中的附件在手机本地物理存在，否则从电脑同步下载
async fn ensure_attachments_locally(
    app: &AppHandle,
    message: &mut ChatMessage,
) -> Result<(), String> {
    let attachments = match &mut message.attachments {
        Some(atts) => atts,
        None => return Ok(()),
    };

    let app_config = app.path().app_config_dir().map_err(|e| e.to_string())?;
    let att_dir = app_config.join("data").join("attachments");
    if !att_dir.exists() {
        fs::create_dir_all(&att_dir)
            .await
            .map_err(|e| e.to_string())?;
    }

    for att in attachments {
        let hash = match &att.hash {
            Some(h) => h.clone(),
            None => continue,
        };

        // 判定后缀 (对齐 file_manager.rs 逻辑)
        let ext = Path::new(&att.name)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("");

        let local_file_name = if ext.is_empty() {
            hash.clone()
        } else {
            format!("{}.{}", hash, ext)
        };

        let local_path = att_dir.join(&local_file_name);
        let local_path_str = local_path.to_string_lossy().into_owned();

        if !local_path.exists() {
            // 尝试下载
            let settings = settings_manager::read_settings(app.clone(), app.state()).await?;
            if !settings.sync_http_url.is_empty() {
                let client = reqwest::Client::new();
                let url = format!(
                    "{}/api/mobile-sync/download-attachment?hash={}",
                    settings.sync_http_url, hash
                );
                match client
                    .get(&url)
                    .header("x-sync-token", &settings.sync_token)
                    .send()
                    .await
                {
                    Ok(resp) if resp.status().is_success() => {
                        if let Ok(bytes) = resp.bytes().await {
                            let _ = fs::write(&local_path, bytes).await;
                        }
                    }
                    _ => {} // 下载失败则跳过，UI 会显示裂图
                }
            }
        }

        // 核心对齐：
        // 1. src 保持物理路径（用于超栈追踪），如果来自电脑端，它已经包含 file:// 路径
        // 2. internal_path 专门作为手机本地可访问路径，前端可通过 convertFileSrc 转换为 asset://
        if att.src.is_empty() {
            att.src = format!("file://{}", local_path_str);
        }
        att.internal_path = local_path_str;
    }
    Ok(())
}

pub async fn append_single_message(
    app_handle: AppHandle,
    db_pool: &sqlx::Pool<sqlx::Sqlite>,
    _owner_id: &str,
    _owner_type: &str,
    topic_id: String,
    mut message: ChatMessage,
) -> Result<(), String> {
    ensure_attachments_locally(&app_handle, &mut message).await?;

    let blocks = MessageRenderCompiler::compile(&message.content);
    let render_bytes = MessageRenderCompiler::serialize(&blocks)?;

    let mut tx = db_pool.begin().await.map_err(|e| e.to_string())?;
    MessageRepository::upsert_message(&mut tx, &message, &topic_id, &render_bytes, false).await?;

    let msg_count: i32 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM messages WHERE topic_id = ? AND deleted_at IS NULL",
    )
    .bind(&topic_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| e.to_string())?
    .unwrap_or(0);

    sqlx::query("UPDATE topics SET updated_at = ?, msg_count = ? WHERE topic_id = ?")
        .bind(message.timestamp as i64)
        .bind(msg_count)
        .bind(&topic_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(())
}

pub async fn patch_single_message(
    app_handle: AppHandle,
    db_pool: &sqlx::Pool<sqlx::Sqlite>,
    _owner_id: &str,
    _owner_type: &str,
    topic_id: String,
    mut message: ChatMessage,
    skip_bubble: bool,
) -> Result<(), String> {
    ensure_attachments_locally(&app_handle, &mut message).await?;

    let blocks = MessageRenderCompiler::compile(&message.content);
    let render_bytes = MessageRenderCompiler::serialize(&blocks)?;

    let mut tx = db_pool.begin().await.map_err(|e| e.to_string())?;
    MessageRepository::upsert_message(&mut tx, &message, &topic_id, &render_bytes, skip_bubble)
        .await?;

    let now = chrono::Utc::now().timestamp_millis();
    sqlx::query("UPDATE topics SET updated_at = ? WHERE topic_id = ?")
        .bind(now)
        .bind(&topic_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(())
}

#[allow(dead_code)]
pub async fn patch_single_message_no_app(
    db_pool: &sqlx::Pool<sqlx::Sqlite>,
    _owner_id: &str,
    _owner_type: &str,
    topic_id: String,
    message: ChatMessage,
    skip_bubble: bool,
) -> Result<(), String> {
    let blocks = MessageRenderCompiler::compile(&message.content);
    let render_bytes = MessageRenderCompiler::serialize(&blocks)?;

    let mut tx = db_pool.begin().await.map_err(|e| e.to_string())?;
    MessageRepository::upsert_message(&mut tx, &message, &topic_id, &render_bytes, skip_bubble)
        .await?;

    let now = chrono::Utc::now().timestamp_millis();
    sqlx::query("UPDATE topics SET updated_at = ? WHERE topic_id = ?")
        .bind(now)
        .bind(&topic_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(())
}

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
    let now = chrono::Utc::now().timestamp_millis();
    let mut q = sqlx::query(&delete_query).bind(now).bind(topic_id);
    for id in &msg_ids {
        q = q.bind(id);
    }
    q.execute(&mut *tx).await.map_err(|e| e.to_string())?;

    let msg_count: i32 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM messages WHERE topic_id = ? AND deleted_at IS NULL",
    )
    .bind(topic_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| e.to_string())?
    .unwrap_or(0);

    sqlx::query("UPDATE topics SET msg_count = ?, updated_at = ? WHERE topic_id = ?")
        .bind(msg_count)
        .bind(now)
        .bind(topic_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(())
}

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
        .bind(topic_id).bind(timestamp).execute(&mut *tx).await.map_err(|e| e.to_string())?;
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
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| e.to_string())?
    .unwrap_or(0);
    sqlx::query("UPDATE topics SET msg_count = ?, updated_at = ? WHERE topic_id = ?")
        .bind(msg_count)
        .bind(timestamp)
        .bind(topic_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(())
}

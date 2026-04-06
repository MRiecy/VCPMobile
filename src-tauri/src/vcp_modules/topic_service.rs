use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::storage_paths::{is_group_item, resolve_topic_dir, resolve_jsonl_path, resolve_astbin_path};
use crate::vcp_modules::topic_metadata_sync_service;
use crate::vcp_modules::topic_list_manager::Topic;
use std::fs;
use tauri::AppHandle;

pub async fn get_topics(db_state: &DbState, item_id: &str) -> Result<Vec<Topic>, String> {
    let pool = &db_state.pool;
    let rows = sqlx::query(
        "SELECT topic_id, title, created_at, locked, unread, unread_count, msg_count 
         FROM topics WHERE owner_id = ? AND deleted_at IS NULL ORDER BY updated_at DESC"
    )
    .bind(item_id)
    .fetch_all(pool)
    .await
    .map_err(|e| e.to_string())?;

    let mut topics = Vec::new();
    for row in rows {
        use sqlx::Row;
        topics.push(Topic {
            id: row.get("topic_id"),
            name: row.get("title"),
            created_at: row.get("created_at"),
            locked: row.get::<i32, _>("locked") != 0,
            unread: row.get::<i32, _>("unread") != 0,
            unread_count: row.get("unread_count"),
            msg_count: row.get("msg_count"),
        });
    }
    Ok(topics)
}

/// Project Leviathan: Initialize core storage and topics table
pub async fn init_topic_storage(
    db_pool: &sqlx::Pool<sqlx::Sqlite>,
    item_id: &str,
    topic_id: &str,
    title: &str,
    timestamp: i64,
) -> Result<(), String> {
    // 1. Ensure directory exists is handled by caller (usually create_topic) or resolve paths
    // But we need to ensure the files themselves exist
    
    // Initialize topics in DB
    sqlx::query(
        "INSERT INTO topics (topic_id, owner_id, owner_type, title, created_at, updated_at, revision, msg_count, locked, unread, unread_count)
         VALUES (?, ?, ?, ?, ?, ?, 0, 0, 0, 0, 0)
         ON CONFLICT(topic_id) DO UPDATE SET
            title = excluded.title,
            updated_at = excluded.updated_at"
    )
    .bind(topic_id)
    .bind(item_id)
    .bind(if item_id.starts_with("____") { "group" } else { "agent" })
    .bind(title)
    .bind(timestamp)
    .bind(timestamp)
    .execute(db_pool)
    .await
    .map_err(|e| e.to_string())?;

    Ok(())
}

pub async fn create_topic(
    app_handle: &AppHandle,
    db_state: &DbState,
    item_id: &str,
    name: &str,
) -> Result<Topic, String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;

    let id = if is_group_item(app_handle, item_id) {
        format!("group_topic_{}", now)
    } else {
        format!("topic_{}", now)
    };

    let topic = Topic {
        id: id.clone(),
        name: name.to_string(),
        created_at: now,
        locked: false,
        unread: false,
        unread_count: 0,
        msg_count: 0,
    };

    // 1. 创建目录
    let topic_dir = resolve_topic_dir(app_handle, item_id, &id);
    fs::create_dir_all(&topic_dir).map_err(|e| e.to_string())?;

    // Project Leviathan: Initialize core files
    let jsonl_path = resolve_jsonl_path(app_handle, item_id, &id);
    let astbin_path = resolve_astbin_path(app_handle, item_id, &id);
    fs::File::create(&jsonl_path).map_err(|e| format!("Failed to create jsonl: {}", e))?;
    fs::File::create(&astbin_path).map_err(|e| format!("Failed to create astbin: {}", e))?;

    // Initialize topic data state
    if let Err(e) = init_topic_storage(&db_state.pool, item_id, &id, name, now).await {
        // Cleanup: If DB init fails, remove the created directory
        let _ = fs::remove_dir_all(&topic_dir);
        return Err(format!("[CreateTopic] DB initialization failed: {}", e));
    }

    // 2. 同步元数据到主配置
    if let Err(e) = topic_metadata_sync_service::sync_new_topic(app_handle, item_id, &topic).await {
        // Cleanup: If sync fails, use delete_topic to rollback DB and Files
        let _ = delete_topic(app_handle, db_state, item_id, &id).await;
        return Err(format!("[CreateTopic] Metadata sync failed: {}", e));
    }

    Ok(topic)
}

pub async fn delete_topic(
    app_handle: &AppHandle,
    db_state: &DbState,
    item_id: &str,
    topic_id: &str,
) -> Result<(), String> {
    // Project Leviathan: 从 topics, messages, message_attachments 删除
    sqlx::query("DELETE FROM message_attachments WHERE msg_id IN (SELECT msg_id FROM messages WHERE topic_id = ?)")
        .bind(topic_id)
        .execute(&db_state.pool)
        .await
        .map_err(|e| e.to_string())?;

    sqlx::query("DELETE FROM messages WHERE topic_id = ?")
        .bind(topic_id)
        .execute(&db_state.pool)
        .await
        .map_err(|e| e.to_string())?;

    sqlx::query("DELETE FROM topics WHERE topic_id = ?")
        .bind(topic_id)
        .execute(&db_state.pool)
        .await
        .map_err(|e| e.to_string())?;

    // 1. 删除磁盘文件
    let topic_dir = resolve_topic_dir(app_handle, item_id, topic_id);

    if topic_dir.exists() {
        fs::remove_dir_all(topic_dir).map_err(|e| e.to_string())?;
    }

    Ok(())
}

pub async fn update_topic_title(
    app_handle: &AppHandle,
    db_state: &DbState,
    item_id: &str,
    topic_id: &str,
    title: &str,
) -> Result<(), String> {
    // 1. 更新数据库
    sqlx::query("UPDATE topics SET title = ?, updated_at = ? WHERE topic_id = ?")
        .bind(title)
        .bind(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as i64,
        )
        .bind(topic_id)
        .execute(&db_state.pool)
        .await
        .map_err(|e| e.to_string())?;

    // 2. 更新元数据
    topic_metadata_sync_service::update_topic_metadata(app_handle, item_id, topic_id, |topic| {
        // 兼容 TopicInfo (name) 和 Topic (title/name)
        if let Some(name) = topic.get_mut("name") {
            *name = serde_json::Value::String(title.to_string());
        }
        if let Some(t) = topic.get_mut("title") {
            *t = serde_json::Value::String(title.to_string());
        }
    })
    .await?;

    Ok(())
}

pub async fn toggle_topic_lock(
    app_handle: &AppHandle,
    db_state: &DbState,
    item_id: &str,
    topic_id: &str,
    locked: bool,
) -> Result<(), String> {
    // 1. 更新数据库
    sqlx::query("UPDATE topics SET locked = ?, updated_at = ? WHERE topic_id = ?")
        .bind(locked)
        .bind(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as i64,
        )
        .bind(topic_id)
        .execute(&db_state.pool)
        .await
        .map_err(|e| e.to_string())?;

    // 2. 更新元数据
    topic_metadata_sync_service::update_topic_metadata(app_handle, item_id, topic_id, |topic| {
        topic["locked"] = serde_json::Value::Bool(locked);
    })
    .await?;

    Ok(())
}

pub async fn set_topic_unread(
    app_handle: &AppHandle,
    db_state: &DbState,
    item_id: &str,
    topic_id: &str,
    unread: bool,
) -> Result<(), String> {
    // 1. 更新数据库
    sqlx::query("UPDATE topics SET unread = ?, updated_at = ? WHERE topic_id = ?")
        .bind(unread)
        .bind(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as i64,
        )
        .bind(topic_id)
        .execute(&db_state.pool)
        .await
        .map_err(|e| e.to_string())?;

    // 2. 更新元数据
    topic_metadata_sync_service::update_topic_metadata(app_handle, item_id, topic_id, |topic| {
        topic["unread"] = serde_json::Value::Bool(unread);
    })
    .await?;

    Ok(())
}

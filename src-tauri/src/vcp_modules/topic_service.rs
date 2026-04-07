// TopicService: 处理会话话题生命周期的模块
// 职责: 完全面向 SQLite 数据库的话题管理，不依赖本地文件系统

use crate::vcp_modules::app_settings_manager::AppSettingsState;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::topic_types::Topic;
use tauri::{AppHandle, State};

#[tauri::command]
pub async fn get_topics(
    db_state: State<'_, DbState>,
    owner_id: String,
    owner_type: String,
) -> Result<Vec<Topic>, String> {
    let pool = &db_state.pool;
    let rows = sqlx::query(
        "SELECT topic_id, title, created_at, locked, unread, unread_count, msg_count 
         FROM topics 
         WHERE owner_id = ? AND owner_type = ? AND deleted_at IS NULL 
         ORDER BY updated_at DESC",
    )
    .bind(&owner_id)
    .bind(&owner_type)
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
            extra_fields: serde_json::Map::new(),
        });
    }
    Ok(topics)
}

#[tauri::command]
pub async fn create_topic(
    _app_handle: AppHandle,
    db_state: State<'_, DbState>,
    owner_id: String,
    owner_type: String,
    name: String,
) -> Result<Topic, String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;

    let id = if owner_type == "group" {
        format!("group_topic_{}", now)
    } else {
        format!("topic_{}", now)
    };

    let topic = Topic {
        id: id.clone(),
        name: name.clone(),
        created_at: now,
        locked: false,
        unread: false,
        unread_count: 0,
        msg_count: 0,
        extra_fields: serde_json::Map::new(),
    };

    sqlx::query(
        "INSERT INTO topics (topic_id, owner_id, owner_type, title, created_at, updated_at, revision, msg_count, locked, unread, unread_count)
         VALUES (?, ?, ?, ?, ?, ?, 0, 0, 0, 0, 0)",
    )
    .bind(&id)
    .bind(&owner_id)
    .bind(&owner_type)
    .bind(&name)
    .bind(now)
    .bind(now)
    .execute(&db_state.pool)
    .await
    .map_err(|e| format!("[CreateTopic] DB initialization failed: {}", e))?;

    Ok(topic)
}

#[tauri::command]
pub async fn delete_topic(
    _app_handle: AppHandle,
    db_state: State<'_, DbState>,
    _owner_id: String,
    _owner_type: String,
    topic_id: String,
) -> Result<(), String> {
    let mut tx = db_state.pool.begin().await.map_err(|e| e.to_string())?;

    sqlx::query(
        "DELETE FROM message_attachments WHERE msg_id IN (SELECT msg_id FROM messages WHERE topic_id = ?)",
    )
    .bind(&topic_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;

    sqlx::query("DELETE FROM messages WHERE topic_id = ?")
        .bind(&topic_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

    sqlx::query("DELETE FROM topics WHERE topic_id = ?")
        .bind(&topic_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

    tx.commit().await.map_err(|e| e.to_string())?;

    Ok(())
}

#[tauri::command]
pub async fn update_topic_title(
    _app_handle: AppHandle,
    db_state: State<'_, DbState>,
    _owner_id: String,
    _owner_type: String,
    topic_id: String,
    title: String,
) -> Result<(), String> {
    sqlx::query("UPDATE topics SET title = ?, updated_at = ? WHERE topic_id = ?")
        .bind(&title)
        .bind(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as i64,
        )
        .bind(&topic_id)
        .execute(&db_state.pool)
        .await
        .map_err(|e| e.to_string())?;

    Ok(())
}

#[tauri::command]
pub async fn summarize_topic(
    app_handle: AppHandle,
    settings_state: State<'_, AppSettingsState>,
    owner_id: String,
    owner_type: String,
    topic_id: String,
    agent_name: String,
) -> Result<String, String> {
    crate::vcp_modules::topic_summary_service::summarize_topic(
        app_handle,
        settings_state,
        owner_id,
        owner_type,
        topic_id,
        agent_name,
    )
    .await
}

#[tauri::command]
pub async fn toggle_topic_lock(
    _app_handle: AppHandle,
    db_state: State<'_, DbState>,
    _owner_id: String,
    _owner_type: String,
    topic_id: String,
    locked: bool,
) -> Result<(), String> {
    sqlx::query("UPDATE topics SET locked = ?, updated_at = ? WHERE topic_id = ?")
        .bind(locked)
        .bind(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as i64,
        )
        .bind(&topic_id)
        .execute(&db_state.pool)
        .await
        .map_err(|e| e.to_string())?;

    Ok(())
}

#[tauri::command]
pub async fn set_topic_unread(
    _app_handle: AppHandle,
    db_state: State<'_, DbState>,
    _owner_id: String,
    _owner_type: String,
    topic_id: String,
    unread: bool,
) -> Result<(), String> {
    sqlx::query("UPDATE topics SET unread = ?, updated_at = ? WHERE topic_id = ?")
        .bind(unread)
        .bind(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as i64,
        )
        .bind(&topic_id)
        .execute(&db_state.pool)
        .await
        .map_err(|e| e.to_string())?;

    Ok(())
}

// TopicService: 处理会话话题生命周期的模块
// 职责: 完全面向 SQLite 数据库的话题管理，不依赖本地文件系统

use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::topic_list_manager::Topic;
use tauri::AppHandle;

pub async fn get_topics(
    db_state: &DbState,
    owner_id: &str,
    owner_type: &str,
) -> Result<Vec<Topic>, String> {
    let pool = &db_state.pool;
    let rows = sqlx::query(
        "SELECT topic_id, title, created_at, locked, unread, unread_count, msg_count 
         FROM topics 
         WHERE owner_id = ? AND owner_type = ? AND deleted_at IS NULL 
         ORDER BY updated_at DESC",
    )
    .bind(owner_id)
    .bind(owner_type)
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

pub async fn create_topic(
    _app_handle: &AppHandle,
    db_state: &DbState,
    owner_id: &str,
    owner_type: &str,
    name: &str,
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
        name: name.to_string(),
        created_at: now,
        locked: false,
        unread: false,
        unread_count: 0,
        msg_count: 0,
    };

    sqlx::query(
        "INSERT INTO topics (topic_id, owner_id, owner_type, title, created_at, updated_at, revision, msg_count, locked, unread, unread_count)
         VALUES (?, ?, ?, ?, ?, ?, 0, 0, 0, 0, 0)"
    )
    .bind(&id)
    .bind(owner_id)
    .bind(owner_type)
    .bind(name)
    .bind(now)
    .bind(now)
    .execute(&db_state.pool)
    .await
    .map_err(|e| format!("[CreateTopic] DB initialization failed: {}", e))?;

    Ok(topic)
}

pub async fn delete_topic(
    _app_handle: &AppHandle,
    db_state: &DbState,
    _owner_id: &str,
    _owner_type: &str,
    topic_id: &str,
) -> Result<(), String> {
    let mut tx = db_state.pool.begin().await.map_err(|e| e.to_string())?;

    sqlx::query("DELETE FROM message_attachments WHERE msg_id IN (SELECT msg_id FROM messages WHERE topic_id = ?)")
        .bind(topic_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

    sqlx::query("DELETE FROM messages WHERE topic_id = ?")
        .bind(topic_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

    sqlx::query("DELETE FROM topics WHERE topic_id = ?")
        .bind(topic_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

    tx.commit().await.map_err(|e| e.to_string())?;

    Ok(())
}

pub async fn update_topic_title(
    _app_handle: &AppHandle,
    db_state: &DbState,
    _owner_id: &str,
    _owner_type: &str,
    topic_id: &str,
    title: &str,
) -> Result<(), String> {
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

    Ok(())
}

pub async fn toggle_topic_lock(
    _app_handle: &AppHandle,
    db_state: &DbState,
    _owner_id: &str,
    _owner_type: &str,
    topic_id: &str,
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
        .bind(topic_id)
        .execute(&db_state.pool)
        .await
        .map_err(|e| e.to_string())?;

    Ok(())
}

pub async fn set_topic_unread(
    _app_handle: &AppHandle,
    db_state: &DbState,
    _owner_id: &str,
    _owner_type: &str,
    topic_id: &str,
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
        .bind(topic_id)
        .execute(&db_state.pool)
        .await
        .map_err(|e| e.to_string())?;

    Ok(())
}

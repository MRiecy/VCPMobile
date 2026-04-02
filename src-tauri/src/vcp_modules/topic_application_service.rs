use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::path_topology_service::{is_group_item, resolve_topic_dir};
use crate::vcp_modules::topic_metadata_sync_service;
use crate::vcp_modules::topic_repository_projection::{self, Topic};
use std::fs;
use tauri::AppHandle;

pub async fn get_topics(db_state: &DbState, item_id: &str) -> Result<Vec<Topic>, String> {
    topic_repository_projection::get_topics_by_item_id(db_state, item_id).await
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
        last_msg_preview: None,
        msg_count: 0,
    };

    // 1. 写入数据库索引
    sqlx::query(
        "INSERT INTO topic_index (topic_id, agent_id, title, mtime, locked, unread, unread_count) VALUES (?, ?, ?, ?, ?, ?, ?)"
    )
    .bind(&id)
    .bind(item_id)
    .bind(name)
    .bind(now)
    .bind(false)
    .bind(false)
    .bind(0)
    .execute(&db_state.pool)
    .await
    .map_err(|e| e.to_string())?;

    // 2. 创建目录
    let topic_dir = resolve_topic_dir(app_handle, item_id, &id);
    fs::create_dir_all(&topic_dir).map_err(|e| e.to_string())?;

    // 初始化 history.json (内容为 [])
    let history_path = topic_dir.join("history.json");
    fs::write(history_path, "[]").map_err(|e| e.to_string())?;

    // 3. 同步元数据到主配置和话题目录
    topic_metadata_sync_service::sync_new_topic(app_handle, item_id, &topic).await?;

    Ok(topic)
}

pub async fn delete_topic(
    app_handle: &AppHandle,
    db_state: &DbState,
    item_id: &str,
    topic_id: &str,
) -> Result<(), String> {
    // 1. 从数据库删除
    sqlx::query("DELETE FROM topic_index WHERE topic_id = ?")
        .bind(topic_id)
        .execute(&db_state.pool)
        .await
        .map_err(|e| e.to_string())?;

    // 2. 删除磁盘文件
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
    sqlx::query("UPDATE topic_index SET title = ?, mtime = ? WHERE topic_id = ?")
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
    sqlx::query("UPDATE topic_index SET locked = ?, mtime = ? WHERE topic_id = ?")
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
    sqlx::query("UPDATE topic_index SET unread = ?, mtime = ? WHERE topic_id = ?")
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

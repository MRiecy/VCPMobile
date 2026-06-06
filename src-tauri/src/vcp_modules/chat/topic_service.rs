// TopicService: 处理会话话题生命周期的模块
// 职责: 完全面向 SQLite 数据库的话题管理，不依赖本地文件系统

use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::settings_manager::SettingsState;
use crate::vcp_modules::sync_hash::HashAggregator;
use crate::vcp_modules::sync_service::{SyncCommand, SyncState};
use crate::vcp_modules::sync_types::SyncDataType;
use crate::vcp_modules::topic_types::Topic;
use serde_json::Value;
use sqlx::Row;
use std::collections::HashMap;
use tauri::{ipc::Channel, AppHandle, Manager, State};

/// 批量获取所有 owner 的未读计数，替代前端的 N+1 查询
#[tauri::command]
pub async fn get_unread_counts(
    db_state: State<'_, DbState>,
) -> Result<HashMap<String, i32>, String> {
    let pool = &db_state.pool;
    let rows = sqlx::query(
        "SELECT owner_id, 
                CAST(COALESCE(SUM(unread_count), 0) AS INTEGER) as total_count,
                MAX(CASE WHEN unread = 1 THEN 1 ELSE 0 END) as has_unread
         FROM topics 
         WHERE deleted_at IS NULL
         GROUP BY owner_id",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| e.to_string())?;

    let mut result = HashMap::new();
    for row in rows {
        let owner_id: String = row.get("owner_id");
        let total_count: i64 = row.get("total_count");
        let has_unread: i32 = row.get("has_unread");

        let value = if total_count > 0 {
            total_count as i32
        } else if has_unread != 0 {
            -1
        } else {
            0
        };

        if value != 0 {
            result.insert(owner_id, value);
        }
    }

    Ok(result)
}

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
         ORDER BY created_at DESC",
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
            owner_id: owner_id.clone(),
            owner_type: owner_type.clone(),
        });
    }
    Ok(topics)
}

#[tauri::command]
pub async fn get_topics_streamed(
    db_state: State<'_, DbState>,
    owner_id: String,
    owner_type: String,
    on_chunk: Channel<Vec<Topic>>,
) -> Result<(), String> {
    let pool = &db_state.pool;
    let mut rows = sqlx::query(
        "SELECT topic_id, title, created_at, locked, unread, unread_count, msg_count 
         FROM topics 
         WHERE owner_id = ? AND owner_type = ? AND deleted_at IS NULL 
         ORDER BY created_at DESC",
    )
    .bind(&owner_id)
    .bind(&owner_type)
    .fetch(pool);

    use futures_util::StreamExt;
    let mut chunk = Vec::new();
    let chunk_size = 15;

    while let Some(row_result) = rows.next().await {
        let row = row_result.map_err(|e| e.to_string())?;
        use sqlx::Row;
        chunk.push(Topic {
            id: row.get("topic_id"),
            name: row.get("title"),
            created_at: row.get("created_at"),
            locked: row.get::<i32, _>("locked") != 0,
            unread: row.get::<i32, _>("unread") != 0,
            unread_count: row.get("unread_count"),
            msg_count: row.get("msg_count"),
            owner_id: owner_id.clone(),
            owner_type: owner_type.clone(),
        });

        if chunk.len() >= chunk_size {
            on_chunk.send(chunk.clone()).map_err(|e| e.to_string())?;
            chunk.clear();
        }
    }

    if !chunk.is_empty() {
        on_chunk.send(chunk).map_err(|e| e.to_string())?;
    }

    Ok(())
}

#[tauri::command]
pub async fn create_topic(
    _app_handle: AppHandle,
    db_state: State<'_, DbState>,
    owner_id: String,
    owner_type: String,
    name: String,
) -> Result<Topic, String> {
    let now = crate::vcp_modules::infra::utils::now_millis();

    let id = if owner_type == "group" {
        format!("group_topic_{}", now)
    } else {
        format!("topic_{}", now)
    };

    let topic = Topic {
        id: id.clone(),
        name: name.clone(),
        created_at: now,
        locked: true,
        unread: false,
        unread_count: 0,
        msg_count: 0,
        owner_id: owner_id.clone(),
        owner_type: owner_type.clone(),
    };

    sqlx::query(
        "INSERT INTO topics (topic_id, owner_id, owner_type, title, created_at, updated_at, msg_count, locked, unread, unread_count)
         VALUES (?, ?, ?, ?, ?, ?, 0, 1, 0, 0)",
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

    // 触发聚合哈希冒泡 (初始化 Topic Hash 并更新 Agent/Group 的 ContentHash)
    let mut tx = db_state.pool.begin().await.map_err(|e| e.to_string())?;
    if let Err(e) = HashAggregator::bubble_from_topic(&mut tx, &id).await {
        log::error!(
            "[CreateTopic] Failed to bubble hash for topic {}: {}",
            id,
            e
        );
    }
    tx.commit().await.map_err(|e| e.to_string())?;

    Ok(topic)
}

#[tauri::command]
pub async fn delete_topic(
    app_handle: AppHandle,
    db_state: State<'_, DbState>,
    owner_id: String,
    owner_type: String,
    topic_id: String,
) -> Result<(), String> {
    let now = chrono::Utc::now().timestamp_millis();

    sqlx::query("UPDATE topics SET deleted_at = ? WHERE topic_id = ?")
        .bind(now)
        .bind(&topic_id)
        .execute(&db_state.pool)
        .await
        .map_err(|e| e.to_string())?;

    if let Some(sync_state) = app_handle.try_state::<SyncState>() {
        let _ = sync_state.ws_sender.send(SyncCommand::NotifyDelete {
            data_type: SyncDataType::Topic,
            id: topic_id.clone(),
        });
    }

    let mut tx = db_state.pool.begin().await.map_err(|e| e.to_string())?;
    if owner_type == "agent" {
        let _ = HashAggregator::bubble_agent_hash(&mut tx, &owner_id).await;
    } else if owner_type == "group" {
        let _ = HashAggregator::bubble_group_hash(&mut tx, &owner_id).await;
    }
    let _ = tx.commit().await;

    Ok(())
}

#[tauri::command]
pub async fn update_topic_title(
    app_handle: AppHandle,
    db_state: State<'_, DbState>,
    _owner_id: String,
    _owner_type: String,
    topic_id: String,
    title: String,
) -> Result<(), String> {
    let now = crate::vcp_modules::infra::utils::now_millis();

    sqlx::query("UPDATE topics SET title = ?, updated_at = ? WHERE topic_id = ?")
        .bind(&title)
        .bind(now)
        .bind(&topic_id)
        .execute(&db_state.pool)
        .await
        .map_err(|e| e.to_string())?;

    // 1. 触发聚合哈希冒泡 (重算当前 topic 的哈希，并向上累加到 Agent/Group)
    let mut tx = db_state.pool.begin().await.map_err(|e| e.to_string())?;
    HashAggregator::bubble_from_topic(&mut tx, &topic_id).await?;
    tx.commit().await.map_err(|e| e.to_string())?;

    // 2. 发送同步通知给局域网同步网络
    if let Some(sync_state) = app_handle.try_state::<SyncState>() {
        let row = sqlx::query("SELECT config_hash FROM topics WHERE topic_id = ?")
            .bind(&topic_id)
            .fetch_one(&db_state.pool)
            .await
            .map_err(|e| e.to_string())?;

        let hash: String = row.get("config_hash");
        let _ = sync_state.ws_sender.send(SyncCommand::NotifyLocalChange {
            data_type: SyncDataType::Topic,
            id: topic_id,
            hash,
            ts: now,
        });
    }

    Ok(())
}

#[tauri::command]
pub async fn summarize_topic(
    app_handle: AppHandle,
    settings_state: State<'_, SettingsState>,
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
    app_handle: AppHandle,
    db_state: State<'_, DbState>,
    _owner_id: String,
    _owner_type: String,
    topic_id: String,
    locked: bool,
) -> Result<(), String> {
    let now = crate::vcp_modules::infra::utils::now_millis();

    sqlx::query("UPDATE topics SET locked = ?, updated_at = ? WHERE topic_id = ?")
        .bind(locked)
        .bind(now)
        .bind(&topic_id)
        .execute(&db_state.pool)
        .await
        .map_err(|e| e.to_string())?;

    // 1. 触发聚合哈希冒泡
    let mut tx = db_state.pool.begin().await.map_err(|e| e.to_string())?;
    HashAggregator::bubble_from_topic(&mut tx, &topic_id).await?;
    tx.commit().await.map_err(|e| e.to_string())?;

    // 2. 发送同步通知
    if let Some(sync_state) = app_handle.try_state::<SyncState>() {
        let row = sqlx::query("SELECT config_hash FROM topics WHERE topic_id = ?")
            .bind(&topic_id)
            .fetch_one(&db_state.pool)
            .await
            .map_err(|e| e.to_string())?;

        let hash: String = row.get("config_hash");
        let _ = sync_state.ws_sender.send(SyncCommand::NotifyLocalChange {
            data_type: SyncDataType::Topic,
            id: topic_id,
            hash,
            ts: now,
        });
    }

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
    set_topic_unread_in_pool(
        &db_state.pool,
        &topic_id,
        unread,
        crate::vcp_modules::infra::utils::now_millis(),
    )
    .await?;

    Ok(())
}

async fn set_topic_unread_in_pool(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    topic_id: &str,
    unread: bool,
    updated_at: i64,
) -> Result<(), String> {
    let unread_int = if unread { 1 } else { 0 };
    sqlx::query(
        "UPDATE topics
         SET unread = ?,
             unread_count = CASE WHEN ? = 0 THEN 0 ELSE unread_count END,
             updated_at = ?
         WHERE topic_id = ?",
    )
    .bind(unread_int)
    .bind(unread_int)
    .bind(updated_at)
    .bind(topic_id)
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    Ok(())
}

#[derive(serde::Deserialize, Clone, Debug)]
pub struct TempMessage {
    pub role: String,
    pub name: Option<String>,
    pub content: String,
    pub timestamp: u64,
}

#[tauri::command]
pub async fn archive_assistant_chat(
    app_handle: AppHandle,
    db_state: State<'_, DbState>,
    owner_id: String,
    owner_type: String,
    temp_messages: Vec<TempMessage>,
) -> Result<String, String> {
    if temp_messages.is_empty() {
        return Err("No messages to archive".to_string());
    }

    let now_millis = crate::vcp_modules::infra::utils::now_millis();

    // 1. 创建默认名称的话题
    let formatted_time = chrono::Local::now().format("%m-%d %H:%M").to_string();
    let default_title = format!("划词助手 {}", formatted_time);

    let topic = create_topic(
        app_handle.clone(),
        db_state.clone(),
        owner_id.clone(),
        owner_type.clone(),
        default_title,
    )
    .await?;

    let new_topic_id = topic.id;

    // 2. 在事务中批量写入消息
    let mut tx = db_state.pool.begin().await.map_err(|e| e.to_string())?;

    for (index, temp_msg) in temp_messages.iter().enumerate() {
        let msg_id = format!("assistant_msg_{}_{}", now_millis, index);

        // 编译 AST 块并序列化
        let blocks =
            crate::vcp_modules::persistence::message_repository::MessageRenderCompiler::compile(
                &temp_msg.content,
            );
        let render_content =
            crate::vcp_modules::persistence::message_repository::MessageRenderCompiler::serialize(
                &blocks,
            )?;

        let chat_msg = crate::vcp_modules::chat_manager::ChatMessage {
            id: msg_id,
            role: temp_msg.role.clone(),
            name: temp_msg.name.clone(),
            content: temp_msg.content.clone(),
            timestamp: temp_msg.timestamp,
            is_thinking: Some(false),
            agent_id: if owner_type == "agent" {
                Some(owner_id.clone())
            } else {
                None
            },
            group_id: if owner_type == "group" {
                Some(owner_id.clone())
            } else {
                None
            },
            topic_id: Some(new_topic_id.clone()),
            is_group_message: Some(owner_type == "group"),
            finish_reason: None,
            attachments: None,
            blocks: None,
            shell: None,
            content_hash: None,
        };

        crate::vcp_modules::persistence::message_repository::MessageRepository::upsert_message(
            &mut tx,
            &chat_msg,
            &new_topic_id,
            &render_content,
            true, // 循环中先不重算全局 Topic 聚合哈希以加速入库
        )
        .await?;
    }

    // 3. 提交事务前冒泡重算当前 Topic 聚合哈希
    crate::vcp_modules::sync_hash::HashAggregator::bubble_from_topic(&mut tx, &new_topic_id)
        .await?;
    tx.commit().await.map_err(|e| e.to_string())?;

    // 4. 更新数据库中话题的消息计数
    sqlx::query("UPDATE topics SET msg_count = ? WHERE topic_id = ?")
        .bind(temp_messages.len() as i32)
        .bind(&new_topic_id)
        .execute(&db_state.pool)
        .await
        .map_err(|e| e.to_string())?;

    // 5. 异步调用总结标题服务并更新标题
    let app_handle_clone = app_handle.clone();
    let owner_id_clone = owner_id.clone();
    let owner_type_clone = owner_type.clone();
    let new_topic_id_clone = new_topic_id.clone();
    let pool_clone = db_state.pool.clone();

    tauri::async_runtime::spawn(async move {
        // 获取 Agent 名字以传入总结服务
        let agent_name = if owner_type_clone == "agent" {
            if let Ok(row) = sqlx::query("SELECT name FROM agents WHERE agent_id = ?")
                .bind(&owner_id_clone)
                .fetch_one(&pool_clone)
                .await
            {
                use sqlx::Row;
                row.get::<String, _>("name")
            } else {
                "Agent".to_string()
            }
        } else {
            "Group".to_string()
        };

        if let Ok(title) = crate::vcp_modules::chat::topic_summary_service::summarize_topic(
            app_handle_clone.clone(),
            app_handle_clone.state::<SettingsState>(),
            owner_id_clone.clone(),
            owner_type_clone.clone(),
            new_topic_id_clone.clone(),
            agent_name,
        )
        .await
        {
            // 调用已有接口更新标题以同步哈希至局域网
            let _ = update_topic_title(
                app_handle_clone.clone(),
                app_handle_clone.state::<DbState>(),
                owner_id_clone,
                owner_type_clone,
                new_topic_id_clone,
                title,
            )
            .await;
        }
    });

    Ok(new_topic_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::SqlitePool;

    #[tokio::test]
    async fn marking_topic_read_clears_unread_count() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query(
            "CREATE TABLE topics (
                topic_id TEXT PRIMARY KEY,
                unread INTEGER NOT NULL,
                unread_count INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO topics (topic_id, unread, unread_count, updated_at)
             VALUES ('topic_1', 1, 3, 1)",
        )
        .execute(&pool)
        .await
        .unwrap();

        set_topic_unread_in_pool(&pool, "topic_1", false, 2)
            .await
            .unwrap();

        let row = sqlx::query(
            "SELECT unread, unread_count, updated_at FROM topics WHERE topic_id = 'topic_1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.get::<i32, _>("unread"), 0);
        assert_eq!(row.get::<i32, _>("unread_count"), 0);
        assert_eq!(row.get::<i64, _>("updated_at"), 2);
    }

    #[tokio::test]
    async fn marking_topic_unread_preserves_existing_count() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query(
            "CREATE TABLE topics (
                topic_id TEXT PRIMARY KEY,
                unread INTEGER NOT NULL,
                unread_count INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO topics (topic_id, unread, unread_count, updated_at)
             VALUES ('topic_1', 0, 5, 1)",
        )
        .execute(&pool)
        .await
        .unwrap();

        set_topic_unread_in_pool(&pool, "topic_1", true, 2)
            .await
            .unwrap();

        let row = sqlx::query(
            "SELECT unread, unread_count, updated_at FROM topics WHERE topic_id = 'topic_1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.get::<i32, _>("unread"), 1);
        assert_eq!(row.get::<i32, _>("unread_count"), 5);
        assert_eq!(row.get::<i64, _>("updated_at"), 2);
    }
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn regenerate_topic_response(
    app_handle: AppHandle,
    agent_state: State<'_, crate::vcp_modules::agent_service::AgentConfigState>,
    group_state: State<'_, crate::vcp_modules::group_service::GroupManagerState>,
    db_state: State<'_, DbState>,
    active_requests: State<'_, crate::vcp_modules::vcp_client::ActiveRequests>,
    cancelled_turns: State<'_, crate::vcp_modules::vcp_client::CancelledGroupTurns>,
    settings_state: State<'_, SettingsState>,
    owner_id: String,
    owner_type: String,
    topic_id: String,
    target_user_msg_id: String,
    stream_channel: Channel<crate::vcp_modules::vcp_client::StreamEvent>,
) -> Result<Value, String> {
    log::info!(
        "[TopicService] Regenerating response for topic: {}, target msg: {}",
        topic_id,
        target_user_msg_id
    );

    // 1. 获取目标用户消息，确保内容完整
    let user_msg = crate::vcp_modules::message_service::fetch_raw_message_content(
        app_handle.clone(),
        target_user_msg_id.clone(),
    )
    .await?;

    // 2. 加载消息元数据（为了获取 timestamp 以后续截断）
    let pool = &db_state.pool;
    let row = sqlx::query("SELECT timestamp, role, name, agent_id, group_id, is_group_message FROM messages WHERE msg_id = ?")
        .bind(&target_user_msg_id)
        .fetch_one(pool)
        .await
        .map_err(|e| e.to_string())?;

    use sqlx::Row;
    let timestamp: i64 = row.get("timestamp");

    // 3. 截断该消息之后的所有历史
    crate::vcp_modules::message_service::truncate_history_after_timestamp(
        app_handle.clone(),
        &db_state.pool,
        &owner_id,
        &owner_type,
        &topic_id,
        timestamp,
    )
    .await?;

    // 4. 构造逻辑上的 ChatMessage 对象 (用于传给内部生成函数)
    let chat_msg = crate::vcp_modules::chat_manager::ChatMessage {
        id: target_user_msg_id,
        role: row.get("role"),
        name: row.get("name"),
        content: user_msg,
        timestamp: timestamp as u64,
        is_thinking: Some(false),
        agent_id: row.get("agent_id"),
        group_id: row.get("group_id"),
        topic_id: Some(topic_id.clone()),
        is_group_message: Some(row.get::<i64, _>("is_group_message") != 0),
        finish_reason: None,
        attachments: None, // 重新生成时，上下文组装会自动从数据库重新拉取附件
        blocks: None,
        shell: None,
        content_hash: None,
    };

    // 5. 获取配置并发起生成
    let settings =
        crate::vcp_modules::settings_manager::read_settings(app_handle.clone(), settings_state)
            .await?;

    if owner_type == "agent" {
        crate::vcp_modules::agent_chat_application_service::internal_process_agent_chat_message(
            app_handle,
            agent_state,
            db_state,
            active_requests,
            crate::vcp_modules::agent_chat_application_service::AgentChatPayload {
                agent_id: owner_id,
                topic_id: topic_id.clone(),
                user_message: chat_msg,
                vcp_url: settings.vcp_server_url,
                vcp_api_key: settings.vcp_api_key,
            },
            stream_channel,
            false, // skip append_user_msg
        )
        .await
    } else {
        crate::vcp_modules::group_chat_application_service::internal_process_group_chat_message(
            app_handle,
            group_state,
            agent_state,
            db_state,
            active_requests,
            cancelled_turns,
            crate::vcp_modules::group_chat_application_service::GroupChatParams {
                group_id: owner_id,
                topic_id: topic_id.clone(),
                user_message: chat_msg,
                vcp_url: settings.vcp_server_url,
                vcp_api_key: settings.vcp_api_key,
                stream_channel: Some(stream_channel),
            },
            false, // skip append_user_msg
        )
        .await
    }
}

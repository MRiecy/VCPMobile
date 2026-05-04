// TopicService: 处理会话话题生命周期的模块
// 职责: 完全面向 SQLite 数据库的话题管理，不依赖本地文件系统

use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::settings_manager::SettingsState;
use crate::vcp_modules::sync_hash::HashAggregator;
use crate::vcp_modules::sync_service::{SyncCommand, SyncState};
use crate::vcp_modules::sync_types::SyncDataType;
use crate::vcp_modules::topic_types::Topic;
use serde_json::Value;
use tauri::{ipc::Channel, AppHandle, Manager, State};

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
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;

    let id = format!("topic_{}", now);

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
    let unread_int = if unread { 1 } else { 0 };
    sqlx::query("UPDATE topics SET unread = ?, updated_at = ? WHERE topic_id = ?")
        .bind(unread_int)
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
    thinking_id: Option<String>,
    stream_channel: Channel<crate::vcp_modules::vcp_client::StreamEvent>,
) -> Result<Value, String> {
    println!(
        "[TopicService] Regenerating response for topic: {}, target msg: {}, thinking_id: {:?}",
        topic_id, target_user_msg_id, thinking_id
    );

    // 1. 获取目标用户消息，确保内容完整
    let user_msg = crate::vcp_modules::message_service::fetch_raw_message_content(
        app_handle.clone(),
        target_user_msg_id.clone(),
    )
    .await?;

    // 2. 加载消息元数据（为了获取 timestamp 以后续截断）
    let pool = &db_state.pool;
    let row = sqlx::query("SELECT timestamp, role, name, agent_id, group_id, is_thinking, is_group_message FROM messages WHERE msg_id = ?")
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
        is_thinking: Some(row.get::<i64, _>("is_thinking") != 0),
        agent_id: row.get("agent_id"),
        group_id: row.get("group_id"),
        topic_id: Some(topic_id.clone()),
        is_group_message: Some(row.get::<i64, _>("is_group_message") != 0),
        finish_reason: None,
        attachments: None, // 重新生成时，上下文组装会自动从数据库重新拉取附件
        blocks: None,
    };

    // 5. 获取配置并发起生成
    let settings =
        crate::vcp_modules::settings_manager::read_settings(app_handle.clone(), settings_state)
            .await?;

    if owner_type == "agent" {
        let final_thinking_id = thinking_id.unwrap_or_else(|| {
            format!(
                "msg_{}_assistant_regen",
                chrono::Utc::now().timestamp_millis()
            )
        });
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
                thinking_message_id: final_thinking_id,
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

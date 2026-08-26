// TopicService: 处理会话话题生命周期的模块
// 职责: 完全面向 SQLite 数据库的话题管理，不依赖本地文件系统

use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::settings_manager::SettingsState;
use crate::vcp_modules::sync_hash::HashAggregator;
use crate::vcp_modules::sync_service::{SyncCommand, SyncState};
use crate::vcp_modules::sync_types::DeleteTarget;
use crate::vcp_modules::topic_types::{MessageKey, Topic, TopicKey};
use serde_json::{json, Value};
use sqlx::Row;
use std::collections::HashMap;
use tauri::{ipc::Channel, AppHandle, Manager, State};

/// 批量获取所有 owner 的未读计数，替代前端的 N+1 查询
#[tauri::command]
pub async fn get_unread_counts(
    db_state: State<'_, DbState>,
) -> Result<HashMap<String, i32>, String> {
    let start_time = std::time::Instant::now();
    let pool = &db_state.pool;
    let rows = sqlx::query(
        "SELECT owner_type, owner_id,
                CAST(COALESCE(SUM(unread_count), 0) AS INTEGER) as total_count,
                MAX(CASE WHEN unread = 1 THEN 1 ELSE 0 END) as has_unread
         FROM topics 
         WHERE deleted_at IS NULL
         GROUP BY owner_type, owner_id",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| e.to_string())?;

    let mut result = HashMap::new();
    for row in rows {
        let owner_type: String = row.get("owner_type");
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
            result.insert(format!("{owner_type}:{owner_id}"), value);
        }
    }

    log::info!(
        "[Profile] get_unread_counts finished. Total: {}ms",
        start_time.elapsed().as_millis()
    );

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

    let mut tx = db_state.pool.begin().await.map_err(|e| e.to_string())?;
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
    .execute(&mut *tx)
    .await
    .map_err(|e| format!("[CreateTopic] DB initialization failed: {}", e))?;

    // 触发聚合哈希冒泡 (初始化 Topic Hash 并更新 Agent/Group 的 ContentHash)
    let key = TopicKey::new(&owner_type, &owner_id, &id);
    HashAggregator::bubble_from_topic(&mut tx, &key).await?;
    tx.commit().await.map_err(|e| e.to_string())?;

    Ok(topic)
}

#[tauri::command]
pub async fn delete_topic(
    app_handle: AppHandle,
    db_state: State<'_, DbState>,
    active_requests: State<'_, crate::vcp_modules::vcp_client::ActiveRequests>,
    owner_id: String,
    owner_type: String,
    topic_id: String,
) -> Result<(), String> {
    let now = chrono::Utc::now().timestamp_millis();
    let key = TopicKey::new(&owner_type, &owner_id, &topic_id);
    let deletion = delete_topic_data(&db_state.pool, &key, now)
        .await?
        .ok_or_else(|| format!("Topic {topic_id} does not exist"))?;
    if !deletion.deleted {
        return Ok(());
    }

    for message_key in deletion.active_messages {
        if let Err(error) = active_requests.cancel(&message_key) {
            log::warn!(
                "Failed to cancel generation from deleted topic {}: {}",
                message_key.msg_id,
                error
            );
        }
    }

    if let Some(sync_state) = app_handle.try_state::<SyncState>() {
        let _ = sync_state.ws_sender.send(SyncCommand::NotifyDelete {
            target: DeleteTarget::Topic(key),
            deleted_at: now,
        });
    }

    Ok(())
}

pub(crate) struct TopicDeletionResult {
    pub active_messages: Vec<MessageKey>,
    pub deleted: bool,
}

pub(crate) async fn delete_topic_data(
    pool: &sqlx::SqlitePool,
    key: &TopicKey,
    deleted_at: i64,
) -> Result<Option<TopicDeletionResult>, String> {
    if key.owner_type.is_empty()
        || key.owner_id.is_empty()
        || key.topic_id.is_empty()
        || deleted_at < 0
    {
        return Err("Topic delete requires an exact identity and non-negative deletedAt".into());
    }

    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
    let Some(stored_topic) = sqlx::query(
        "SELECT deleted_at FROM topics
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| e.to_string())?
    else {
        return Ok(None);
    };
    let stored_deleted_at: Option<i64> = stored_topic
        .try_get("deleted_at")
        .map_err(|error| format!("Topic {} tombstone decode failed: {error}", key.topic_id))?;
    if stored_deleted_at.is_some() {
        tx.commit().await.map_err(|e| e.to_string())?;
        return Ok(Some(TopicDeletionResult {
            active_messages: Vec::new(),
            deleted: false,
        }));
    }

    let active_ids: Vec<String> = sqlx::query_scalar(
        "SELECT msg_id FROM active_generations
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ?",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .fetch_all(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;

    let deleted = sqlx::query(
        "UPDATE topics SET deleted_at = ?
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL",
    )
    .bind(deleted_at)
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;
    if deleted.rows_affected() != 1 {
        return Err(format!("Topic {} disappeared during delete", key.topic_id));
    }

    // 级联将该话题下的所有消息标记为逻辑删除
    sqlx::query(
        "UPDATE messages SET deleted_at = ?
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL",
    )
    .bind(deleted_at)
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;

    sqlx::query(
        "DELETE FROM message_attachments
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;

    sqlx::query(
        "DELETE FROM render_cache
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;

    // 级联清除活跃生成注册表，杜绝已删除消息复活
    sqlx::query(
        "DELETE FROM active_generations
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;
    match key.owner_type.as_str() {
        "agent" => HashAggregator::bubble_agent_hash(&mut tx, &key.owner_id).await?,
        "group" => HashAggregator::bubble_group_hash(&mut tx, &key.owner_id).await?,
        other => {
            return Err(format!(
                "Topic {} has unsupported owner_type {other}",
                key.topic_id
            ));
        }
    }
    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(Some(TopicDeletionResult {
        active_messages: active_ids
            .into_iter()
            .map(|msg_id| MessageKey::new(key.clone(), msg_id))
            .collect(),
        deleted: true,
    }))
}

#[tauri::command]
pub async fn update_topic_title(
    _app_handle: AppHandle,
    db_state: State<'_, DbState>,
    owner_id: String,
    owner_type: String,
    topic_id: String,
    title: String,
    expected_title: Option<String>,
) -> Result<bool, String> {
    let now = crate::vcp_modules::infra::utils::now_millis();

    let mut tx = db_state.pool.begin().await.map_err(|e| e.to_string())?;
    let changed = sqlx::query(
        "UPDATE topics SET title = ?, updated_at = ?
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
           AND deleted_at IS NULL AND title IS NOT ?
           AND (? IS NULL OR title = ?)",
    )
    .bind(&title)
    .bind(now)
    .bind(&owner_type)
    .bind(&owner_id)
    .bind(&topic_id)
    .bind(&title)
    .bind(&expected_title)
    .bind(&expected_title)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;

    if changed.rows_affected() == 1 {
        let key = TopicKey::new(owner_type, owner_id, topic_id);
        HashAggregator::bubble_from_topic(&mut tx, &key).await?;
    }
    tx.commit().await.map_err(|e| e.to_string())?;

    Ok(changed.rows_affected() == 1)
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
    owner_id: String,
    owner_type: String,
    topic_id: String,
    locked: bool,
) -> Result<(), String> {
    let now = crate::vcp_modules::infra::utils::now_millis();

    let mut tx = db_state.pool.begin().await.map_err(|e| e.to_string())?;
    let changed = sqlx::query(
        "UPDATE topics SET locked = ?, updated_at = ?
         WHERE owner_type = 'agent' AND owner_id = ? AND topic_id = ?
           AND deleted_at IS NULL AND locked IS NOT ?",
    )
    .bind(locked)
    .bind(now)
    .bind(&owner_id)
    .bind(&topic_id)
    .bind(locked)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;

    if changed.rows_affected() == 1 {
        let key = TopicKey::new(owner_type, owner_id, topic_id);
        HashAggregator::bubble_from_topic(&mut tx, &key).await?;
    }
    tx.commit().await.map_err(|e| e.to_string())?;

    Ok(())
}

#[tauri::command]
pub async fn set_topic_unread(
    _app_handle: AppHandle,
    db_state: State<'_, DbState>,
    owner_id: String,
    owner_type: String,
    topic_id: String,
    unread: bool,
) -> Result<(), String> {
    let key = TopicKey::new(owner_type, owner_id, topic_id);
    set_topic_unread_in_pool(&db_state.pool, &key, unread).await
}

fn validate_agent_topic_key(key: &TopicKey) -> Result<(), String> {
    if key.owner_type != "agent" || !key.is_valid() {
        return Err("Unread state requires an exact Agent Topic identity".to_string());
    }
    Ok(())
}

async fn set_topic_unread_in_pool(
    pool: &sqlx::SqlitePool,
    key: &TopicKey,
    unread: bool,
) -> Result<(), String> {
    validate_agent_topic_key(key)?;
    let unread_int = if unread { 1 } else { 0 };
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;

    if !unread {
        sqlx::query(
            "UPDATE topics SET unread_count = 0
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
               AND deleted_at IS NULL AND unread_count != 0",
        )
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    }

    let changed = sqlx::query(
        "UPDATE topics SET unread = ?, updated_at = ?
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
           AND deleted_at IS NULL AND unread IS NOT ?",
    )
    .bind(unread_int)
    .bind(crate::vcp_modules::infra::utils::now_millis())
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(unread_int)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;

    if changed.rows_affected() == 1 {
        HashAggregator::bubble_from_topic(&mut tx, key).await?;
    }
    tx.commit().await.map_err(|e| e.to_string())?;

    Ok(())
}

#[tauri::command]
pub async fn increment_topic_unread_count(
    db_state: State<'_, DbState>,
    owner_id: String,
    owner_type: String,
    topic_id: String,
) -> Result<i32, String> {
    let key = TopicKey::new(owner_type, owner_id, topic_id);
    validate_agent_topic_key(&key)?;

    let mut tx = db_state.pool.begin().await.map_err(|e| e.to_string())?;
    let incremented = sqlx::query(
        "UPDATE topics SET unread_count = unread_count + 1
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;
    if incremented.rows_affected() != 1 {
        return Err(format!(
            "Agent Topic {}/{}/{} is missing or deleted",
            key.owner_type, key.owner_id, key.topic_id
        ));
    }

    let promoted = sqlx::query(
        "UPDATE topics SET unread = 1, updated_at = ?
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
           AND deleted_at IS NULL AND unread IS NOT 1",
    )
    .bind(crate::vcp_modules::infra::utils::now_millis())
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;
    if promoted.rows_affected() == 1 {
        HashAggregator::bubble_from_topic(&mut tx, &key).await?;
    }

    let unread_count = sqlx::query_scalar::<_, i32>(
        "SELECT unread_count FROM topics
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;

    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(unread_count)
}

#[derive(serde::Deserialize, Clone, Debug)]
#[allow(dead_code)] // DORMANT ASSET: floating-assistant archive DTO is not runtime-exposed.
pub struct TempMessage {
    pub role: String,
    pub name: Option<String>,
    pub content: String,
    pub timestamp: u64,
}

#[tauri::command]
#[allow(dead_code)] // DORMANT ASSET: floating-assistant archive command is not registered.
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
        default_title.clone(),
    )
    .await?;

    let new_topic_id = topic.id;
    let key = TopicKey::new(&owner_type, &owner_id, &new_topic_id);

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
            updated_at: None,
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
            &key,
            &render_content,
            true, // 循环中先不重算全局 Topic 聚合哈希以加速入库
        )
        .await?;
    }

    // 3. 提交事务前冒泡重算当前 Topic 聚合哈希
    crate::vcp_modules::sync_hash::HashAggregator::bubble_from_topic(&mut tx, &key).await?;
    tx.commit().await.map_err(|e| e.to_string())?;

    // 4. 更新数据库中话题的消息计数
    sqlx::query(
        "UPDATE topics SET msg_count = ?
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?",
    )
    .bind(temp_messages.len() as i32)
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
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
            if let Ok(row) =
                sqlx::query("SELECT name FROM agents WHERE owner_type = 'agent' AND agent_id = ?")
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
                Some(default_title),
            )
            .await;
        }
    });

    Ok(new_topic_id)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn regenerate_topic_response(
    app_handle: AppHandle,
    agent_state: State<'_, crate::vcp_modules::agent_service::AgentConfigState>,
    group_state: State<'_, crate::vcp_modules::group_service::GroupManagerState>,
    db_state: State<'_, DbState>,
    active_requests: State<'_, crate::vcp_modules::vcp_client::ActiveRequests>,
    active_group_turns: State<'_, crate::vcp_modules::vcp_client::ActiveGroupTurns>,
    settings_state: State<'_, SettingsState>,
    owner_id: String,
    owner_type: String,
    topic_id: String,
    target_response_msg_id: String,
    stream_channel: Channel<crate::vcp_modules::vcp_client::StreamEvent>,
) -> Result<Value, String> {
    let key = TopicKey::new(&owner_type, &owner_id, &topic_id);
    log::info!(
        "[TopicService] Regenerating response for topic: {}, target msg: {}",
        topic_id,
        target_response_msg_id
    );

    // 1. 以后端完整历史为准，找到目标回复之前最后一条用户消息。
    let pool = &db_state.pool;
    let target_order: (i64, String) = sqlx::query_as(
        "SELECT timestamp, msg_id FROM messages
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?
           AND role = 'assistant' AND deleted_at IS NULL",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(&target_response_msg_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| e.to_string())?
    .ok_or_else(|| format!("Response message {target_response_msg_id} is missing or deleted"))?;

    let row = sqlx::query(
        "SELECT msg_id, content, timestamp, role, name, agent_id, group_id, is_group_message
         FROM messages
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
           AND role = 'user' AND deleted_at IS NULL
           AND (timestamp < ? OR (timestamp = ? AND msg_id < ?))
         ORDER BY timestamp DESC, msg_id DESC
         LIMIT 1",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(target_order.0)
    .bind(target_order.0)
    .bind(&target_order.1)
    .fetch_optional(pool)
    .await
    .map_err(|e| e.to_string())?
    .ok_or_else(|| format!("No user message precedes response {target_response_msg_id}"))?;

    use sqlx::Row;
    let target_user_msg_id: String = row.get("msg_id");
    let user_msg: String = row.get("content");
    let timestamp: i64 = row.get("timestamp");

    // 2. 构造逻辑上的 ChatMessage 对象 (用于传给内部生成函数)
    let chat_msg = crate::vcp_modules::chat_manager::ChatMessage {
        id: target_user_msg_id.clone(),
        role: row.get("role"),
        name: row.get("name"),
        content: user_msg,
        timestamp: timestamp as u64,
        updated_at: None,
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

    // 3. 在写入任何墓碑前完成可确定的生成前检查。检查失败时前端重新加载
    // 历史即可恢复乐观隐藏的旧消息，无需设计墓碑撤销机制。
    let settings =
        crate::vcp_modules::settings_manager::read_settings(app_handle.clone(), settings_state)
            .await?;

    match owner_type.as_str() {
        "agent" => {
            crate::vcp_modules::agent_service::read_agent_config_internal(
                &app_handle,
                &agent_state,
                &owner_id,
                Some(false),
            )
            .await?;
        }
        "group" => {
            let group_config = crate::vcp_modules::group_service::read_group_config(
                app_handle.clone(),
                group_state.clone(),
                owner_id.clone(),
            )
            .await?;
            match group_config.mode.as_str() {
                "sequential" | "naturerandom" => {}
                "invite_only" => {
                    return Err("邀请发言群组不能使用自动重新生成，请直接邀请成员发言".to_string())
                }
                mode => return Err(format!("群组发言模式 {mode} 暂不支持重新生成")),
            }

            let has_live_member: bool = sqlx::query_scalar(
                "SELECT EXISTS(
                    SELECT 1 FROM group_members gm
                    JOIN agents a
                      ON a.owner_type = 'agent' AND a.agent_id = gm.agent_id
                     AND a.deleted_at IS NULL
                    WHERE gm.group_id = ?
                 )",
            )
            .bind(&owner_id)
            .fetch_one(pool)
            .await
            .map_err(|e| e.to_string())?;
            if !has_live_member {
                return Err("群组没有可发言的有效成员".to_string());
            }
        }
        other => return Err(format!("Unsupported conversation owner type: {other}")),
    }

    // 4. 生成前检查通过后，才按稳定顺序截断旧回复。
    let deletion = crate::vcp_modules::message_service::truncate_history_after_message(
        &db_state.pool,
        &key,
        &target_user_msg_id,
    )
    .await?;
    for msg_id in &deletion.active_ids {
        if let Err(error) = active_requests.cancel(&MessageKey::new(key.clone(), msg_id)) {
            log::warn!(
                "Failed to cancel regenerated generation {}: {}",
                msg_id,
                error
            );
        }
    }
    crate::vcp_modules::chat_manager::notify_message_deletions(&app_handle, &key, &deletion);

    // 5. 发起生成。网络失败由既有 Finalizer 落成一条带错误原因的终态消息。
    let generation_result = if owner_type == "agent" {
        crate::vcp_modules::agent_chat_application_service::internal_process_agent_chat_message(
            app_handle,
            agent_state,
            db_state.clone(),
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
        .await?
    } else {
        crate::vcp_modules::group_chat_application_service::internal_process_group_chat_message(
            app_handle,
            group_state,
            agent_state,
            db_state.clone(),
            active_requests,
            active_group_turns,
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
        .await?
    };

    if generation_result["status"].as_str() == Some("no_ai_response") {
        let reason = generation_result["reason"].as_str().unwrap_or("unknown");
        return Err(format!("重新生成未能启动回复: {reason}"));
    }

    let msg_count: i32 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM messages
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .fetch_one(pool)
    .await
    .map_err(|e| e.to_string())?;

    Ok(json!({
        "generation": generation_result,
        "msgCount": msg_count,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    #[tokio::test]
    async fn agent_topic_unread_change_advances_config_and_bubbles_owner() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open test database");
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .expect("run migrations");
        sqlx::query(
            "INSERT INTO agents (agent_id, name, model, updated_at)
             VALUES ('agent', 'Agent', 'model', 1);
             INSERT INTO topics (
                topic_id, owner_type, owner_id, title, created_at, updated_at, unread
             ) VALUES ('topic', 'agent', 'agent', 'Topic', 1, 1, 0);",
        )
        .execute(&pool)
        .await
        .expect("seed owner and topic");
        let mut tx = pool.begin().await.expect("begin hash initialization");
        let key = TopicKey::new("agent", "agent", "topic");
        HashAggregator::bubble_from_topic(&mut tx, &key)
            .await
            .expect("initialize hashes");
        tx.commit().await.expect("commit hash initialization");

        let before: (String, String, String, i64) = sqlx::query_as(
            "SELECT t.config_hash, t.content_hash, a.content_hash, t.updated_at
             FROM topics t JOIN agents a ON a.agent_id = t.owner_id
             WHERE t.topic_id = 'topic'",
        )
        .fetch_one(&pool)
        .await
        .expect("read initial state");

        set_topic_unread_in_pool(&pool, &key, true)
            .await
            .expect("mark topic unread");
        let changed: (String, String, String, i64) = sqlx::query_as(
            "SELECT t.config_hash, t.content_hash, a.content_hash, t.updated_at
             FROM topics t JOIN agents a ON a.agent_id = t.owner_id
             WHERE t.topic_id = 'topic'",
        )
        .fetch_one(&pool)
        .await
        .expect("read changed state");
        assert_ne!(changed.0, before.0);
        assert_eq!(changed.1, before.1);
        assert_ne!(changed.2, before.2);
        assert!(changed.3 > before.3);

        set_topic_unread_in_pool(&pool, &key, true)
            .await
            .expect("repeat unread state");
        let repeated: (String, String, String, i64) = sqlx::query_as(
            "SELECT t.config_hash, t.content_hash, a.content_hash, t.updated_at
             FROM topics t JOIN agents a ON a.agent_id = t.owner_id
             WHERE t.topic_id = 'topic'",
        )
        .fetch_one(&pool)
        .await
        .expect("read repeated state");
        assert_eq!(repeated, changed);
    }
}

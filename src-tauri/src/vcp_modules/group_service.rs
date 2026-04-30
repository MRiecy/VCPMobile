// GroupService: 处理群组(Agent Group)配置与生命周期的核心模块 (IPC 层)
// 职责: 作为 Tauri 命令入口，处理群组业务逻辑，完全面向 SQLite 存储。

use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::group_types::GroupConfig;
use crate::vcp_modules::sync_dto::GroupSyncDTO;
use crate::vcp_modules::sync_hash::HashAggregator;
use crate::vcp_modules::sync_service::{SyncCommand, SyncState};
use crate::vcp_modules::sync_types::SyncDataType;
use crate::vcp_modules::topic_types::Topic;
use dashmap::DashMap;
use std::sync::Arc;
use tauri::{AppHandle, Manager, Runtime, State};
use tokio::sync::Mutex;

/// GroupManagerState 的全局状态
pub struct GroupManagerState {
    /// 配置缓存: group_id -> GroupConfig
    pub caches: DashMap<String, GroupConfig>,
    /// 任务队列锁: group_id -> Mutex
    pub locks: DashMap<String, Arc<Mutex<()>>>,
}

impl GroupManagerState {
    pub fn new() -> Self {
        Self {
            caches: DashMap::new(),
            locks: DashMap::new(),
        }
    }

    pub async fn acquire_lock(&self, group_id: &str) -> Arc<Mutex<()>> {
        self.locks
            .entry(group_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .value()
            .clone()
    }
}

#[tauri::command]
pub async fn read_group_config<R: Runtime>(
    app_handle: AppHandle<R>,
    state: State<'_, GroupManagerState>,
    group_id: String,
) -> Result<GroupConfig, String> {
    read_group_config_internal(&app_handle, &state, &group_id).await
}

pub async fn read_group_config_internal<R: Runtime>(
    app_handle: &AppHandle<R>,
    state: &GroupManagerState,
    group_id: &str,
) -> Result<GroupConfig, String> {
    if let Some(cached) = state.caches.get(group_id) {
        return Ok(cached.value().clone());
    }

    let db_state = app_handle.state::<DbState>();
    let pool = &db_state.pool;

    let group_row: Option<sqlx::sqlite::SqliteRow> = sqlx::query(
        "SELECT g.name, g.mode, g.group_prompt, g.invite_prompt, g.use_unified_model, g.unified_model, g.tag_match_mode, g.created_at, g.current_topic_id, av.dominant_color 
         FROM groups g
         LEFT JOIN avatars av ON av.owner_id = g.group_id AND av.owner_type = 'group'
         WHERE g.group_id = ? AND g.deleted_at IS NULL"
    )
    .bind(group_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| e.to_string())?;

    if let Some(row) = group_row {
        use sqlx::Row;
        let avatar_calculated_color: Option<String> = row.get("dominant_color");
        let current_topic_id: Option<String> = row.get("current_topic_id");

        let member_rows: Vec<sqlx::sqlite::SqliteRow> = sqlx::query(
            "SELECT agent_id, member_tag FROM group_members WHERE group_id = ? ORDER BY sort_order ASC"
        )
        .bind(group_id)
        .fetch_all(pool)
        .await
        .map_err(|e| e.to_string())?;

        let mut members = Vec::new();
        let mut member_tags = serde_json::Map::new();
        for mr in member_rows {
            let aid: String = mr.get("agent_id");
            let tag: Option<String> = mr.get("member_tag");
            members.push(aid.clone());
            if let Some(t) = tag {
                member_tags.insert(aid, serde_json::Value::String(t));
            }
        }

        let topic_rows: Vec<sqlx::sqlite::SqliteRow> = sqlx::query(
            "SELECT topic_id, title, created_at, locked, unread, unread_count, msg_count 
             FROM topics WHERE owner_type = 'group' AND owner_id = ? AND deleted_at IS NULL ORDER BY updated_at DESC"
        )
        .bind(group_id)
        .fetch_all(pool)
        .await
        .map_err(|e| e.to_string())?;

        let mut topics = Vec::new();
        for tr in topic_rows {
            topics.push(Topic {
                id: tr.get("topic_id"),
                name: tr.get("title"),
                created_at: tr.get("created_at"),
                locked: tr.get::<i32, _>("locked") != 0,
                unread: tr.get::<i32, _>("unread") != 0,
                unread_count: tr.get("unread_count"),
                msg_count: tr.get("msg_count"),
                owner_id: group_id.to_string(),
                owner_type: "group".to_string(),
            });
        }

        let config = GroupConfig {
            id: group_id.to_string(),
            name: row.get("name"),
            avatar_calculated_color,
            members,
            mode: row.get("mode"),
            member_tags: Some(serde_json::Value::Object(member_tags)),
            group_prompt: row.get("group_prompt"),
            invite_prompt: row.get("invite_prompt"),
            use_unified_model: row.get::<i32, _>("use_unified_model") != 0,
            unified_model: row.get("unified_model"),
            topics,
            tag_match_mode: row.get("tag_match_mode"),
            created_at: row.get("created_at"),
            current_topic_id,
        };

        state.caches.insert(group_id.to_string(), config.clone());
        return Ok(config);
    }

    Err(format!("Group {} not found", group_id))
}

#[tauri::command]
pub async fn save_group_config(
    app_handle: AppHandle,
    state: State<'_, GroupManagerState>,
    group: GroupConfig,
) -> Result<bool, String> {
    let group_id = if group.id.is_empty() {
        return Err("Group ID cannot be empty".to_string());
    } else {
        group.id.clone()
    };

    let mutex = state.acquire_lock(&group_id).await;
    let _lock = mutex.lock().await;

    internal_write_group_config(&app_handle, &state, &group_id, &group, false, false).await
}

#[tauri::command]
pub async fn get_groups(
    app_handle: AppHandle,
    state: State<'_, GroupManagerState>,
) -> Result<Vec<GroupConfig>, String> {
    let db_state = app_handle.state::<DbState>();
    let pool = &db_state.pool;

    let rows: Vec<sqlx::sqlite::SqliteRow> =
        sqlx::query("SELECT group_id FROM groups WHERE deleted_at IS NULL")
            .fetch_all(pool)
            .await
            .map_err(|e| e.to_string())?;

    let mut groups = Vec::new();
    for row in rows {
        use sqlx::Row;
        let group_id: String = row.get("group_id");
        if let Ok(config) = read_group_config(app_handle.clone(), state.clone(), group_id).await {
            groups.push(config);
        }
    }

    Ok(groups)
}

#[tauri::command]
pub async fn update_group_config(
    app_handle: AppHandle,
    state: State<'_, GroupManagerState>,
    group_id: String,
    updates: serde_json::Value,
) -> Result<GroupConfig, String> {
    let mutex = state.acquire_lock(&group_id).await;
    let _lock = mutex.lock().await;

    let config = read_group_config(app_handle.clone(), state.clone(), group_id.clone()).await?;

    let mut config_val = serde_json::to_value(&config).map_err(|e| e.to_string())?;

    if let Some(updates_obj) = updates.as_object() {
        if let Some(config_obj) = config_val.as_object_mut() {
            for (k, v) in updates_obj {
                config_obj.insert(k.clone(), v.clone());
            }
        }
    }

    let new_config: GroupConfig = serde_json::from_value(config_val).map_err(|e| e.to_string())?;

    internal_write_group_config(&app_handle, &state, &group_id, &new_config, false, false).await?;

    Ok(new_config)
}

#[tauri::command]
pub async fn create_group(
    app_handle: AppHandle,
    state: State<'_, GroupManagerState>,
    name: String,
) -> Result<GroupConfig, String> {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;

    let base_id = name
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
        .collect::<String>();
    let group_id = format!("____{}_{}", base_id, timestamp);

    let default_topic_id = format!("group_topic_{}", timestamp);

    let db_state = app_handle.state::<DbState>();
    let pool = &db_state.pool;

    let mut tx: sqlx::Transaction<'_, sqlx::Sqlite> =
        pool.begin().await.map_err(|e| e.to_string())?;

    let default_topic = Topic {
        id: default_topic_id.clone(),
        name: "主要群聊".to_string(),
        created_at: timestamp,
        locked: true,
        unread: false,
        unread_count: 0,
        msg_count: 0,
        owner_id: group_id.clone(),
        owner_type: "group".to_string(),
    };

    let config = GroupConfig {
        id: group_id.clone(),
        name: name.clone(),
        avatar_calculated_color: None,
        members: vec![],
        mode: "sequential".to_string(),
        member_tags: Some(serde_json::json!({})),
        group_prompt: Some("".to_string()),
        invite_prompt: Some("现在轮到你{{VCPChatAgentName}}发言了。系统已经为大家添加[xxx的发言：]这样的标记头，以用于区分不同发言来自谁。大家不用自己再输出自己的发言标记头，也不需要讨论发言标记系统，正常聊天即可。".to_string()),
        use_unified_model: false,
        unified_model: None,
        topics: vec![default_topic.clone()],
        tag_match_mode: Some("strict".to_string()),
        created_at: timestamp,
        current_topic_id: Some(default_topic_id.clone()),
    };

    let dto = GroupSyncDTO::from(&config);
    let config_hash = HashAggregator::compute_group_config_hash(&dto);

    sqlx::query(
        "INSERT INTO groups (group_id, name, created_at, updated_at, mode, use_unified_model, config_hash, current_topic_id) VALUES (?, ?, ?, ?, 'sequential', 0, ?, ?)"
    )
    .bind(&group_id)
    .bind(&name)
    .bind(timestamp)
    .bind(timestamp)
    .bind(&config_hash)
    .bind(&config.current_topic_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;

    for topic in &config.topics {
        sqlx::query(
            "INSERT INTO topics (topic_id, owner_type, owner_id, title, created_at, updated_at) 
             VALUES (?, 'group', ?, ?, ?, ?)",
        )
        .bind(&topic.id)
        .bind(&group_id)
        .bind(&topic.name)
        .bind(topic.created_at)
        .bind(timestamp)
        .bind(timestamp)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    }

    tx.commit().await.map_err(|e| e.to_string())?;

    // 触发聚合哈希冒泡
    let mut bubble_tx = pool.begin().await.map_err(|e| e.to_string())?;
    HashAggregator::bubble_group_hash(&mut bubble_tx, &group_id).await?;
    bubble_tx.commit().await.map_err(|e| e.to_string())?;

    state.caches.insert(group_id, config.clone());
    Ok(config)
}

async fn internal_write_group_config<R: Runtime>(
    app_handle: &AppHandle<R>,
    state: &GroupManagerState,
    group_id: &str,
    config: &GroupConfig,
    skip_bubble: bool,
    from_sync: bool,
) -> Result<bool, String> {
    let db_state = app_handle.state::<DbState>();
    let pool = &db_state.pool;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;

    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;

    let dto = GroupSyncDTO::from(config);
    let config_hash = HashAggregator::compute_group_config_hash(&dto);

    // 只有非同步来源且哈希发生变化时，才通知同步中心
    if !from_sync {
        if let Some(sync_state) = app_handle.try_state::<SyncState>() {
            let rows = sqlx::query("SELECT config_hash FROM groups WHERE group_id = ?")
                .bind(group_id)
                .fetch_optional(pool)
                .await
                .map_err(|e| e.to_string())?;

            let old_hash = rows.and_then(|r| {
                use sqlx::Row;
                r.get::<Option<String>, _>("config_hash")
            });

            if old_hash.as_ref() != Some(&config_hash) {
                let _ = sync_state.ws_sender.send(SyncCommand::NotifyLocalChange {
                    id: group_id.to_string(),
                    data_type: SyncDataType::Group,
                    hash: config_hash.clone(),
                    ts: now,
                });
            }
        }
    }

    sqlx::query(
        "INSERT INTO groups (
            group_id, name, mode, 
            group_prompt, invite_prompt, use_unified_model, unified_model, 
            tag_match_mode, created_at, config_hash, current_topic_id, updated_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(group_id) DO UPDATE SET
            name = excluded.name,
            mode = excluded.mode,
            group_prompt = excluded.group_prompt,
            invite_prompt = excluded.invite_prompt,
            use_unified_model = excluded.use_unified_model,
            unified_model = excluded.unified_model,
            tag_match_mode = excluded.tag_match_mode,
            config_hash = excluded.config_hash,
            current_topic_id = excluded.current_topic_id,
            updated_at = excluded.updated_at",
    )
    .bind(group_id)
    .bind(&config.name)
    .bind(&config.mode)
    .bind(&config.group_prompt)
    .bind(&config.invite_prompt)
    .bind(if config.use_unified_model { 1 } else { 0 })
    .bind(&config.unified_model)
    .bind(&config.tag_match_mode)
    .bind(config.created_at)
    .bind(&config_hash)
    .bind(&config.current_topic_id)
    .bind(now)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;

    sqlx::query("DELETE FROM group_members WHERE group_id = ?")
        .bind(group_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

    let member_tags = config.member_tags.as_ref().and_then(|v| v.as_object());

    for (idx, agent_id) in config.members.iter().enumerate() {
        let tag = member_tags
            .and_then(|m| m.get(agent_id))
            .and_then(|v| v.as_str());
        sqlx::query(
            "INSERT INTO group_members (
                group_id, agent_id, member_tag, sort_order, updated_at
            ) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(group_id)
        .bind(agent_id)
        .bind(tag)
        .bind(idx as i32)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    }

    for topic in &config.topics {
        sqlx::query(
            "INSERT INTO topics (
                topic_id, owner_type, owner_id, title,
                created_at, updated_at, locked, unread
            ) VALUES (?, 'group', ?, ?, ?, ?, ?, ?)
             ON CONFLICT(topic_id) DO UPDATE SET
                title = excluded.title,
                locked = excluded.locked,
                unread = excluded.unread,
                updated_at = excluded.updated_at",
        )
        .bind(&topic.id)
        .bind(group_id)
        .bind(&topic.name)
        .bind(topic.created_at)
        .bind(now)
        .bind(topic.locked)
        .bind(topic.unread)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    }

    tx.commit().await.map_err(|e| e.to_string())?;

    // 触发聚合哈希冒泡
    if !skip_bubble {
        let mut bubble_tx = pool.begin().await.map_err(|e| e.to_string())?;
        HashAggregator::bubble_group_hash(&mut bubble_tx, group_id).await?;
        bubble_tx.commit().await.map_err(|e| e.to_string())?;
    }

    // 通知同步中心：本地数据已变动 (已由上面的 config_hash 比对逻辑处理)

    state.caches.insert(group_id.to_string(), config.clone());

    Ok(true)
}

#[tauri::command]
pub async fn delete_group(
    app_handle: AppHandle,
    state: State<'_, GroupManagerState>,
    group_id: String,
) -> Result<bool, String> {
    let db_state = app_handle.state::<DbState>();
    let pool = &db_state.pool;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;

    sqlx::query("UPDATE groups SET deleted_at = ? WHERE group_id = ?")
        .bind(now)
        .bind(&group_id)
        .execute(pool)
        .await
        .map_err(|e| e.to_string())?;

    state.caches.remove(&group_id);
    state.locks.remove(&group_id);

    if let Some(sync_state) = app_handle.try_state::<SyncState>() {
        let _ = sync_state.ws_sender.send(SyncCommand::NotifyDelete {
            data_type: SyncDataType::Group,
            id: group_id.clone(),
        });
    }

    Ok(true)
}

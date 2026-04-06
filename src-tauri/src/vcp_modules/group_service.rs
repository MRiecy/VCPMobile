// GroupService: 处理群组(Agent Group)配置与生命周期的核心模块 (IPC 层)
// 职责: 作为 Tauri 命令入口，处理群组业务逻辑，完全面向 SQLite 存储。

use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::group_types::GroupConfig;
use crate::vcp_modules::storage_paths::get_groups_base_path;
use crate::vcp_modules::topic_list_manager::Topic;
use dashmap::DashMap;
use std::sync::Arc;
use tauri::{AppHandle, Manager, State};
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

/// 路径转换辅助: 针对群组头像
fn resolve_group_avatar_path(app: &AppHandle, config: &mut GroupConfig) {
    if let Some(avatar) = &mut config.avatar {
        if !avatar.contains('/') && !avatar.contains('\\') {
            let mut path = get_groups_base_path(app);
            path.push(&config.id);
            path.push(&avatar);
            *avatar = path.to_string_lossy().replace("\\", "/");
        } else if avatar.contains("AppData/AgentGroups") || avatar.contains("AppData\\AgentGroups")
        {
            let config_dir = app.path().app_config_dir().unwrap_or_default();
            let config_dir_str = config_dir.to_string_lossy().replace("\\", "/");
            let parts: Vec<&str> = avatar.split(&['/', '\\'][..]).collect();
            if let Some(idx) = parts.iter().position(|&r| r == "AgentGroups") {
                let relative_path = parts[idx + 1..].join("/");
                *avatar = format!("{}/AgentGroups/{}", config_dir_str, relative_path);
            }
        }
    } else {
        let base_path = get_groups_base_path(app).join(&config.id);
        let extensions = ["png", "jpg", "jpeg", "webp", "gif"];
        for ext in extensions {
            let avatar_path = base_path.join(format!("avatar.{}", ext));
            if avatar_path.exists() {
                config.avatar = Some(avatar_path.to_string_lossy().replace("\\", "/"));
                break;
            }
        }
    }
}

#[tauri::command]
pub async fn read_group_config(
    app_handle: AppHandle,
    state: State<'_, GroupManagerState>,
    group_id: String,
) -> Result<GroupConfig, String> {
    if let Some(cached) = state.caches.get(&group_id) {
        let mut config = cached.value().clone();
        resolve_group_avatar_path(&app_handle, &mut config);
        return Ok(config);
    }

    let db_state = app_handle.state::<DbState>();
    let pool = &db_state.pool;

    let group_row = sqlx::query(
        "SELECT name, avatar, avatar_calculated_color, mode, group_prompt, invite_prompt, use_unified_model, unified_model, tag_match_mode, extra_json 
         FROM groups WHERE group_id = ? AND deleted_at IS NULL"
    )
    .bind(&group_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| e.to_string())?;

    if let Some(row) = group_row {
        use sqlx::Row;
        let extra: serde_json::Map<String, serde_json::Value> =
            if let Some(ej) = row.get::<Option<String>, _>("extra_json") {
                serde_json::from_str(&ej).unwrap_or_default()
            } else {
                serde_json::Map::new()
            };

        let member_rows = sqlx::query(
            "SELECT agent_id, member_tag FROM group_members WHERE group_id = ? AND deleted_at IS NULL ORDER BY sort_order ASC"
        )
        .bind(&group_id)
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

        let topic_rows = sqlx::query(
            "SELECT topic_id, title, created_at, locked, unread, extra_json 
             FROM topics WHERE owner_type = 'group' AND owner_id = ? AND deleted_at IS NULL ORDER BY updated_at DESC"
        )
        .bind(&group_id)
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
                unread_count: 0,
                msg_count: 0,
            });
        }

        let mut config = GroupConfig {
            id: group_id.clone(),
            name: row.get("name"),
            avatar: row.get("avatar"),
            avatar_calculated_color: row.get("avatar_calculated_color"),
            members,
            mode: row.get("mode"),
            member_tags: Some(serde_json::Value::Object(member_tags)),
            group_prompt: row.get("group_prompt"),
            invite_prompt: row.get("invite_prompt"),
            use_unified_model: row.get::<i32, _>("use_unified_model") != 0,
            unified_model: row.get("unified_model"),
            created_at: 0,
            topics,
            tag_match_mode: row.get("tag_match_mode"),
            extra,
        };

        resolve_group_avatar_path(&app_handle, &mut config);
        state.caches.insert(group_id.clone(), config.clone());
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

    internal_write_group_config(&app_handle, &state, &group_id, &group).await
}

#[tauri::command]
pub async fn get_groups(
    app_handle: AppHandle,
    state: State<'_, GroupManagerState>,
) -> Result<Vec<GroupConfig>, String> {
    let db_state = app_handle.state::<DbState>();
    let pool = &db_state.pool;

    let rows = sqlx::query("SELECT group_id FROM groups WHERE deleted_at IS NULL")
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

    internal_write_group_config(&app_handle, &state, &group_id, &new_config).await?;

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

    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;

    sqlx::query(
        "INSERT INTO groups (group_id, name, created_at, updated_at, mode, use_unified_model) VALUES (?, ?, ?, ?, 'sequential', 0)"
    )
    .bind(&group_id)
    .bind(&name)
    .bind(timestamp)
    .bind(timestamp)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;

    sqlx::query(
        "INSERT INTO topics (topic_id, owner_type, owner_id, title, created_at, updated_at) VALUES (?, 'group', ?, ?, ?, ?)"
    )
    .bind(&default_topic_id)
    .bind(&group_id)
    .bind("主要群聊")
    .bind(timestamp)
    .bind(timestamp)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;

    tx.commit().await.map_err(|e| e.to_string())?;

    let default_topic = Topic {
        id: default_topic_id.clone(),
        name: "主要群聊".to_string(),
        created_at: timestamp,
        locked: false,
        unread: false,
        unread_count: 0,
        msg_count: 0,
    };

    let config = GroupConfig {
        id: group_id.clone(),
        name: name.clone(),
        avatar: None,
        avatar_calculated_color: None,
        members: vec![],
        mode: "sequential".to_string(),
        member_tags: Some(serde_json::json!({})),
        group_prompt: Some("".to_string()),
        invite_prompt: Some("现在轮到你{{VCPChatAgentName}}发言了...".to_string()),
        use_unified_model: false,
        unified_model: None,
        created_at: timestamp,
        topics: vec![default_topic.clone()],
        tag_match_mode: Some("strict".to_string()),
        extra: serde_json::Map::new(),
    };

    state.caches.insert(group_id, config.clone());
    Ok(config)
}

async fn internal_write_group_config(
    _app_handle: &AppHandle,
    state: &GroupManagerState,
    group_id: &str,
    config: &GroupConfig,
) -> Result<bool, String> {
    let db_state = _app_handle.state::<DbState>();
    let pool = &db_state.pool;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;

    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;

    let extra_json = serde_json::to_value(&config.extra)
        .ok()
        .map(|v| v.to_string());

    sqlx::query(
        "INSERT INTO groups (
            group_id, name, avatar, avatar_calculated_color, mode, 
            group_prompt, invite_prompt, use_unified_model, unified_model, 
            tag_match_mode, extra_json, created_at, updated_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(group_id) DO UPDATE SET
            name = excluded.name,
            avatar = excluded.avatar,
            avatar_calculated_color = excluded.avatar_calculated_color,
            mode = excluded.mode,
            group_prompt = excluded.group_prompt,
            invite_prompt = excluded.invite_prompt,
            use_unified_model = excluded.use_unified_model,
            unified_model = excluded.unified_model,
            tag_match_mode = excluded.tag_match_mode,
            extra_json = excluded.extra_json,
            updated_at = excluded.updated_at",
    )
    .bind(group_id)
    .bind(&config.name)
    .bind(&config.avatar)
    .bind(&config.avatar_calculated_color)
    .bind(&config.mode)
    .bind(&config.group_prompt)
    .bind(&config.invite_prompt)
    .bind(if config.use_unified_model { 1 } else { 0 })
    .bind(&config.unified_model)
    .bind(&config.tag_match_mode)
    .bind(extra_json)
    .bind(now)
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
                group_id, agent_id, member_tag, sort_order, created_at, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(group_id)
        .bind(agent_id)
        .bind(tag)
        .bind(idx as i32)
        .bind(now)
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

    state.caches.insert(group_id.to_string(), config.clone());

    Ok(true)
}

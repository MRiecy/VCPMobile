// GroupService: 处理群组(Agent Group)配置与生命周期的核心模块 (IPC 层)
// 职责: 作为 Tauri 命令入口，处理群组业务逻辑，完全面向 SQLite 存储。

use crate::vcp_modules::db_manager::{begin_immediate_write, DbState};
use crate::vcp_modules::group_types::{
    parse_member_tags, serialize_member_tags, GroupConfig, MemberTags,
};
use crate::vcp_modules::infra::utils::desktop_compatible_id_base;
use crate::vcp_modules::sync_dto::GroupSyncDTO;
use crate::vcp_modules::sync_hash::HashAggregator;
use crate::vcp_modules::sync_service::{SyncCommand, SyncState};
use crate::vcp_modules::sync_types::{DeleteTarget, OwnerType, SYNC_TOMBSTONE_HASH};
use crate::vcp_modules::topic_types::{MessageKey, Topic, TopicKey};
use dashmap::DashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as CacheCommitMutex};
use tauri::{AppHandle, Manager, Runtime, State};
use tokio::sync::Mutex;

/// GroupManagerState 的全局状态
pub struct GroupManagerState {
    /// 配置缓存: group_id -> GroupConfig
    pub caches: DashMap<String, GroupConfig>,
    /// 任务队列锁: group_id -> Mutex
    pub locks: DashMap<String, Arc<Mutex<()>>>,
    /// 同步写绕过 Facade 时推进的缓存代次，阻止旧数据库快照迟到回填。
    cache_generation: AtomicU64,
    /// 只保护 generation 校验与 DashMap 提交/清空，不跨任何 await。
    cache_commit: CacheCommitMutex<()>,
}

impl GroupManagerState {
    pub fn new() -> Self {
        Self {
            caches: DashMap::new(),
            locks: DashMap::new(),
            cache_generation: AtomicU64::new(0),
            cache_commit: CacheCommitMutex::new(()),
        }
    }

    pub async fn acquire_lock(&self, group_id: &str) -> Arc<Mutex<()>> {
        self.locks
            .entry(group_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .value()
            .clone()
    }

    fn current_cache_generation(&self) -> u64 {
        self.cache_generation.load(Ordering::Acquire)
    }

    fn insert_cache_if_current(&self, group_id: String, config: GroupConfig, generation: u64) {
        let _commit_guard = self
            .cache_commit
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.cache_generation.load(Ordering::Acquire) == generation {
            self.caches.insert(group_id, config);
        }
    }

    pub fn invalidate_cache(&self) {
        let _commit_guard = self
            .cache_commit
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.caches.clear();
        self.cache_generation.fetch_add(1, Ordering::Release);
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
    let mutex = state.acquire_lock(group_id).await;
    let _lock = mutex.lock().await;
    read_group_config_locked(app_handle, state, group_id).await
}

async fn read_group_config_locked<R: Runtime>(
    app_handle: &AppHandle<R>,
    state: &GroupManagerState,
    group_id: &str,
) -> Result<GroupConfig, String> {
    if let Some(cached) = state.caches.get(group_id) {
        return Ok(cached.value().clone());
    }
    let cache_generation = state.current_cache_generation();

    let db_state = app_handle.state::<DbState>();
    let pool = &db_state.pool;

    let group_row: Option<sqlx::sqlite::SqliteRow> = sqlx::query(
        "SELECT g.name, g.mode, g.group_prompt, g.invite_prompt, g.use_unified_model, g.unified_model, g.tag_match_mode, g.member_tags, g.created_at, av.dominant_color
         FROM groups g
         LEFT JOIN avatars av ON av.owner_id = g.group_id AND av.owner_type = 'group' AND av.deleted_at IS NULL
         WHERE g.owner_type = 'group' AND g.group_id = ? AND g.deleted_at IS NULL"
    )
    .bind(group_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| e.to_string())?;

    if let Some(row) = group_row {
        use sqlx::Row;
        let avatar_calculated_color: Option<String> = row.get("dominant_color");

        let member_rows: Vec<sqlx::sqlite::SqliteRow> = sqlx::query(
            "SELECT agent_id FROM group_members WHERE group_id = ? ORDER BY sort_order ASC",
        )
        .bind(group_id)
        .fetch_all(pool)
        .await
        .map_err(|e| e.to_string())?;

        let mut members = Vec::new();
        for mr in member_rows {
            let aid: String = mr.get("agent_id");
            members.push(aid);
        }
        let member_tags_raw: String = row.get("member_tags");
        let member_tags = parse_member_tags(&member_tags_raw)
            .map_err(|error| format!("Group {group_id} memberTags decode failed: {error}"))?;

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
            member_tags: Some(member_tags),
            group_prompt: row.get("group_prompt"),
            invite_prompt: row.get("invite_prompt"),
            use_unified_model: row.get::<i32, _>("use_unified_model") != 0,
            unified_model: row.get("unified_model"),
            topics,
            tag_match_mode: row.get("tag_match_mode"),
            created_at: row.get("created_at"),
        };

        state.insert_cache_if_current(group_id.to_string(), config.clone(), cache_generation);
        return Ok(config);
    }

    Err(format!("Group {} not found", group_id))
}

#[tauri::command]
pub async fn save_group_config(
    app_handle: AppHandle,
    state: State<'_, GroupManagerState>,
    group: GroupConfig,
) -> Result<GroupConfig, String> {
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
    _state: State<'_, GroupManagerState>,
) -> Result<Vec<GroupConfig>, String> {
    let start_total = std::time::Instant::now();
    let db_state = app_handle.state::<DbState>();
    let pool = &db_state.pool;

    let start_groups = std::time::Instant::now();
    // 1. 查询所有未删除群组的基础配置及 avatars 主色
    let group_rows = sqlx::query(
        "SELECT g.group_id, g.name, g.mode, g.group_prompt, g.invite_prompt, g.use_unified_model, g.unified_model, g.tag_match_mode, g.member_tags, g.created_at, av.dominant_color
         FROM groups g
         LEFT JOIN avatars av ON av.owner_id = g.group_id AND av.owner_type = 'group' AND av.deleted_at IS NULL
         WHERE g.owner_type = 'group' AND g.deleted_at IS NULL"
    )
    .fetch_all(pool)
    .await
    .map_err(|e| e.to_string())?;
    let duration_groups = start_groups.elapsed();

    if group_rows.is_empty() {
        log::info!(
            "[Profile] get_groups total: {}ms (empty)",
            start_total.elapsed().as_millis()
        );
        return Ok(Vec::new());
    }

    let start_members = std::time::Instant::now();
    // 2. 一次性查询所有群组成员
    let member_rows = sqlx::query(
        "SELECT group_id, agent_id
         FROM group_members 
         ORDER BY group_id, sort_order ASC",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| e.to_string())?;
    let duration_members = start_members.elapsed();

    let start_mapping = std::time::Instant::now();
    use std::collections::HashMap;

    // 分组整理 members；完整 memberTags 由 groups 表直接保存。
    let mut group_members: HashMap<String, Vec<String>> = HashMap::new();

    for mr in member_rows {
        use sqlx::Row;
        let gid: String = mr.get("group_id");
        let aid: String = mr.get("agent_id");

        group_members.entry(gid).or_default().push(aid);
    }

    // 组装列表；实体缓存只允许在 per-ID 锁内填充。
    let mut groups = Vec::new();
    for row in group_rows {
        use sqlx::Row;
        let group_id: String = row.get("group_id");
        let avatar_calculated_color: Option<String> = row.get("dominant_color");

        let members = group_members.remove(&group_id).unwrap_or_default();
        let member_tags_raw: String = row.get("member_tags");
        let member_tags = parse_member_tags(&member_tags_raw)
            .map_err(|error| format!("Group {group_id} memberTags decode failed: {error}"))?;

        let config = GroupConfig {
            id: group_id.clone(),
            name: row.get("name"),
            avatar_calculated_color,
            members,
            mode: row.get("mode"),
            member_tags: Some(member_tags),
            group_prompt: row.get("group_prompt"),
            invite_prompt: row.get("invite_prompt"),
            use_unified_model: row.get::<i32, _>("use_unified_model") != 0,
            unified_model: row.get("unified_model"),
            topics: vec![], // 优化：话题改为前端选中时流式懒加载，初始置为空
            tag_match_mode: row.get("tag_match_mode"),
            created_at: row.get("created_at"),
        };

        groups.push(config);
    }
    let duration_mapping = start_mapping.elapsed();

    log::info!(
        "[Profile] get_groups finished. Total: {}ms | SQL Groups: {}ms | SQL Members: {}ms | Map & Cache: {}ms",
        start_total.elapsed().as_millis(),
        duration_groups.as_millis(),
        duration_members.as_millis(),
        duration_mapping.as_millis()
    );

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

    let config = read_group_config_locked(&app_handle, &state, &group_id).await?;

    let mut config_val = serde_json::to_value(&config).map_err(|e| e.to_string())?;

    if let Some(updates_obj) = updates.as_object() {
        if let Some(config_obj) = config_val.as_object_mut() {
            for (k, v) in updates_obj {
                config_obj.insert(k.clone(), v.clone());
            }
        }
    }

    let new_config: GroupConfig = serde_json::from_value(config_val).map_err(|e| e.to_string())?;

    internal_write_group_config(&app_handle, &state, &group_id, &new_config).await
}

#[tauri::command]
pub async fn create_group(
    app_handle: AppHandle,
    state: State<'_, GroupManagerState>,
    name: String,
) -> Result<GroupConfig, String> {
    let cache_generation = state.current_cache_generation();
    let timestamp = crate::vcp_modules::infra::utils::now_millis();

    let base_id = desktop_compatible_id_base(&name);
    let group_id = format!("{}_{}", base_id, timestamp);

    let default_topic_id = format!("group_topic_{}", timestamp);

    let db_state = app_handle.state::<DbState>();

    let (write_permit, mut tx) = db_state.begin_write("identity.group.create").await?;

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
        member_tags: Some(MemberTags::new()),
        group_prompt: Some("".to_string()),
        invite_prompt: Some("现在轮到你{{VCPChatAgentName}}发言了。系统已经为大家添加[xxx的发言：]这样的标记头，以用于区分不同发言来自谁。大家不用自己再输出自己的发言标记头，也不需要讨论发言标记系统，正常聊天即可。".to_string()),
        use_unified_model: false,
        unified_model: Some("".to_string()),
        topics: vec![default_topic.clone()],
        tag_match_mode: Some("strict".to_string()),
        created_at: timestamp,
    };

    let dto = GroupSyncDTO::from(&config);
    let config_hash = HashAggregator::compute_group_config_hash(&dto);
    let member_tags = serialize_member_tags(config.member_tags.as_ref())
        .map_err(|error| format!("Group {group_id} memberTags encode failed: {error}"))?;

    sqlx::query(
        "INSERT INTO groups (
            owner_type, group_id, name, mode,
            group_prompt, invite_prompt, use_unified_model, unified_model,
            tag_match_mode, member_tags, created_at, config_hash, updated_at
         ) VALUES ('group', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&group_id)
    .bind(&name)
    .bind(&config.mode)
    .bind(&config.group_prompt)
    .bind(&config.invite_prompt)
    .bind(if config.use_unified_model { 1 } else { 0 })
    .bind(&config.unified_model)
    .bind(&config.tag_match_mode)
    .bind(member_tags)
    .bind(timestamp)
    .bind(&config_hash)
    .bind(timestamp)
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
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

        // 初始化 Topic 自身哈希
        let key = TopicKey::new("group", &group_id, &topic.id);
        HashAggregator::bubble_topic_hash(&mut tx, &key).await?;
    }

    // 触发聚合哈希冒泡
    HashAggregator::bubble_group_hash(&mut tx, &group_id).await?;
    tx.commit().await.map_err(|e| e.to_string())?;
    drop(write_permit);

    state.insert_cache_if_current(group_id, config.clone(), cache_generation);
    Ok(config)
}

async fn internal_write_group_config<R: Runtime>(
    app_handle: &AppHandle<R>,
    state: &GroupManagerState,
    group_id: &str,
    config: &GroupConfig,
) -> Result<GroupConfig, String> {
    let cache_generation = state.current_cache_generation();
    let db_state = app_handle.state::<DbState>();

    let now = crate::vcp_modules::infra::utils::now_millis();

    let (write_permit, mut tx) = db_state.begin_write("identity.group.save").await?;

    let deleted_at = sqlx::query_scalar::<_, Option<i64>>(
        "SELECT deleted_at FROM groups WHERE owner_type = 'group' AND group_id = ?",
    )
    .bind(group_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;
    if matches!(deleted_at, Some(Some(_))) {
        tx.commit().await.map_err(|e| e.to_string())?;
        drop(write_permit);
        return Err(format!("Group {group_id} has been deleted"));
    }

    // 同步阶段允许 Group 比新 Agent 更早落库，因此只排除“明确已有墓碑”的成员；
    // 完全缺失的 Agent 仍作为前向引用保留，避免依赖 Phase 1 到达顺序。
    let mut canonical_config = config.clone();
    let mut live_members = Vec::with_capacity(config.members.len());
    for agent_id in &config.members {
        let deleted_at = sqlx::query_scalar::<_, Option<i64>>(
            "SELECT deleted_at FROM agents WHERE agent_id = ?",
        )
        .bind(agent_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
        if !matches!(deleted_at, Some(Some(_))) {
            live_members.push(agent_id.clone());
        }
    }
    canonical_config.members = live_members;

    let dto = GroupSyncDTO::from(&canonical_config);
    let config_hash = HashAggregator::compute_group_config_hash(&dto);
    let member_tags = serialize_member_tags(canonical_config.member_tags.as_ref())
        .map_err(|error| format!("Group {group_id} memberTags encode failed: {error}"))?;

    sqlx::query(
        "INSERT INTO groups (
            owner_type, group_id, name, mode,
            group_prompt, invite_prompt, use_unified_model, unified_model, 
            tag_match_mode, member_tags, created_at, config_hash, updated_at
        ) VALUES ('group', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(owner_type, group_id) DO UPDATE SET
            name = excluded.name,
            mode = excluded.mode,
            group_prompt = excluded.group_prompt,
            invite_prompt = excluded.invite_prompt,
            use_unified_model = excluded.use_unified_model,
            unified_model = excluded.unified_model,
            tag_match_mode = excluded.tag_match_mode,
            member_tags = excluded.member_tags,
            created_at = excluded.created_at,
            updated_at = CASE
                WHEN groups.config_hash IS NOT excluded.config_hash THEN excluded.updated_at
                ELSE groups.updated_at
            END,
            config_hash = excluded.config_hash",
    )
    .bind(group_id)
    .bind(&canonical_config.name)
    .bind(&canonical_config.mode)
    .bind(&canonical_config.group_prompt)
    .bind(&canonical_config.invite_prompt)
    .bind(if canonical_config.use_unified_model {
        1
    } else {
        0
    })
    .bind(&canonical_config.unified_model)
    .bind(&canonical_config.tag_match_mode)
    .bind(member_tags)
    .bind(canonical_config.created_at)
    .bind(&config_hash)
    .bind(now)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;

    sqlx::query("DELETE FROM group_members WHERE group_id = ?")
        .bind(group_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

    for (idx, agent_id) in canonical_config.members.iter().enumerate() {
        sqlx::query(
            "INSERT INTO group_members (
                group_id, agent_id, sort_order, updated_at
            ) VALUES (?, ?, ?, ?)",
        )
        .bind(group_id)
        .bind(agent_id)
        .bind(idx as i32)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    }

    tx.commit().await.map_err(|e| e.to_string())?;
    drop(write_permit);

    state.insert_cache_if_current(
        group_id.to_string(),
        canonical_config.clone(),
        cache_generation,
    );

    Ok(canonical_config)
}

#[tauri::command]
pub async fn delete_group(
    app_handle: AppHandle,
    state: State<'_, GroupManagerState>,
    group_id: String,
) -> Result<bool, String> {
    let deleted_at = delete_group_internal(&app_handle, &state, &group_id, None).await?;

    if let Some(sync_state) = app_handle.try_state::<SyncState>() {
        let _ = sync_state.ws_sender.send(SyncCommand::NotifyDelete {
            target: DeleteTarget::Owner {
                owner_type: OwnerType::Group,
                owner_id: group_id,
            },
            deleted_at,
        });
    }

    Ok(true)
}

pub async fn delete_group_internal<R: Runtime>(
    app_handle: &AppHandle<R>,
    state: &GroupManagerState,
    group_id: &str,
    requested_deleted_at: Option<i64>,
) -> Result<i64, String> {
    // 与 save/update 共享同一把实体锁，避免迟到写入在删除后重新填充 cache。
    // 锁条目有意保留：删除后若同 ID 被重新创建，仍必须沿用同一所有权串行化。
    let mutex = state.acquire_lock(group_id).await;
    let _lock = mutex.lock().await;

    let db_state = app_handle.state::<DbState>();
    let pool = &db_state.pool;
    let now = requested_deleted_at.unwrap_or_else(crate::vcp_modules::infra::utils::now_millis);
    if now < 0 {
        return Err("Group delete requires a non-negative deletedAt".to_string());
    }
    let writer_kind = if requested_deleted_at.is_some() {
        "sync.delete.group"
    } else {
        "identity.group.delete"
    };
    let write_permit = db_state.write_gate.acquire(writer_kind).await?;
    let mut tx = begin_immediate_write(pool)
        .await
        .map_err(|e| e.to_string())?;

    let existing_deleted_at: Option<Option<i64>> = sqlx::query_scalar(
        "SELECT deleted_at FROM groups WHERE owner_type = 'group' AND group_id = ?",
    )
    .bind(group_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;
    let tombstone_only = match existing_deleted_at {
        None if requested_deleted_at.is_some() => {
            sqlx::query(
                "INSERT INTO groups (
                    owner_type, group_id, name, config_hash, updated_at, deleted_at
                 ) VALUES ('group', ?, '', ?, ?, ?)",
            )
            .bind(group_id)
            .bind(SYNC_TOMBSTONE_HASH)
            .bind(now)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
            true
        }
        None => {
            tx.commit().await.map_err(|e| e.to_string())?;
            drop(write_permit);
            return Err(format!("Group {group_id} does not exist"));
        }
        Some(Some(existing)) => {
            tx.commit().await.map_err(|e| e.to_string())?;
            drop(write_permit);
            state.caches.remove(group_id);
            return Ok(existing);
        }
        Some(None) => false,
    };

    if !tombstone_only {
        let group_delete = sqlx::query(
            "UPDATE groups SET deleted_at = ?
                 WHERE owner_type = 'group' AND group_id = ? AND deleted_at IS NULL",
        )
        .bind(now)
        .bind(group_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
        if group_delete.rows_affected() != 1 {
            return Err(format!("Group {group_id} disappeared during delete"));
        }
    }

    // 级联将该 Group 下的所有话题标记为逻辑删除
    sqlx::query("UPDATE topics SET deleted_at = ? WHERE owner_id = ? AND owner_type = 'group' AND deleted_at IS NULL")
        .bind(now)
        .bind(group_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

    // 级联将该 Group 下所有话题的所有消息标记为逻辑删除
    sqlx::query(
        "UPDATE messages SET deleted_at = ?
         WHERE owner_type = 'group' AND owner_id = ? AND deleted_at IS NULL",
    )
    .bind(now)
    .bind(group_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;

    // group_members 不是墓碑实体；父 Group 删除后不再保留无消费者的成员关系。
    sqlx::query("DELETE FROM group_members WHERE group_id = ?")
        .bind(group_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

    sqlx::query(
        "DELETE FROM message_attachments
         WHERE owner_type = 'group' AND owner_id = ?",
    )
    .bind(group_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;

    sqlx::query("DELETE FROM render_cache WHERE owner_type = 'group' AND owner_id = ?")
        .bind(group_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

    sqlx::query(
        "UPDATE avatars SET deleted_at = ?
         WHERE owner_type = 'group' AND owner_id = ? AND deleted_at IS NULL",
    )
    .bind(now)
    .bind(group_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;

    let active_keys: Vec<MessageKey> = sqlx::query_as::<_, (String, String)>(
        "SELECT topic_id, msg_id FROM active_generations
         WHERE owner_type = 'group' AND owner_id = ?",
    )
    .bind(group_id)
    .fetch_all(&mut *tx)
    .await
    .map_err(|e| e.to_string())?
    .into_iter()
    .map(|(topic_id, msg_id)| MessageKey::new(TopicKey::new("group", group_id, topic_id), msg_id))
    .collect();

    // 级联清除该 Group 下的所有活跃生成，杜绝已删除消息复活
    sqlx::query("DELETE FROM active_generations WHERE owner_id = ? AND owner_type = 'group'")
        .bind(group_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

    tx.commit().await.map_err(|e| e.to_string())?;
    drop(write_permit);

    if let Some(active_requests) =
        app_handle.try_state::<crate::vcp_modules::vcp_client::ActiveRequests>()
    {
        for key in active_keys {
            if let Err(error) = active_requests.cancel(&key) {
                log::warn!(
                    "Failed to cancel generation {} after deleting Group {}: {}",
                    key.msg_id,
                    group_id,
                    error
                );
            }
        }
    }

    state.caches.remove(group_id);
    Ok(now)
}

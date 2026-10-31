// GroupService: 处理群组(Agent Group)配置与生命周期的核心模块 (IPC 层)
// 职责: 作为 Tauri 命令入口，处理群组业务逻辑，完全面向 SQLite 存储。

use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::group_types::{
    parse_group_local_settings, parse_member_tags, serialize_member_tags, GroupConfig,
    GroupLocalSettings, GroupLocalSettingsPatch, MemberTags,
};
use crate::vcp_modules::infra::utils::desktop_compatible_id_base;
use crate::vcp_modules::sync_dto::GroupSyncDTO;
use crate::vcp_modules::sync_hash::HashAggregator;
use crate::vcp_modules::sync_service::{SyncCommand, SyncState};
use crate::vcp_modules::sync_types::{DeleteTarget, OwnerType, SYNC_TOMBSTONE_HASH};
use crate::vcp_modules::topic_types::{MessageKey, Topic, TopicKey};
use dashmap::DashMap;
use sqlx::{Row, SqliteConnection};
use std::collections::BTreeMap;
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

fn canonical_local_settings_json(raw: Option<&str>) -> Result<String, String> {
    serde_json::to_string(&parse_group_local_settings(raw)).map_err(|error| error.to_string())
}

fn stored_local_settings_json(row: &sqlx::sqlite::SqliteRow) -> Result<Option<String>, String> {
    match row.try_get("settings_json") {
        Ok(value) => Ok(value),
        Err(sqlx::Error::ColumnDecode { .. }) => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

async fn stored_group_local_settings_rows<'a, E>(
    executor: E,
    group_id: Option<&str>,
) -> Result<Vec<sqlx::sqlite::SqliteRow>, String>
where
    E: sqlx::Executor<'a, Database = sqlx::Sqlite>,
{
    match group_id {
        Some(group_id) => {
            sqlx::query("SELECT group_id,settings_json FROM group_local_settings WHERE group_id=?")
                .bind(group_id)
                .fetch_all(executor)
                .await
        }
        None => {
            sqlx::query("SELECT group_id,settings_json FROM group_local_settings")
                .fetch_all(executor)
                .await
        }
    }
    .map_err(|error| error.to_string())
}

// Read again under the writer transaction. A normalization never writes the
// earlier read snapshot over a settings import or a local editor's commit.
async fn normalize_stored_group_local_settings(
    connection: &mut SqliteConnection,
    group_id: Option<&str>,
) -> Result<bool, String> {
    let rows = stored_group_local_settings_rows(&mut *connection, group_id).await?;
    let mut changed = false;
    for row in rows {
        let group_id: String = row.try_get("group_id").map_err(|error| error.to_string())?;
        let raw = stored_local_settings_json(&row)?;
        let canonical = canonical_local_settings_json(raw.as_deref())?;
        if Some(canonical.as_str()) == raw.as_deref() {
            continue;
        }
        let result =
            sqlx::query("UPDATE group_local_settings SET settings_json=? WHERE group_id=?")
                .bind(canonical)
                .bind(group_id)
                .execute(&mut *connection)
                .await
                .map_err(|error| error.to_string())?;
        changed |= result.rows_affected() > 0;
    }
    Ok(changed)
}

/// Config decoding persists typed defaults without changing business timestamps
/// or shared sync data. Actual database errors still reach the caller.
pub(crate) async fn normalize_group_local_settings(
    db: &DbState,
    group_id: Option<&str>,
) -> Result<bool, String> {
    let rows = stored_group_local_settings_rows(&db.pool, group_id).await?;
    let mut needed = false;
    for row in rows {
        let raw = stored_local_settings_json(&row)?;
        if Some(canonical_local_settings_json(raw.as_deref())?.as_str()) != raw.as_deref() {
            needed = true;
            break;
        }
    }
    if needed {
        let mut transaction = db
            .write_transaction("identity.group.normalize_settings")
            .await?;
        let changed = normalize_stored_group_local_settings(&mut transaction, group_id).await?;
        transaction
            .commit()
            .await
            .map_err(|error| error.to_string())?;
        return Ok(changed);
    }
    Ok(false)
}

fn stored_group_field<T>(
    row: &sqlx::sqlite::SqliteRow,
    column: &str,
    default: T,
    changed: &mut bool,
) -> Result<T, String>
where
    T: for<'r> sqlx::Decode<'r, sqlx::Sqlite> + sqlx::Type<sqlx::Sqlite>,
{
    match row.try_get(column) {
        Ok(value) => Ok(value),
        Err(sqlx::Error::ColumnDecode { .. }) => {
            *changed = true;
            Ok(default)
        }
        Err(error) => Err(error.to_string()),
    }
}

/// Decode configuration leaves only. Identity, relations and sync metadata are
/// not settings and remain checked by their respective storage owners.
fn decode_group_fields(row: &sqlx::sqlite::SqliteRow) -> Result<(GroupConfig, bool), String> {
    let defaults = GroupConfig::default();
    let mut changed = false;
    let name = stored_group_field(row, "name", defaults.name, &mut changed)?;
    let mode = stored_group_field(row, "mode", defaults.mode, &mut changed)?;
    let group_prompt =
        stored_group_field(row, "group_prompt", defaults.group_prompt, &mut changed)?;
    let invite_prompt =
        stored_group_field(row, "invite_prompt", defaults.invite_prompt, &mut changed)?;
    let unified_model =
        stored_group_field(row, "unified_model", defaults.unified_model, &mut changed)?;
    let unified: i64 = stored_group_field(row, "use_unified_model", 0, &mut changed)?;
    let use_unified_model = match unified {
        0 => false,
        1 => true,
        _ => {
            changed = true;
            defaults.use_unified_model
        }
    };
    let tag_match_mode = stored_group_field(
        row,
        "tag_match_mode",
        defaults.tag_match_mode.clone(),
        &mut changed,
    )?;
    let tag_match_mode = match tag_match_mode.as_deref() {
        None | Some("strict" | "natural") => tag_match_mode,
        _ => {
            changed = true;
            defaults.tag_match_mode
        }
    };
    let raw_tags: String = stored_group_field(row, "member_tags", "{}".into(), &mut changed)?;
    let member_tags = match parse_member_tags(&raw_tags) {
        Ok(tags) => tags,
        Err(_) => {
            changed = true;
            // Each dynamic tag is a string leaf; keep valid neighboring tags.
            crate::vcp_modules::settings_manager::schema::parse_config_json(
                &raw_tags,
                &serde_json::json!({}),
            )
            .as_object()
            .cloned()
            .map(|tags| {
                tags.into_iter()
                    .filter(|(id, _)| !id.is_empty())
                    .map(|(id, value)| (id, value.as_str().unwrap_or_default().to_owned()))
                    .collect()
            })
            .unwrap_or_default()
        }
    };
    Ok((
        GroupConfig {
            name,
            mode,
            group_prompt,
            invite_prompt,
            use_unified_model,
            unified_model,
            tag_match_mode,
            member_tags: Some(member_tags),
            ..GroupConfig::default()
        },
        changed,
    ))
}

async fn stored_group_configuration_rows<'a, E>(
    executor: E,
    group_id: Option<&str>,
) -> Result<Vec<sqlx::sqlite::SqliteRow>, String>
where
    E: sqlx::Executor<'a, Database = sqlx::Sqlite>,
{
    match group_id {
        Some(group_id) => sqlx::query(
            "SELECT * FROM groups WHERE owner_type='group' AND group_id=? AND deleted_at IS NULL",
        )
        .bind(group_id)
        .fetch_all(executor)
        .await,
        None => {
            sqlx::query("SELECT * FROM groups WHERE owner_type='group' AND deleted_at IS NULL")
                .fetch_all(executor)
                .await
        }
    }
    .map_err(|error| error.to_string())
}

/// Repair only the requested group, or all groups for a list read. Re-reading
/// within the write transaction preserves concurrent edits and shared hashes.
pub(crate) async fn normalize_group_configurations<F>(
    db: &DbState,
    group_id: Option<&str>,
    mut on_repaired: F,
) -> Result<bool, String>
where
    F: FnMut() + Send + 'static,
{
    let db = db.clone();
    let group_id = group_id.map(str::to_owned);
    tokio::spawn(async move {
        let local_changed = normalize_group_local_settings(&db, group_id.as_deref()).await?;
        if local_changed { on_repaired(); }
        let rows = stored_group_configuration_rows(&db.pool, group_id.as_deref()).await?;
        let mut needed = false;
        for row in &rows {
            if decode_group_fields(row)?.1 { needed = true; break; }
        }
        if !needed { return Ok(local_changed); }
        let mut tx = db.write_transaction("identity.group.normalize_config").await?;
        let rows = stored_group_configuration_rows(&mut *tx, group_id.as_deref()).await?;
        let mut changed = false;
        for row in rows {
            let (mut config, invalid) = decode_group_fields(&row)?;
            if !invalid { continue; }
            config.id = row.try_get("group_id").map_err(|error| error.to_string())?;
            config.created_at = row.try_get("created_at").map_err(|error| error.to_string())?;
            config.members = sqlx::query_scalar("SELECT agent_id FROM group_members WHERE group_id=? ORDER BY sort_order ASC")
                .bind(&config.id).fetch_all(&mut *tx).await.map_err(|error| error.to_string())?;
            let hash = HashAggregator::compute_group_config_hash(&GroupSyncDTO::from(&config));
            sqlx::query("UPDATE groups SET name=?,mode=?,group_prompt=?,invite_prompt=?,use_unified_model=?,unified_model=?,tag_match_mode=?,member_tags=?,config_hash=?,updated_at=? WHERE owner_type='group' AND group_id=?")
                .bind(&config.name).bind(&config.mode).bind(&config.group_prompt).bind(&config.invite_prompt)
                .bind(i32::from(config.use_unified_model)).bind(&config.unified_model).bind(&config.tag_match_mode)
                .bind(serialize_member_tags(config.member_tags.as_ref())?).bind(hash)
                .bind(crate::vcp_modules::infra::utils::now_millis()).bind(&config.id)
                .execute(&mut *tx).await.map_err(|error| error.to_string())?;
            changed = true;
        }
        tx.commit().await.map_err(|error| error.to_string())?;
        if changed { on_repaired(); }
        Ok(changed || local_changed)
    }).await.map_err(|error| format!("群组配置修复任务异常：{error}"))?
}

/// 显式配置备份包含已经保存的本机群偏好，也保留尚未同步绑定的群 ID。
pub async fn export_group_local_settings(
    db: &DbState,
) -> Result<BTreeMap<String, GroupLocalSettings>, String> {
    normalize_group_local_settings(db, None).await?;
    let rows = sqlx::query(
        "SELECT l.group_id, l.settings_json FROM group_local_settings l
         JOIN groups g ON g.owner_type = 'group' AND g.group_id = l.group_id
         WHERE g.deleted_at IS NULL ORDER BY l.group_id",
    )
    .fetch_all(&db.pool)
    .await
    .map_err(|error| error.to_string())?;
    rows.into_iter()
        .map(|row| {
            let group_id: String = row.try_get("group_id").map_err(|error| error.to_string())?;
            Ok((
                group_id,
                parse_group_local_settings(stored_local_settings_json(&row)?.as_deref()),
            ))
        })
        .collect()
}

pub fn parse_group_local_settings_backup(
    value: &serde_json::Value,
) -> Result<BTreeMap<String, GroupLocalSettingsPatch>, String> {
    let mut preferences = BTreeMap::new();
    let Some(source) = value.as_object() else {
        return Ok(preferences);
    };
    for (group_id, value) in source {
        if group_id.trim().is_empty() {
            continue;
        }
        let patch = GroupLocalSettingsPatch::parse(value)?;
        if !patch.is_empty() {
            preferences.insert(group_id.clone(), patch);
        }
    }
    Ok(preferences)
}

fn is_local_settings_field(field: &str) -> bool {
    matches!(
        field,
        "chatMode" | "modeSettings" | "enableContextMessageWindow" | "contextMessageWindowSize"
    )
}

async fn read_stored_local_settings(
    connection: &mut SqliteConnection,
    group_id: &str,
) -> Result<GroupLocalSettings, String> {
    let row = sqlx::query("SELECT settings_json FROM group_local_settings WHERE group_id=?")
        .bind(group_id)
        .fetch_optional(connection)
        .await
        .map_err(|error| error.to_string())?;
    let raw = row
        .as_ref()
        .map(stored_local_settings_json)
        .transpose()?
        .flatten();
    Ok(parse_group_local_settings(raw.as_deref()))
}

async fn update_group_local_settings(
    connection: &mut SqliteConnection,
    group_id: &str,
    updates: &serde_json::Map<String, serde_json::Value>,
) -> Result<bool, String> {
    if updates.keys().any(|key| !is_local_settings_field(key)) {
        return Err("本机群设置补丁包含共享字段".into());
    }
    let live: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM groups WHERE owner_type='group' AND group_id=? AND deleted_at IS NULL)",
    ).bind(group_id).fetch_one(&mut *connection).await.map_err(|error| error.to_string())?;
    if !live {
        return Err(format!("Group {group_id} not found"));
    }
    let current = read_stored_local_settings(connection, group_id).await?;
    let settings = GroupLocalSettingsPatch::parse(&serde_json::Value::Object(updates.clone()))?
        .apply(&current)?;
    write_group_local_settings(connection, group_id, &settings).await
}

/// 调用者持有 settings 导入事务；本函数不提交，提交后须失效 GroupManagerState 缓存。
/// 缺失字段保留原值；仅恢复本机已有存活群。返回是否有尚未同步的群，墓碑静默跳过。
pub async fn import_group_local_settings(
    connection: &mut SqliteConnection,
    preferences: &BTreeMap<String, GroupLocalSettingsPatch>,
) -> Result<bool, String> {
    let mut missing_groups = false;
    for (group_id, patch) in preferences {
        if group_id.trim().is_empty() || patch.is_empty() {
            continue;
        }
        let deleted_at: Option<Option<i64>> = sqlx::query_scalar(
            "SELECT deleted_at FROM groups WHERE owner_type='group' AND group_id=?",
        )
        .bind(group_id)
        .fetch_optional(&mut *connection)
        .await
        .map_err(|error| error.to_string())?;
        match deleted_at {
            None => {
                missing_groups = true;
                continue;
            }
            Some(Some(_)) => continue,
            Some(None) => {}
        }
        let current = read_stored_local_settings(connection, group_id).await?;
        let restored = patch.apply(&current)?;
        write_group_local_settings(connection, group_id, &restored).await?;
    }
    Ok(missing_groups)
}

async fn write_group_local_settings(
    connection: &mut SqliteConnection,
    group_id: &str,
    settings: &GroupLocalSettings,
) -> Result<bool, String> {
    let mut settings = settings.clone();
    settings.normalize();
    let json = serde_json::to_string(&settings).map_err(|error| error.to_string())?;
    let result = sqlx::query(
        "INSERT INTO group_local_settings (group_id, settings_json, updated_at)
         SELECT ?, ?, ? WHERE EXISTS (
             SELECT 1 FROM groups WHERE owner_type = 'group' AND group_id = ? AND deleted_at IS NULL
         )
         ON CONFLICT(group_id) DO UPDATE SET
             settings_json = excluded.settings_json, updated_at = excluded.updated_at
         WHERE group_local_settings.settings_json IS NOT excluded.settings_json",
    )
    .bind(group_id)
    .bind(json)
    .bind(crate::vcp_modules::infra::utils::now_millis())
    .bind(group_id)
    .execute(connection)
    .await
    .map_err(|error| error.to_string())?;
    Ok(result.rows_affected() > 0)
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
    let db_state = app_handle.state::<DbState>();
    let app = app_handle.clone();
    normalize_group_configurations(&db_state, Some(group_id), move || {
        if let Some(state) = app.try_state::<GroupManagerState>() {
            state.invalidate_cache();
        }
    })
    .await?;
    let cache_generation = state.current_cache_generation();
    let pool = &db_state.pool;

    let group_row: Option<sqlx::sqlite::SqliteRow> = sqlx::query(
        "SELECT g.name, g.mode, g.group_prompt, g.invite_prompt, g.use_unified_model, g.unified_model, g.tag_match_mode, g.member_tags, g.created_at, av.dominant_color, l.settings_json
         FROM groups g
         LEFT JOIN avatars av ON av.owner_id = g.group_id AND av.owner_type = 'group' AND av.deleted_at IS NULL
         LEFT JOIN group_local_settings l ON l.group_id = g.group_id
         WHERE g.owner_type = 'group' AND g.group_id = ? AND g.deleted_at IS NULL"
    )
    .bind(group_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| e.to_string())?;

    if let Some(row) = group_row {
        use sqlx::Row;
        let avatar_calculated_color = row
            .try_get::<Option<String>, _>("dominant_color")
            .ok()
            .flatten();

        let member_rows: Vec<sqlx::sqlite::SqliteRow> = sqlx::query(
            "SELECT agent_id FROM group_members WHERE group_id = ? ORDER BY sort_order ASC",
        )
        .bind(group_id)
        .fetch_all(pool)
        .await
        .map_err(|e| e.to_string())?;

        let mut members = Vec::new();
        for mr in member_rows {
            let aid: String = mr.try_get("agent_id").map_err(|error| error.to_string())?;
            members.push(aid);
        }
        let (fields, _) = decode_group_fields(&row)?;

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

        let local_settings =
            parse_group_local_settings(stored_local_settings_json(&row)?.as_deref());
        let config = GroupConfig {
            id: group_id.to_string(),
            avatar_calculated_color,
            members,
            topics,
            created_at: row
                .try_get("created_at")
                .map_err(|error| error.to_string())?,
            local_settings,
            ..fields
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

    internal_write_group_config(&app_handle, &state, &group_id, &group, None).await
}

#[tauri::command]
pub async fn get_groups(
    app_handle: AppHandle,
    _state: State<'_, GroupManagerState>,
) -> Result<Vec<GroupConfig>, String> {
    let start_total = std::time::Instant::now();
    let db_state = app_handle.state::<DbState>();
    let app = app_handle.clone();
    normalize_group_configurations(&db_state, None, move || {
        if let Some(state) = app.try_state::<GroupManagerState>() {
            state.invalidate_cache();
        }
    })
    .await?;
    let pool = &db_state.pool;

    let start_groups = std::time::Instant::now();
    // 1. 查询所有未删除群组的基础配置及 avatars 主色
    let group_rows = sqlx::query(
        "SELECT g.group_id, g.name, g.mode, g.group_prompt, g.invite_prompt, g.use_unified_model, g.unified_model, g.tag_match_mode, g.member_tags, g.created_at, av.dominant_color, l.settings_json
         FROM groups g
         LEFT JOIN avatars av ON av.owner_id = g.group_id AND av.owner_type = 'group' AND av.deleted_at IS NULL
         LEFT JOIN group_local_settings l ON l.group_id = g.group_id
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
        let gid: String = mr.try_get("group_id").map_err(|error| error.to_string())?;
        let aid: String = mr.try_get("agent_id").map_err(|error| error.to_string())?;

        group_members.entry(gid).or_default().push(aid);
    }

    // 组装列表；实体缓存只允许在 per-ID 锁内填充。
    let mut groups = Vec::new();
    for row in group_rows {
        use sqlx::Row;
        let group_id: String = row.try_get("group_id").map_err(|error| error.to_string())?;
        let avatar_calculated_color = row
            .try_get::<Option<String>, _>("dominant_color")
            .ok()
            .flatten();

        let members = group_members.remove(&group_id).unwrap_or_default();
        let (fields, _) = decode_group_fields(&row)?;

        let local_settings =
            parse_group_local_settings(stored_local_settings_json(&row)?.as_deref());
        let config = GroupConfig {
            id: group_id.clone(),
            avatar_calculated_color,
            members,
            topics: vec![], // 优化：话题改为前端选中时流式懒加载，初始置为空
            created_at: row
                .try_get("created_at")
                .map_err(|error| error.to_string())?,
            local_settings,
            ..fields
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

    let updates_obj = updates
        .as_object()
        .ok_or_else(|| "群配置补丁必须为对象".to_string())?;
    if updates_obj.keys().all(|key| is_local_settings_field(key)) {
        if !updates_obj.is_empty() {
            let db = app_handle.state::<DbState>();
            let mut tx = db
                .write_transaction("identity.group.local-settings")
                .await?;
            let changed = update_group_local_settings(&mut tx, &group_id, updates_obj).await?;
            tx.commit().await.map_err(|error| error.to_string())?;
            state.caches.remove(&group_id);
            if changed {
                if let Some(turns) =
                    app_handle.try_state::<crate::vcp_modules::vcp_client::ActiveGroupTurns>()
                {
                    turns.notify_group_changed(&group_id);
                }
            }
        }
        state.caches.remove(&group_id);
        return read_group_config_locked(&app_handle, &state, &group_id).await;
    }

    // 增量编辑必须从最新持久视图读取；同步写入不通过 GroupManagerState 的缓存。
    state.caches.remove(&group_id);
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

    let local_updates = updates_obj
        .iter()
        .filter(|(key, _)| is_local_settings_field(key))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    internal_write_group_config(
        &app_handle,
        &state,
        &group_id,
        &new_config,
        Some(&local_updates),
    )
    .await
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

    let mut tx = db_state.write_transaction("identity.group.create").await?;

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
        local_settings: GroupLocalSettings::default(),
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
    state.insert_cache_if_current(group_id, config.clone(), cache_generation);
    Ok(config)
}

async fn internal_write_group_config<R: Runtime>(
    app_handle: &AppHandle<R>,
    state: &GroupManagerState,
    group_id: &str,
    config: &GroupConfig,
    local_updates: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Result<GroupConfig, String> {
    let cache_generation = state.current_cache_generation();
    let db_state = app_handle.state::<DbState>();

    let now = crate::vcp_modules::infra::utils::now_millis();

    let mut tx = db_state.write_transaction("identity.group.save").await?;

    let existing = sqlx::query_as::<_, (Option<i64>, String)>(
        "SELECT deleted_at, config_hash FROM groups WHERE owner_type = 'group' AND group_id = ?",
    )
    .bind(group_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;
    if existing
        .as_ref()
        .is_some_and(|(deleted_at, _)| deleted_at.is_some())
    {
        tx.commit().await.map_err(|e| e.to_string())?;
        return Err(format!("Group {group_id} has been deleted"));
    }

    // 同步阶段允许 Group 比新 Agent 更早落库，因此只排除“明确已有墓碑”的成员；
    // 完全缺失的 Agent 仍作为前向引用保留，避免依赖 Phase 1 到达顺序。
    let mut canonical_config = config.clone();
    if let Some(updates) = local_updates {
        let current = read_stored_local_settings(&mut tx, group_id).await?;
        canonical_config.local_settings =
            GroupLocalSettingsPatch::parse(&serde_json::Value::Object(updates.clone()))?
                .apply(&current)?;
    }
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
    canonical_config.local_settings.normalize();
    let dto = GroupSyncDTO::from(&canonical_config);
    let config_hash = HashAggregator::compute_group_config_hash(&dto);
    let member_tags = serialize_member_tags(canonical_config.member_tags.as_ref())
        .map_err(|error| format!("Group {group_id} memberTags encode failed: {error}"))?;

    // 本机行为保存不能改变共享 Owner 或成员关系的版本。
    let shared_changed = existing
        .as_ref()
        .is_none_or(|(_, hash)| hash != &config_hash);
    if shared_changed {
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
    }
    let local_changed = if local_updates.is_none_or(|updates| !updates.is_empty()) {
        write_group_local_settings(&mut tx, group_id, &canonical_config.local_settings).await?
    } else {
        false
    };

    tx.commit().await.map_err(|e| e.to_string())?;
    state.insert_cache_if_current(
        group_id.to_string(),
        canonical_config.clone(),
        cache_generation,
    );
    if shared_changed || local_changed {
        if let Some(turns) =
            app_handle.try_state::<crate::vcp_modules::vcp_client::ActiveGroupTurns>()
        {
            turns.notify_group_changed(group_id);
        }
    }

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
    let now = requested_deleted_at.unwrap_or_else(crate::vcp_modules::infra::utils::now_millis);
    if now < 0 {
        return Err("Group delete requires a non-negative deletedAt".to_string());
    }
    let writer_kind = if requested_deleted_at.is_some() {
        "sync.delete.group"
    } else {
        "identity.group.delete"
    };
    let mut tx = db_state.write_transaction(writer_kind).await?;

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
            return Err(format!("Group {group_id} does not exist"));
        }
        Some(Some(existing)) => {
            tx.commit().await.map_err(|e| e.to_string())?;
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
    if let Some(turns) = app_handle.try_state::<crate::vcp_modules::vcp_client::ActiveGroupTurns>()
    {
        turns.cancel_owner("group", group_id);
    }
    for key in active_keys {
        crate::vcp_modules::vcp_client::discard_deleted_generation(app_handle, &key).await;
    }

    state.caches.remove(group_id);
    Ok(now)
}

#[cfg(test)]
mod local_settings_tests {
    use super::*;
    use crate::vcp_modules::group_types::{GroupChatMode, JevFallbackPolicy, JevModeSettings};
    use sqlx::Connection;

    async fn database() -> SqliteConnection {
        let mut connection = SqliteConnection::connect("sqlite::memory:")
            .await
            .expect("open group settings database");
        sqlx::raw_sql(include_str!("../../../migrations/0100_baseline_v2.sql"))
            .execute(&mut connection)
            .await
            .expect("create shared group schema");
        sqlx::raw_sql(include_str!(
            "../../../migrations/0104_group_local_settings.sql"
        ))
        .execute(&mut connection)
        .await
        .expect("migrate local group settings");
        connection
    }

    #[test]
    fn chat_mode_is_independent_and_invalid_values_are_canonical() {
        for value in [
            serde_json::json!({}),
            serde_json::json!({"chatMode":null}),
            serde_json::json!({"chatMode":""}),
            serde_json::json!({"chatMode":"unknown"}),
            serde_json::json!({"chatMode":false}),
            serde_json::json!({"chatMode":42}),
            serde_json::json!({"chatMode":[]}),
            serde_json::json!({"chatMode":{}}),
        ] {
            let settings: GroupLocalSettings =
                serde_json::from_value(value).expect("normalize mode");
            assert_eq!(settings.effective_mode(), GroupChatMode::Sequential);
            assert_eq!(
                serde_json::to_value(settings).unwrap()["chatMode"],
                "sequential"
            );
        }
        let current = GroupLocalSettings {
            chat_mode: GroupChatMode::Jev,
            ..Default::default()
        };
        let missing =
            GroupLocalSettingsPatch::parse(&serde_json::json!({"contextMessageWindowSize":19}))
                .unwrap();
        assert_eq!(
            missing.apply(&current).unwrap().chat_mode,
            GroupChatMode::Jev
        );
        let invalid =
            GroupLocalSettingsPatch::parse(&serde_json::json!({"chatMode":false})).unwrap();
        assert_eq!(
            serde_json::to_value(&invalid).unwrap()["chatMode"],
            "sequential"
        );
        assert_eq!(
            invalid.apply(&current).unwrap().chat_mode,
            GroupChatMode::Sequential
        );
    }

    #[test]
    fn local_decode_recovers_only_damaged_fields_and_keeps_typed_modes() {
        let settings: GroupLocalSettings = serde_json::from_value(serde_json::json!({
            "chatMode":"jev", "modeOverride":"invite_only", "unknown":true,
            "enableContextMessageWindow":"yes", "contextMessageWindowSize":17,
            "modeSettings": {
                "sequential":{"speakerOrder":["b",null,"",7,"a","b"," "]},
                "jev": {
                    "historyWindow":0, "maxSpeakersPerRound":"three", "speakerThreshold":-0.1,
                    "continueThreshold":0.23, "stopThreshold":true, "maxAutonomousRounds":71,
                    "continueDebounceMs":99999, "navigatorPrompt":false, "fallbackPolicy":"top_speaker",
                    "memberStyles":{"a":"kept","b":12," ":"discarded"}, "unknown":"discarded"
                }
            }
        })).unwrap();
        let defaults = JevModeSettings::default();
        assert_eq!(settings.chat_mode, GroupChatMode::Jev);
        assert!(!settings.enable_context_message_window);
        assert_eq!(settings.context_message_window_size, 17);
        assert_eq!(
            settings.mode_settings.sequential.speaker_order,
            Vec::<String>::new()
        );
        let jev = &settings.mode_settings.jev;
        assert_eq!(jev.history_window, defaults.history_window);
        assert_eq!(jev.max_speakers_per_round, defaults.max_speakers_per_round);
        assert_eq!(jev.speaker_threshold, defaults.speaker_threshold);
        assert_eq!(jev.continue_threshold, 0.23);
        assert_eq!(jev.stop_threshold, defaults.stop_threshold);
        assert_eq!(jev.max_autonomous_rounds, 71);
        assert_eq!(jev.continue_debounce_ms, defaults.continue_debounce_ms);
        assert_eq!(jev.navigator_prompt, defaults.navigator_prompt);
        assert_eq!(jev.fallback_policy, JevFallbackPolicy::TopSpeaker);
        assert_eq!(
            jev.member_styles,
            BTreeMap::from([("a".into(), "kept".into()), ("b".into(), "".into())])
        );
        let canonical = serde_json::to_value(&settings).unwrap();
        assert!(canonical.get("modeOverride").is_none());
        assert!(canonical.get("unknown").is_none());
        assert!(canonical["modeSettings"]["jev"].get("unknown").is_none());
        assert_eq!(
            serde_json::from_value::<GroupLocalSettings>(canonical).unwrap(),
            settings
        );
        for raw in ["not-json", "null", "false", "[]", "12"] {
            assert_eq!(
                parse_group_local_settings(Some(raw)),
                GroupLocalSettings::default()
            );
        }
    }

    #[tokio::test]
    async fn normalization_persists_typed_defaults_from_current_values_without_touching_timestamps()
    {
        let mut connection = database().await;
        sqlx::query("INSERT INTO group_local_settings VALUES ('bad', ?, 77), ('valid', ?, 88), ('broken', 'false', 99)")
            .bind(r#"{"chatMode":null,"contextMessageWindowSize":100,"modeSettings":{"jev":{"memberStyles":{"a":"keep"}}}}"#)
            .bind(r#"{"chatMode":"jev","contextMessageWindowSize":222}"#)
            .execute(&mut connection).await.unwrap();
        // A different local field changes after a caller's earlier read.
        sqlx::query("UPDATE group_local_settings SET settings_json=json_set(settings_json,'$.contextMessageWindowSize',321) WHERE group_id='bad'")
            .execute(&mut connection).await.unwrap();
        assert!(normalize_stored_group_local_settings(&mut connection, None)
            .await
            .unwrap());
        let (raw, time): (String, i64) = sqlx::query_as(
            "SELECT settings_json,updated_at FROM group_local_settings WHERE group_id='bad'",
        )
        .fetch_one(&mut connection)
        .await
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(value["chatMode"], "sequential");
        assert_eq!(value["contextMessageWindowSize"], 321);
        assert_eq!(value["modeSettings"]["jev"]["memberStyles"]["a"], "keep");
        assert_eq!(time, 77);
        let valid: String = sqlx::query_scalar(
            "SELECT settings_json FROM group_local_settings WHERE group_id='valid'",
        )
        .fetch_one(&mut connection)
        .await
        .unwrap();
        let valid = parse_group_local_settings(Some(&valid));
        assert_eq!(valid.chat_mode, GroupChatMode::Jev);
        assert_eq!(valid.context_message_window_size, 222);
        let (broken, time): (String, i64) = sqlx::query_as(
            "SELECT settings_json,updated_at FROM group_local_settings WHERE group_id='broken'",
        )
        .fetch_one(&mut connection)
        .await
        .unwrap();
        assert_eq!(
            serde_json::from_str::<GroupLocalSettings>(&broken).unwrap(),
            GroupLocalSettings::default()
        );
        assert_eq!(time, 99);
        assert!(
            !normalize_stored_group_local_settings(&mut connection, None)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn local_normalization_keeps_real_database_failures_visible() {
        let mut connection = database().await;
        sqlx::query("DROP TABLE group_local_settings")
            .execute(&mut connection)
            .await
            .unwrap();
        assert!(normalize_stored_group_local_settings(&mut connection, None)
            .await
            .is_err());
    }

    #[test]
    fn overflowing_json_number_defaults_only_its_group_setting() {
        let settings = parse_group_local_settings(Some(
            r#"{"chatMode":"jev","contextMessageWindowSize":222,"modeSettings":{"jev":{"speakerThreshold":1e400,"continueThreshold":0.7,"memberStyles":{"a":"keep","b":1e400}}}}"#,
        ));
        assert_eq!(settings.chat_mode, GroupChatMode::Jev);
        assert_eq!(settings.context_message_window_size, 222);
        assert_eq!(
            settings.mode_settings.jev.speaker_threshold,
            JevModeSettings::default().speaker_threshold
        );
        assert_eq!(settings.mode_settings.jev.continue_threshold, 0.7);
        assert_eq!(settings.mode_settings.jev.member_styles["a"], "keep");
        assert_eq!(settings.mode_settings.jev.member_styles["b"], "");
    }

    #[tokio::test]
    async fn binary_local_configuration_recovers_without_coercion_or_timestamp_changes() {
        let mut connection = database().await;
        sqlx::query("INSERT INTO group_local_settings VALUES ('blob', ?, 41)")
            .bind(br#"{"chatMode":"jev"}"#.as_slice())
            .execute(&mut connection)
            .await
            .unwrap();
        assert!(normalize_stored_group_local_settings(&mut connection, None)
            .await
            .unwrap());
        let (raw, stamp): (String, i64) = sqlx::query_as(
            "SELECT settings_json,updated_at FROM group_local_settings WHERE group_id='blob'",
        )
        .fetch_one(&mut connection)
        .await
        .unwrap();
        assert_eq!(
            parse_group_local_settings(Some(&raw)),
            GroupLocalSettings::default()
        );
        assert_eq!(stamp, 41);
        assert!(
            !normalize_stored_group_local_settings(&mut connection, None)
                .await
                .unwrap()
        );
        // Simulate stored corruption; ordinary writes are already constrained.
        sqlx::query("PRAGMA ignore_check_constraints=ON")
            .execute(&mut connection)
            .await
            .unwrap();
        sqlx::query("UPDATE group_local_settings SET settings_json=CAST(x'ff' AS TEXT)")
            .execute(&mut connection)
            .await
            .unwrap();
        sqlx::query("PRAGMA ignore_check_constraints=OFF")
            .execute(&mut connection)
            .await
            .unwrap();
        assert!(normalize_stored_group_local_settings(&mut connection, None)
            .await
            .unwrap());
        assert_eq!(
            read_stored_local_settings(&mut connection, "blob")
                .await
                .unwrap(),
            GroupLocalSettings::default()
        );
        sqlx::query("PRAGMA ignore_check_constraints=ON")
            .execute(&mut connection)
            .await
            .unwrap();
        sqlx::query("UPDATE group_local_settings SET settings_json=x'ff'")
            .execute(&mut connection)
            .await
            .unwrap();
        sqlx::query("PRAGMA ignore_check_constraints=OFF")
            .execute(&mut connection)
            .await
            .unwrap();
        sqlx::query("INSERT INTO groups(owner_type,group_id,name,updated_at) VALUES('group','blob','Synced',1)")
            .execute(&mut connection).await.unwrap();
        let changes = serde_json::json!({"blob":{"contextMessageWindowSize":333}});
        import_group_local_settings(
            &mut connection,
            &parse_group_local_settings_backup(&changes).unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(
            read_stored_local_settings(&mut connection, "blob")
                .await
                .unwrap()
                .context_message_window_size,
            333
        );
    }

    #[tokio::test]
    async fn shared_group_settings_repair_typed_fields_and_keep_sync_projection_consistent() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::raw_sql(include_str!("../../../migrations/0100_baseline_v2.sql"))
            .execute(&pool)
            .await
            .unwrap();
        sqlx::raw_sql(include_str!(
            "../../../migrations/0104_group_local_settings.sql"
        ))
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO groups(owner_type,group_id,name,group_prompt,invite_prompt,use_unified_model,tag_match_mode,member_tags,created_at,updated_at,content_hash) VALUES ('group','g',x'ff','keep',NULL,4294967297,'invalid',?,123,42,'untouched-content')")
            .bind(r#"{"a":"keep","b":1e400}"#).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO group_members VALUES ('g','a',0,42)")
            .execute(&pool)
            .await
            .unwrap();
        let db = DbState::new(pool.clone(), std::path::PathBuf::new());
        assert!(normalize_group_configurations(&db, None, || {})
            .await
            .unwrap());
        let row = sqlx::query("SELECT * FROM groups WHERE group_id='g'")
            .fetch_one(&pool)
            .await
            .unwrap();
        let (mut config, invalid) = decode_group_fields(&row).unwrap();
        assert!(!invalid);
        assert_eq!(config.name, GroupConfig::default().name);
        assert_eq!(config.group_prompt.as_deref(), Some("keep"));
        assert_eq!(config.invite_prompt, None);
        assert!(!config.use_unified_model);
        assert_eq!(config.tag_match_mode, GroupConfig::default().tag_match_mode);
        assert_eq!(config.member_tags.as_ref().unwrap()["a"], "keep");
        assert_eq!(config.member_tags.as_ref().unwrap()["b"], "");
        config.members = vec!["a".into()];
        config.created_at = 123;
        assert_eq!(
            row.get::<String, _>("config_hash"),
            HashAggregator::compute_group_config_hash(&GroupSyncDTO::from(&config))
        );
        assert_eq!(row.get::<String, _>("content_hash"), "untouched-content");
        assert!(row.get::<i64, _>("updated_at") > 42);
        assert!(!normalize_group_configurations(&db, None, || {})
            .await
            .unwrap());
        // Storage failure must not masquerade as a successful default recovery.
        sqlx::query("UPDATE groups SET name=x'ff' WHERE group_id='g'")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::raw_sql("CREATE TRIGGER reject_repair BEFORE UPDATE ON groups BEGIN SELECT RAISE(ABORT,'write failed'); END;").execute(&pool).await.unwrap();
        assert!(normalize_group_configurations(&db, None, || {})
            .await
            .is_err());
        let storage_type: String =
            sqlx::query_scalar("SELECT typeof(name) FROM groups WHERE group_id='g'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(storage_type, "blob");
    }

    #[tokio::test]
    async fn single_group_cold_read_repairs_only_its_scope_and_full_scope_repairs_the_rest() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::raw_sql(include_str!("../../../migrations/0100_baseline_v2.sql"))
            .execute(&pool)
            .await
            .unwrap();
        sqlx::raw_sql(include_str!(
            "../../../migrations/0104_group_local_settings.sql"
        ))
        .execute(&pool)
        .await
        .unwrap();
        sqlx::raw_sql(
            "INSERT INTO groups(owner_type,group_id,name,group_prompt,config_hash,updated_at) VALUES
                ('group','target',x'ff','keep target prompt','before',42),
                ('group','other',x'ff','keep other prompt','before',42);
             INSERT INTO group_local_settings VALUES
                ('target','{\"chatMode\":\"jev\"}',42),('other','false',42);
             CREATE TRIGGER reject_unrelated_shared_repair BEFORE UPDATE ON groups
                WHEN OLD.group_id='other' BEGIN SELECT RAISE(ABORT,'unrelated shared repair'); END;
             CREATE TRIGGER reject_unrelated_local_repair BEFORE UPDATE ON group_local_settings
                WHEN OLD.group_id='other' BEGIN SELECT RAISE(ABORT,'unrelated local repair'); END;",
        ).execute(&pool).await.unwrap();
        let db = DbState::new(pool.clone(), std::path::PathBuf::new());
        let app = tauri::test::mock_app();
        app.manage(db.clone());
        app.manage(GroupManagerState::new());

        let config =
            read_group_config_internal(app.handle(), &app.state::<GroupManagerState>(), "target")
                .await
                .expect("a single-group read must not repair unrelated groups");
        assert_eq!(config.name, GroupConfig::default().name);
        assert_eq!(config.group_prompt.as_deref(), Some("keep target prompt"));
        assert_eq!(config.local_settings.chat_mode, GroupChatMode::Jev);
        let other: (String, String, i64, String, i64) = sqlx::query_as(
            "SELECT typeof(g.name),g.config_hash,g.updated_at,l.settings_json,l.updated_at
             FROM groups g JOIN group_local_settings l ON l.group_id=g.group_id
             WHERE g.group_id='other'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            other,
            ("blob".into(), "before".into(), 42, "false".into(), 42)
        );

        sqlx::raw_sql("DROP TRIGGER reject_unrelated_shared_repair; DROP TRIGGER reject_unrelated_local_repair;")
            .execute(&pool).await.unwrap();
        assert!(normalize_group_configurations(&db, None, || {})
            .await
            .unwrap());
        let other = sqlx::query("SELECT * FROM groups WHERE group_id='other'")
            .fetch_one(&pool)
            .await
            .unwrap();
        let (config, invalid) = decode_group_fields(&other).unwrap();
        assert!(!invalid);
        assert_eq!(config.name, GroupConfig::default().name);
        assert_eq!(config.group_prompt.as_deref(), Some("keep other prompt"));
        let settings: String = sqlx::query_scalar(
            "SELECT settings_json FROM group_local_settings WHERE group_id='other'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            serde_json::from_str::<GroupLocalSettings>(&settings).unwrap(),
            GroupLocalSettings::default()
        );
        assert!(!normalize_group_configurations(&db, None, || {})
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn export_persists_default_mode_without_changing_shared_configuration() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::raw_sql(include_str!("../../../migrations/0100_baseline_v2.sql"))
            .execute(&pool)
            .await
            .unwrap();
        sqlx::raw_sql(include_str!(
            "../../../migrations/0104_group_local_settings.sql"
        ))
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO groups(owner_type,group_id,name,mode,config_hash,updated_at) VALUES ('group','group','Group','jev','shared-hash',41)")
            .execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO group_local_settings VALUES ('group', '{\"contextMessageWindowSize\":222}', 42)")
            .execute(&pool).await.unwrap();
        let db = DbState::new(pool.clone(), std::path::PathBuf::new());
        let exported = export_group_local_settings(&db).await.unwrap();
        assert_eq!(exported["group"].chat_mode, GroupChatMode::Sequential);
        let saved_mode: String = sqlx::query_scalar("SELECT json_extract(settings_json,'$.chatMode') FROM group_local_settings WHERE group_id='group'")
            .fetch_one(&pool).await.unwrap();
        assert_eq!(saved_mode, "sequential");
        let shared: (String, String, i64) =
            sqlx::query_as("SELECT mode,config_hash,updated_at FROM groups WHERE group_id='group'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(shared, ("jev".into(), "shared-hash".into(), 41));
    }

    #[tokio::test]
    async fn incremental_shared_save_uses_current_local_fields_and_only_applies_supplied_changes() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::raw_sql(include_str!("../../../migrations/0100_baseline_v2.sql"))
            .execute(&pool)
            .await
            .unwrap();
        sqlx::raw_sql(include_str!(
            "../../../migrations/0104_group_local_settings.sql"
        ))
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO groups(owner_type,group_id,name,mode,updated_at) VALUES ('group','group','Original','invite_only',1)")
            .execute(&pool).await.unwrap();
        let mut earlier: GroupConfig = serde_json::from_value(serde_json::json!({
            "id":"group", "name":"Renamed", "mode":"invite_only", "chatMode":"sequential"
        }))
        .unwrap();
        // A backup commit lands after the incremental editor read this object.
        let latest = r#"{"chatMode":"jev","contextMessageWindowSize":888,"modeSettings":{"jev":{"memberStyles":{"a":"latest style"}}}}"#;
        sqlx::query("INSERT INTO group_local_settings VALUES('group',?,7)")
            .bind(latest)
            .execute(&pool)
            .await
            .unwrap();
        let app = tauri::test::mock_app();
        app.manage(DbState::new(pool.clone(), std::path::PathBuf::new()));
        let state = GroupManagerState::new();
        let shared_only = serde_json::Map::new();
        let saved = internal_write_group_config(
            app.handle(),
            &state,
            "group",
            &earlier,
            Some(&shared_only),
        )
        .await
        .unwrap();
        assert_eq!(saved.local_settings.chat_mode, GroupChatMode::Jev);
        let (raw, time): (String, i64) = sqlx::query_as(
            "SELECT settings_json,updated_at FROM group_local_settings WHERE group_id='group'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(raw, latest);
        assert_eq!(time, 7);
        let mixed = serde_json::json!({"contextMessageWindowSize":222})
            .as_object()
            .unwrap()
            .clone();
        earlier.name = "Renamed again".into();
        let saved =
            internal_write_group_config(app.handle(), &state, "group", &earlier, Some(&mixed))
                .await
                .unwrap();
        assert_eq!(saved.local_settings.chat_mode, GroupChatMode::Jev);
        assert_eq!(saved.local_settings.context_message_window_size, 222);
        assert_eq!(
            saved.local_settings.mode_settings.jev.member_styles["a"],
            "latest style"
        );
        let saved = internal_write_group_config(app.handle(), &state, "group", &earlier, None)
            .await
            .unwrap();
        assert_eq!(
            saved.local_settings.chat_mode,
            GroupChatMode::Sequential,
            "explicit full save keeps replacement semantics"
        );
    }

    #[test]
    fn old_group_defaults_are_local_and_do_not_enter_the_sync_projection() {
        let mut group: GroupConfig = serde_json::from_value(serde_json::json!({
            "id": "group", "mode": "jev", "members": ["a", "b"]
        }))
        .expect("read old shared group with no local fields");
        assert_eq!(group.effective_mode(), GroupChatMode::Sequential);
        let before = serde_json::to_value(GroupSyncDTO::from(&group)).expect("sync before");
        let hash_before = HashAggregator::compute_group_config_hash(&GroupSyncDTO::from(&group));
        group.local_settings.chat_mode = GroupChatMode::InviteOnly;
        group.local_settings.mode_settings.sequential.speaker_order = vec!["b".into(), "a".into()];
        group.local_settings.mode_settings.jev.speaker_threshold = 0.73;
        group.local_settings.enable_context_message_window = true;
        assert_eq!(group.effective_mode(), GroupChatMode::InviteOnly);
        assert_eq!(
            serde_json::to_value(GroupSyncDTO::from(&group)).expect("sync after"),
            before
        );
        assert_eq!(
            HashAggregator::compute_group_config_hash(&GroupSyncDTO::from(&group)),
            hash_before
        );
        let ipc = serde_json::to_value(&group).expect("IPC group");
        assert_eq!(ipc["mode"], "jev");
        assert_eq!(ipc["chatMode"], "invite_only");
        assert_eq!(
            ipc["modeSettings"]["sequential"]["speakerOrder"],
            serde_json::json!(["b", "a"])
        );
    }

    #[test]
    fn backup_preserves_presence_and_replaces_member_maps_and_arrays() {
        let supplied = serde_json::json!({
            "group": {
                "chatMode": null,
                "modeSettings": {
                    "sequential": {"speakerOrder":["b"]},
                    "jev": {"historyWindow":19,"memberStyles":{"b":"new style"}}
                }
            }
        });
        let parsed = parse_group_local_settings_backup(&supplied).expect("parse partial backup");
        let mut normalized = supplied.clone();
        normalized["group"]["chatMode"] = serde_json::json!("sequential");
        assert_eq!(
            serde_json::to_value(&parsed).expect("serialize patch"),
            normalized
        );
        let mut current = GroupLocalSettings {
            chat_mode: GroupChatMode::Jev,
            context_message_window_size: 321,
            ..Default::default()
        };
        current.mode_settings.sequential.speaker_order = vec!["a".into(), "b".into()];
        current.mode_settings.jev.navigator_prompt = "keep current navigator".into();
        current.mode_settings.jev.max_autonomous_rounds = 77;
        current
            .mode_settings
            .jev
            .member_styles
            .insert("a".into(), "old style".into());
        let restored = parsed["group"]
            .apply(&current)
            .expect("merge partial backup");
        assert_eq!(restored.chat_mode, GroupChatMode::Sequential);
        assert_eq!(restored.context_message_window_size, 321);
        assert_eq!(
            restored.mode_settings.jev.navigator_prompt,
            "keep current navigator"
        );
        assert_eq!(restored.mode_settings.jev.max_autonomous_rounds, 77);
        assert_eq!(restored.mode_settings.jev.history_window, 19);
        assert_eq!(restored.mode_settings.sequential.speaker_order, vec!["b"]);
        assert_eq!(
            restored.mode_settings.jev.member_styles,
            BTreeMap::from([("b".into(), "new style".into())])
        );
        let clear = GroupLocalSettingsPatch::parse(&serde_json::json!({
            "modeSettings":{"sequential":{"speakerOrder":[]},"jev":{"memberStyles":{}}}
        }))
        .expect("parse explicit clears");
        let cleared = clear.apply(&restored).expect("apply explicit clears");
        assert!(cleared.mode_settings.sequential.speaker_order.is_empty());
        assert!(cleared.mode_settings.jev.member_styles.is_empty());
        assert_eq!(cleared.mode_settings.jev.history_window, 19);
    }

    #[test]
    fn backup_defaults_invalid_fields_and_discards_unknown_fields_without_resetting_siblings() {
        let mut current = GroupLocalSettings {
            chat_mode: GroupChatMode::Jev,
            enable_context_message_window: true,
            context_message_window_size: 321,
            ..Default::default()
        };
        current.mode_settings.jev.history_window = 19;
        current.mode_settings.jev.max_autonomous_rounds = 71;
        let patch = GroupLocalSettingsPatch::parse(&serde_json::json!({
            "unknown":true, "modeOverride":"invite_only", "enableContextMessageWindow":null,
            "modeSettings": {"jev": {"historyWindow":0, "fallbackPolicy":"invalid", "unknown":1,
                "speakerThreshold":0.7,"memberStyles":{"a":null,"b":"keep"," ":"drop"}}}
        }))
        .unwrap();
        let restored = patch.apply(&current).unwrap();
        assert_eq!(restored.chat_mode, GroupChatMode::Jev);
        assert!(!restored.enable_context_message_window);
        assert_eq!(restored.context_message_window_size, 321);
        assert_eq!(
            restored.mode_settings.jev.history_window,
            JevModeSettings::default().history_window
        );
        assert_eq!(restored.mode_settings.jev.max_autonomous_rounds, 71);
        assert_eq!(restored.mode_settings.jev.speaker_threshold, 0.7);
        assert_eq!(
            restored.mode_settings.jev.fallback_policy,
            JevFallbackPolicy::Stop
        );
        assert_eq!(
            restored.mode_settings.jev.member_styles,
            BTreeMap::from([("a".into(), "".into()), ("b".into(), "keep".into())])
        );
        let encoded = serde_json::to_value(&patch).unwrap();
        assert!(encoded.get("unknown").is_none());
        assert!(encoded.get("modeOverride").is_none());
        assert_eq!(encoded["modeSettings"]["jev"]["historyWindow"], 12);
        let reset_jev =
            GroupLocalSettingsPatch::parse(&serde_json::json!({"modeSettings":{"jev":false}}))
                .unwrap()
                .apply(&current)
                .unwrap();
        assert_eq!(reset_jev.mode_settings.jev, JevModeSettings::default());
        assert_eq!(reset_jev.chat_mode, GroupChatMode::Jev);
        assert_eq!(reset_jev.context_message_window_size, 321);
        let parsed = parse_group_local_settings_backup(&serde_json::json!({
            "group": {"modeSettings":{"sequential":{},"jev":{"unknown":1}}},
            "another": {}
        }))
        .expect("parse empty structures");
        assert!(parsed.is_empty());
    }

    #[tokio::test]
    async fn local_patch_preserves_the_latest_shared_row_and_member_versions() {
        let mut connection = database().await;
        sqlx::raw_sql(
            "INSERT INTO groups(owner_type,group_id,name,mode,config_hash,updated_at)
             VALUES('group','group','remote latest','invite_only','remote-hash',42);
             INSERT INTO group_members VALUES('group','a',0,43);",
        )
        .execute(&mut connection)
        .await
        .expect("seed committed remote state");
        let updates = serde_json::json!({"chatMode":"jev","contextMessageWindowSize":17});
        let mut transaction = connection.begin().await.expect("begin local patch");
        assert!(update_group_local_settings(
            &mut transaction,
            "group",
            updates.as_object().expect("patch")
        )
        .await
        .expect("local patch"));
        transaction.commit().await.expect("commit local patch");
        let shared: (String, String, String, i64) = sqlx::query_as(
            "SELECT name,mode,config_hash,updated_at FROM groups WHERE group_id='group'",
        )
        .fetch_one(&mut connection)
        .await
        .expect("read shared group");
        assert_eq!(
            shared,
            (
                "remote latest".into(),
                "invite_only".into(),
                "remote-hash".into(),
                42
            )
        );
        let member_version: i64 =
            sqlx::query_scalar("SELECT updated_at FROM group_members WHERE group_id='group'")
                .fetch_one(&mut connection)
                .await
                .expect("read shared member");
        assert_eq!(member_version, 43);
        let raw: String = sqlx::query_scalar(
            "SELECT settings_json FROM group_local_settings WHERE group_id='group'",
        )
        .fetch_one(&mut connection)
        .await
        .expect("read local state");
        let local = parse_group_local_settings(Some(&raw));
        assert_eq!(local.effective_mode(), GroupChatMode::Jev);
        assert_eq!(local.context_message_window_size, 17);
        assert!(!update_group_local_settings(
            &mut connection,
            "group",
            updates.as_object().expect("patch")
        )
        .await
        .expect("repeat no-op patch"));
        let mut with_overrides = local;
        with_overrides.mode_settings.sequential.speaker_order = vec!["a".into()];
        with_overrides
            .mode_settings
            .jev
            .member_styles
            .insert("a".into(), "style".into());
        write_group_local_settings(&mut connection, "group", &with_overrides)
            .await
            .unwrap();
        let clear = serde_json::json!({"modeSettings":{"sequential":{"speakerOrder":[]},"jev":{"memberStyles":{}}}});
        update_group_local_settings(&mut connection, "group", clear.as_object().unwrap())
            .await
            .unwrap();
        let raw: String = sqlx::query_scalar(
            "SELECT settings_json FROM group_local_settings WHERE group_id='group'",
        )
        .fetch_one(&mut connection)
        .await
        .unwrap();
        let cleared = parse_group_local_settings(Some(&raw));
        assert!(
            cleared.mode_settings.sequential.speaker_order.is_empty(),
            "clearing the override must not persist the shared member list"
        );
        assert!(cleared.mode_settings.jev.member_styles.is_empty());
        assert_eq!(cleared.context_message_window_size, 17);
    }

    #[tokio::test]
    async fn backup_requires_synced_group_then_explicit_reimport_and_skips_tombstones() {
        let mut connection = database().await;
        let settings = GroupLocalSettings {
            chat_mode: GroupChatMode::Jev,
            ..GroupLocalSettings::default()
        };
        let preferences = parse_group_local_settings_backup(&serde_json::json!({
            "future-group":{"chatMode":"jev"}
        }))
        .expect("parse group patch");
        assert!(import_group_local_settings(&mut connection, &preferences)
            .await
            .expect("report missing group"));
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM group_local_settings")
            .fetch_one(&mut connection)
            .await
            .expect("no unbound preferences");
        assert_eq!(count, 0);
        sqlx::query("INSERT INTO groups(owner_type,group_id,name,updated_at) VALUES('group','future-group','Synced',1)")
            .execute(&mut connection).await.expect("sync group entity");
        assert_eq!(
            read_stored_local_settings(&mut connection, "future-group")
                .await
                .unwrap(),
            GroupLocalSettings::default(),
            "syncing an entity must not automatically restore an earlier backup"
        );
        let mut transaction = connection.begin().await.expect("begin backup import");
        assert!(!import_group_local_settings(&mut transaction, &preferences)
            .await
            .expect("import synced group"));
        transaction.rollback().await.expect("rollback whole backup");
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM group_local_settings")
            .fetch_one(&mut connection)
            .await
            .expect("count rolled back preferences");
        assert_eq!(count, 0);
        assert!(!import_group_local_settings(&mut connection, &preferences)
            .await
            .expect("explicitly import after syncing"));
        let raw: String = sqlx::query_scalar(
            "SELECT settings_json FROM group_local_settings WHERE group_id='future-group'",
        )
        .fetch_one(&mut connection)
        .await
        .expect("read restored preferences");
        assert_eq!(parse_group_local_settings(Some(&raw)), settings);
        sqlx::query("UPDATE groups SET deleted_at=2 WHERE group_id='future-group'")
            .execute(&mut connection)
            .await
            .expect("apply remote tombstone");
        assert!(!import_group_local_settings(&mut connection, &preferences)
            .await
            .expect("deleted group is silently skipped"));
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM group_local_settings")
            .fetch_one(&mut connection)
            .await
            .expect("count after delete");
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn backup_import_merges_with_the_latest_local_row_in_its_transaction() {
        let mut connection = database().await;
        sqlx::query("INSERT INTO groups(owner_type,group_id,name,config_hash,updated_at) VALUES('group','group','Shared','shared-hash',42)")
            .execute(&mut connection).await.expect("seed shared group");
        let patch = parse_group_local_settings_backup(&serde_json::json!({
            "group":{"modeSettings":{"jev":{"historyWindow":9}}}
        }))
        .expect("parse before concurrent local edit");
        let mut latest = GroupLocalSettings::default();
        latest.mode_settings.jev.navigator_prompt = "edited after backup preview".into();
        latest.context_message_window_size = 123;
        write_group_local_settings(&mut connection, "group", &latest)
            .await
            .expect("commit newer local edit");
        let mut transaction = connection.begin().await.expect("begin restore transaction");
        import_group_local_settings(&mut transaction, &patch)
            .await
            .expect("restore supplied field");
        transaction.commit().await.expect("commit restore");
        let raw: String = sqlx::query_scalar(
            "SELECT settings_json FROM group_local_settings WHERE group_id='group'",
        )
        .fetch_one(&mut connection)
        .await
        .expect("read restored local settings");
        let restored = parse_group_local_settings(Some(&raw));
        assert_eq!(restored.mode_settings.jev.history_window, 9);
        assert_eq!(
            restored.mode_settings.jev.navigator_prompt,
            "edited after backup preview"
        );
        assert_eq!(restored.context_message_window_size, 123);
        let shared: (String, i64) =
            sqlx::query_as("SELECT config_hash,updated_at FROM groups WHERE group_id='group'")
                .fetch_one(&mut connection)
                .await
                .expect("shared version");
        assert_eq!(shared, ("shared-hash".into(), 42));
        let empty = BTreeMap::from([(
            "missing-group".into(),
            GroupLocalSettingsPatch::parse(&serde_json::json!({})).expect("empty patch"),
        )]);
        import_group_local_settings(&mut connection, &empty)
            .await
            .expect("ignore empty patch");
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM group_local_settings WHERE group_id='missing-group'",
        )
        .fetch_one(&mut connection)
        .await
        .expect("no empty row");
        assert_eq!(count, 0);
    }
}

// AgentService: 处理智能体(Agent)配置的核心模块 (Facade 层)
// 职责: 作为应用层 Facade，协调数据库存储与业务逻辑，完全面向 SQLite 存储。

use crate::vcp_modules::agent_types::{
    AgentConfig, AgentListItem, AgentLocalPreferences, AgentLocalSettingsPatch,
};
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::group::group_service::GroupManagerState;
use crate::vcp_modules::group::group_types::GroupListItem;
use crate::vcp_modules::infra::utils::desktop_compatible_id_base;
use crate::vcp_modules::persistence::agent_repository;
use crate::vcp_modules::sync_dto::AgentSyncDTO;
use crate::vcp_modules::sync_hash::HashAggregator;
use crate::vcp_modules::sync_service::{SyncCommand, SyncState};
use crate::vcp_modules::sync_types::{DeleteTarget, OwnerType, SYNC_TOMBSTONE_HASH};
use crate::vcp_modules::topic_types::{MessageKey, Topic, TopicKey};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use sqlx::SqliteConnection;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as CacheCommitMutex};

use tauri::{AppHandle, Manager, Runtime, State};
use tokio::sync::Mutex;

/// AgentConfigState 的全局状态
pub struct AgentConfigState {
    /// 配置缓存: agent_id -> AgentConfig
    pub caches: DashMap<String, AgentConfig>,
    /// 任务队列锁: agent_id -> Mutex
    pub locks: DashMap<String, Arc<Mutex<()>>>,
    /// 同步写绕过 Facade 时推进的缓存代次，阻止旧数据库快照迟到回填。
    cache_generation: AtomicU64,
    /// 只保护 generation 校验与 DashMap 提交/清空，不跨任何 await。
    cache_commit: CacheCommitMutex<()>,
}

impl AgentConfigState {
    pub fn new() -> Self {
        Self {
            caches: DashMap::new(),
            locks: DashMap::new(),
            cache_generation: AtomicU64::new(0),
            cache_commit: CacheCommitMutex::new(()),
        }
    }

    pub async fn acquire_lock(&self, agent_id: &str) -> Arc<Mutex<()>> {
        self.locks
            .entry(agent_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .value()
            .clone()
    }

    fn current_cache_generation(&self) -> u64 {
        self.cache_generation.load(Ordering::Acquire)
    }

    fn insert_cache_if_current(&self, agent_id: String, config: AgentConfig, generation: u64) {
        let _commit_guard = self
            .cache_commit
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.cache_generation.load(Ordering::Acquire) == generation {
            self.caches.insert(agent_id, config);
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

pub fn create_default_config(agent_id: &str) -> AgentConfig {
    AgentConfig {
        id: agent_id.to_string(),
        name: "New Agent".to_string(),
        ..AgentConfig::default()
    }
}

/// 🛡️ 前端专属数据加载指令 (仅供 Vue 前端跨进程 IPC 调用)
///
/// ⚠️ 警告：出于数据防泄密及减轻跨端传输 IPC 序列化性能开销的考量，此接口在返回前会【强行清空】`system_prompt`。
/// ❌ 绝对禁止在 Rust 后端业务逻辑、群聊组装或同步推送代码中调用此函数！
/// ➡️ 后端读取完整智能体配置请使用 `read_agent_config_internal`！
#[tauri::command]
pub async fn read_agent_config<R: Runtime>(
    app_handle: AppHandle<R>,
    state: State<'_, AgentConfigState>,
    agent_id: String,
    allow_default: Option<bool>,
) -> Result<AgentConfig, String> {
    let mut config =
        read_agent_config_internal(&app_handle, &state, &agent_id, allow_default).await?;
    config.system_prompt = String::new();
    Ok(config)
}

pub async fn read_agent_config_internal<R: Runtime>(
    app_handle: &AppHandle<R>,
    state: &AgentConfigState,
    agent_id: &str,
    allow_default: Option<bool>,
) -> Result<AgentConfig, String> {
    let mutex = state.acquire_lock(agent_id).await;
    let _lock = mutex.lock().await;
    read_agent_config_locked(app_handle, state, agent_id, allow_default).await
}

async fn read_agent_config_locked<R: Runtime>(
    app_handle: &AppHandle<R>,
    state: &AgentConfigState,
    agent_id: &str,
    allow_default: Option<bool>,
) -> Result<AgentConfig, String> {
    if let Some(cached) = state.caches.get(agent_id) {
        return Ok(cached.value().clone());
    }
    let db_state = app_handle.state::<DbState>();
    let repair_app = app_handle.clone();
    agent_repository::normalize_agent_configs(&db_state, Some(agent_id), move || {
        repair_app.state::<AgentConfigState>().invalidate_cache();
    })
    .await?;
    let cache_generation = state.current_cache_generation();
    let pool = &db_state.pool;

    let agent_row = sqlx::query(
        "SELECT a.name, a.system_prompt, a.mobile_system_prompt, a.model, a.temperature, a.context_token_limit, a.max_output_tokens, a.stream_output, a.use_temperature, a.voice_config, av.dominant_color
         FROM agents a
         LEFT JOIN avatars av ON av.owner_id = a.agent_id AND av.owner_type = 'agent' AND av.deleted_at IS NULL
         WHERE a.owner_type = 'agent' AND a.agent_id = ? AND a.deleted_at IS NULL"
    )
    .bind(agent_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| e.to_string())?;

    if let Some(row) = agent_row {
        use sqlx::Row;
        let topic_rows = sqlx::query(
            "SELECT topic_id, title, created_at, locked, unread, unread_count, msg_count 
             FROM topics WHERE owner_type = 'agent' AND owner_id = ? AND deleted_at IS NULL ORDER BY updated_at DESC"
        )
        .bind(agent_id)
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
                owner_id: agent_id.to_string(),
                owner_type: "agent".to_string(),
            });
        }

        let mut config = agent_repository::decode_agent_row(&row, agent_id)?.config;
        config.topics = topics;

        state.insert_cache_if_current(agent_id.to_string(), config.clone(), cache_generation);
        return Ok(config);
    }

    if allow_default.unwrap_or(false) {
        return Ok(create_default_config(agent_id));
    }

    Err(format!("Agent {} not found", agent_id))
}

#[tauri::command]
pub async fn save_agent_config(
    app_handle: AppHandle,
    agent: AgentConfig,
    baseline: AgentConfig,
) -> Result<AgentConfig, String> {
    if agent.id.is_empty() || agent.id != baseline.id {
        return Err("助手设置保存目标与编辑基准不一致".into());
    }
    // This durable task owns the Agent lock through the physical commit and
    // cache refresh, even if the invoking WebView leaves.
    tokio::spawn(async move {
        let state = app_handle.state::<AgentConfigState>();
        let mutex = state.acquire_lock(&agent.id).await;
        let _lock = mutex.lock().await;
        let db = app_handle.state::<DbState>();
        agent_repository::save_agent_edit(&db, &agent, &baseline).await?;
        state.caches.remove(&agent.id);
        let mut canonical =
            read_agent_config_locked(&app_handle, &state, &agent.id, Some(false)).await?;
        canonical.system_prompt.clear();
        Ok(canonical)
    })
    .await
    .map_err(|error| format!("助手设置保存任务异常：{error}"))?
}

#[tauri::command]
pub async fn get_agents(
    app_handle: AppHandle,
    _state: State<'_, AgentConfigState>,
) -> Result<Vec<AgentConfig>, String> {
    let start_total = std::time::Instant::now();
    let db_state = app_handle.state::<DbState>();
    let repair_app = app_handle.clone();
    agent_repository::normalize_agent_configs(&db_state, None, move || {
        repair_app.state::<AgentConfigState>().invalidate_cache();
    })
    .await?;
    let pool = &db_state.pool;

    let start_agents = std::time::Instant::now();
    // 1. 一次性查询所有未删除的 agents 基础配置 (包括 avatars 主色)
    let agent_rows = sqlx::query(
        "SELECT a.agent_id, a.name, a.system_prompt, a.mobile_system_prompt, a.model, a.temperature, a.context_token_limit, a.max_output_tokens, a.stream_output, a.use_temperature, a.voice_config, av.dominant_color
         FROM agents a
         LEFT JOIN avatars av ON av.owner_id = a.agent_id AND av.owner_type = 'agent' AND av.deleted_at IS NULL
         WHERE a.owner_type = 'agent' AND a.deleted_at IS NULL"
    )
    .fetch_all(pool)
    .await
    .map_err(|e| e.to_string())?;
    let duration_agents = start_agents.elapsed();

    if agent_rows.is_empty() {
        log::info!(
            "[Profile] get_agents total: {}ms (empty)",
            start_total.elapsed().as_millis()
        );
        return Ok(Vec::new());
    }

    let start_mapping = std::time::Instant::now();
    // 2. 组装列表；实体缓存只允许在 per-ID 锁内填充。
    let mut agents = Vec::new();
    for row in agent_rows {
        use sqlx::Row;
        let agent_id: String = row.try_get("agent_id").map_err(|error| error.to_string())?;
        let config = agent_repository::decode_agent_row(&row, &agent_id)?.config;

        agents.push(config);
    }
    let duration_mapping = start_mapping.elapsed();

    log::info!(
        "[Profile] get_agents finished. Total: {}ms | SQL Agents: {}ms | Map & Cache: {}ms",
        start_total.elapsed().as_millis(),
        duration_agents.as_millis(),
        duration_mapping.as_millis()
    );

    Ok(agents)
}

#[tauri::command]
pub async fn update_agent_config<R: Runtime>(
    app_handle: AppHandle<R>,
    agent_id: String,
    updates: serde_json::Value,
) -> Result<AgentConfig, String> {
    let updates = updates
        .as_object()
        .cloned()
        .ok_or_else(|| "助手配置补丁必须为对象".to_string())?;
    tokio::spawn(async move {
        let state = app_handle.state::<AgentConfigState>();
        let mutex = state.acquire_lock(&agent_id).await;
        let _lock = mutex.lock().await;
        state.caches.remove(&agent_id);
        let baseline =
            read_agent_config_locked(&app_handle, &state, &agent_id, Some(false)).await?;
        let mut value = serde_json::to_value(&baseline).map_err(|error| error.to_string())?;
        let object = value.as_object_mut().ok_or("助手配置序列化失败")?;
        for (field, value) in updates {
            if !matches!(
                field.as_str(),
                "name"
                    | "mobileSystemPrompt"
                    | "model"
                    | "temperature"
                    | "contextTokenLimit"
                    | "maxOutputTokens"
                    | "streamOutput"
                    | "useTemperature"
                    | "voice"
            ) {
                return Err(format!("此助手字段不可编辑：{field}"));
            }
            object.insert(field, value);
        }
        let draft: AgentConfig =
            serde_json::from_value(value).map_err(|error| error.to_string())?;
        agent_repository::save_agent_edit(&app_handle.state::<DbState>(), &draft, &baseline)
            .await?;
        state.caches.remove(&agent_id);
        let mut canonical =
            read_agent_config_locked(&app_handle, &state, &agent_id, Some(false)).await?;
        canonical.system_prompt.clear();
        Ok(canonical)
    })
    .await
    .map_err(|error| format!("助手设置保存任务异常：{error}"))?
}

/// 删除 Agent
#[tauri::command]
pub async fn delete_agent(
    app_handle: AppHandle,
    state: State<'_, AgentConfigState>,
    agent_id: String,
) -> Result<bool, String> {
    let deleted_at = delete_agent_internal(&app_handle, &state, &agent_id, None).await?;

    if let Some(sync_state) = app_handle.try_state::<SyncState>() {
        let _ = sync_state.ws_sender.send(SyncCommand::NotifyDelete {
            target: DeleteTarget::Owner {
                owner_type: OwnerType::Agent,
                owner_id: agent_id,
            },
            deleted_at,
        });
    }

    Ok(true)
}

pub async fn delete_agent_internal<R: Runtime>(
    app_handle: &AppHandle<R>,
    state: &AgentConfigState,
    agent_id: &str,
    requested_deleted_at: Option<i64>,
) -> Result<i64, String> {
    // 与 save/update 共享同一把实体锁，避免迟到写入在删除后重新填充 cache。
    // 锁条目有意保留：删除后若同 ID 被重新创建，仍必须沿用同一所有权串行化。
    let mutex = state.acquire_lock(agent_id).await;
    let _lock = mutex.lock().await;

    let db_state = app_handle.state::<DbState>();
    let now = requested_deleted_at.unwrap_or_else(crate::vcp_modules::infra::utils::now_millis);
    if now < 0 {
        return Err("Agent delete requires a non-negative deletedAt".to_string());
    }
    let writer_kind = if requested_deleted_at.is_some() {
        "sync.delete.agent"
    } else {
        "identity.agent.delete"
    };
    let mut tx = db_state.write_transaction(writer_kind).await?;

    let existing_deleted_at: Option<Option<i64>> = sqlx::query_scalar(
        "SELECT deleted_at FROM agents WHERE owner_type = 'agent' AND agent_id = ?",
    )
    .bind(agent_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;
    let tombstone_only = match existing_deleted_at {
        None if requested_deleted_at.is_some() => {
            sqlx::query(
                "INSERT INTO agents (
                    owner_type, agent_id, name, model, config_hash, updated_at, deleted_at
                 ) VALUES ('agent', ?, '', '', ?, ?, ?)",
            )
            .bind(agent_id)
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
            return Err(format!("Agent {agent_id} does not exist"));
        }
        Some(Some(existing)) => {
            tx.commit().await.map_err(|e| e.to_string())?;
            state.caches.remove(agent_id);
            return Ok(existing);
        }
        Some(None) => false,
    };

    let affected_group_ids: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT gm.group_id
         FROM group_members gm
         JOIN groups g ON g.owner_type = 'group' AND g.group_id = gm.group_id
         WHERE gm.agent_id = ? AND g.deleted_at IS NULL
         ORDER BY gm.group_id",
    )
    .bind(agent_id)
    .fetch_all(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;

    if !tombstone_only {
        let agent_delete = sqlx::query(
            "UPDATE agents SET deleted_at = ?
                 WHERE owner_type = 'agent' AND agent_id = ? AND deleted_at IS NULL",
        )
        .bind(now)
        .bind(agent_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
        if agent_delete.rows_affected() != 1 {
            return Err(format!("Agent {agent_id} disappeared during delete"));
        }
    }

    // 级联将该 Agent 下的所有话题标记为逻辑删除
    sqlx::query("UPDATE topics SET deleted_at = ? WHERE owner_id = ? AND owner_type = 'agent' AND deleted_at IS NULL")
        .bind(now)
        .bind(agent_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

    // 级联将该 Agent 下所有话题的所有消息标记为逻辑删除
    sqlx::query(
        "UPDATE messages SET deleted_at = ?
         WHERE owner_type = 'agent' AND owner_id = ? AND deleted_at IS NULL",
    )
    .bind(now)
    .bind(agent_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;

    sqlx::query(
        "DELETE FROM message_attachments
         WHERE owner_type = 'agent' AND owner_id = ?",
    )
    .bind(agent_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;

    sqlx::query("DELETE FROM render_cache WHERE owner_type = 'agent' AND owner_id = ?")
        .bind(agent_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

    sqlx::query(
        "UPDATE avatars SET deleted_at = ?
         WHERE owner_type = 'agent' AND owner_id = ? AND deleted_at IS NULL",
    )
    .bind(now)
    .bind(agent_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;

    let active_keys: Vec<MessageKey> = sqlx::query_as::<_, (String, String)>(
        "SELECT topic_id, msg_id FROM active_generations
         WHERE owner_type = 'agent' AND owner_id = ?",
    )
    .bind(agent_id)
    .fetch_all(&mut *tx)
    .await
    .map_err(|e| e.to_string())?
    .into_iter()
    .map(|(topic_id, msg_id)| MessageKey::new(TopicKey::new("agent", agent_id, topic_id), msg_id))
    .collect();

    // 级联清除该 Agent 下的所有活跃生成，杜绝已删除消息复活
    sqlx::query("DELETE FROM active_generations WHERE owner_id = ? AND owner_type = 'agent'")
        .bind(agent_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

    // Agent 实体删除会改变所有引用它的 Group DTO；保留历史发言与 memberTags，
    // 只移除活跃成员关系并推进对应 Group 的配置哈希/时钟。
    sqlx::query("DELETE FROM group_members WHERE agent_id = ?")
        .bind(agent_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    let group_config_updated_at = crate::vcp_modules::infra::utils::now_millis();
    for group_id in &affected_group_ids {
        HashAggregator::recompute_group_config_hash(&mut tx, group_id, group_config_updated_at)
            .await?;
    }

    tx.commit().await.map_err(|e| e.to_string())?;
    for key in active_keys {
        crate::vcp_modules::vcp_client::discard_deleted_generation(app_handle, &key).await;
    }

    state.caches.remove(agent_id);
    if !affected_group_ids.is_empty() {
        if let Some(group_state) = app_handle.try_state::<GroupManagerState>() {
            group_state.invalidate_cache();
        }
    }
    Ok(now)
}

/// 创建 Agent (原子化数据库插入)
#[tauri::command]
pub async fn create_agent(
    app_handle: AppHandle,
    state: State<'_, AgentConfigState>,
    name: String,
) -> Result<AgentConfig, String> {
    let cache_generation = state.current_cache_generation();
    let timestamp = crate::vcp_modules::infra::utils::now_millis();

    let base_id = desktop_compatible_id_base(&name);
    let agent_id = format!("{}_{}", base_id, timestamp);
    let default_topic_id = format!("topic_{}", timestamp);

    let db_state = app_handle.state::<DbState>();

    let config = AgentConfig {
        id: agent_id.clone(),
        name: name.clone(),
        system_prompt: format!("你是 {}。", name),
        mobile_system_prompt: format!("你是 {}。", name),
        topics: vec![Topic {
            id: default_topic_id.clone(),
            name: "主要对话".to_string(),
            created_at: timestamp,
            locked: true,
            unread: false,
            unread_count: 0,
            msg_count: 0,
            owner_id: agent_id.clone(),
            owner_type: "agent".to_string(),
        }],
        ..AgentConfig::default()
    };

    log::info!("[AgentService] Creating agent '{}' atomically.", agent_id);

    let mut tx = db_state.write_transaction("identity.agent.create").await?;

    // 1. 插入 agents 表
    let dto = AgentSyncDTO::from(&config);
    let config_hash = HashAggregator::compute_agent_config_hash(&dto);
    sqlx::query(
        "INSERT INTO agents (owner_type, agent_id, name, system_prompt, mobile_system_prompt, model, temperature, context_token_limit, max_output_tokens, stream_output, use_temperature, config_hash, updated_at)
         VALUES ('agent', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
    )
    .bind(&agent_id)
    .bind(&config.name)
    .bind(&config.system_prompt)
    .bind(&config.mobile_system_prompt)
    .bind(&config.model)
    .bind(config.temperature)
    .bind(config.context_token_limit)
    .bind(config.max_output_tokens)
    .bind(if config.stream_output { 1 } else { 0 })
    .bind(if config.use_temperature { 1 } else { 0 })
    .bind(&config_hash)
    .bind(timestamp)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;

    for topic in &config.topics {
        sqlx::query(
            "INSERT INTO topics (topic_id, owner_type, owner_id, title, created_at, updated_at) 
             VALUES (?, 'agent', ?, ?, ?, ?)",
        )
        .bind(&topic.id)
        .bind(&agent_id)
        .bind(&topic.name)
        .bind(topic.created_at)
        .bind(timestamp)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

        // 初始化 Topic 自身哈希 (config_hash, content_hash)
        let key = TopicKey::new("agent", &agent_id, &topic.id);
        HashAggregator::bubble_topic_hash(&mut tx, &key).await?;
    }

    // 触发聚合哈希冒泡 (更新 agents.content_hash)
    HashAggregator::bubble_agent_hash(&mut tx, &agent_id).await?;
    tx.commit().await.map_err(|e| e.to_string())?;
    state.insert_cache_if_current(agent_id.clone(), config.clone(), cache_generation);

    Ok(config)
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AssistantsSnapshot {
    pub agents: Vec<AgentListItem>,
    pub groups: Vec<GroupListItem>,
    pub unread_counts: HashMap<String, i32>,
}

#[tauri::command]
pub async fn get_assistants_snapshot(
    app_handle: AppHandle,
    _agent_state: State<'_, AgentConfigState>,
    _group_state: State<'_, GroupManagerState>,
    db_state: State<'_, DbState>,
) -> Result<AssistantsSnapshot, String> {
    let repair_app = app_handle.clone();
    agent_repository::normalize_agent_configs(&db_state, None, move || {
        repair_app.state::<AgentConfigState>().invalidate_cache();
    })
    .await?;
    let repair_app = app_handle.clone();
    crate::vcp_modules::group_service::normalize_group_configurations(&db_state, None, move || {
        repair_app.state::<GroupManagerState>().invalidate_cache();
    })
    .await?;
    build_assistants_snapshot(&db_state.pool).await
}

/// 快照查询主体（与 Tauri State 解耦，便于内存库单测）。
async fn build_assistants_snapshot(pool: &sqlx::SqlitePool) -> Result<AssistantsSnapshot, String> {
    let start_total = std::time::Instant::now();
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;

    // 1. 获取 agents。批量快照不写实体缓存，避免并发删除后迟到回填 ghost。
    let agent_rows = sqlx::query(
        "SELECT a.agent_id, a.name, a.model, av.dominant_color
         FROM agents a
         LEFT JOIN avatars av ON av.owner_id = a.agent_id AND av.owner_type = 'agent' AND av.deleted_at IS NULL
         WHERE a.owner_type = 'agent' AND a.deleted_at IS NULL",
    )
    .fetch_all(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;

    let mut agents_list = Vec::new();
    for row in agent_rows {
        use sqlx::Row;
        let agent_id: String = row.try_get("agent_id").map_err(|error| error.to_string())?;
        let defaults = AgentConfig::default();
        let avatar_calculated_color = row
            .try_get::<Option<String>, _>("dominant_color")
            .ok()
            .flatten();
        let model = row
            .try_get::<Option<String>, _>("model")
            .ok()
            .flatten()
            .unwrap_or(defaults.model);
        let name = row
            .try_get::<Option<String>, _>("name")
            .ok()
            .flatten()
            .unwrap_or(defaults.name);

        agents_list.push(AgentListItem {
            id: agent_id,
            name,
            model,
            avatar_calculated_color,
        });
    }

    // 2. 获取 groups，同样只生成列表快照而不预热实体缓存。
    // 列表只暴露本机有效模式，实体与同步仍保留 groups.mode 的共享值。
    let group_rows = sqlx::query(
        "SELECT g.group_id, g.name, av.dominant_color, local.settings_json
         FROM groups g
         LEFT JOIN avatars av ON av.owner_id = g.group_id AND av.owner_type = 'group' AND av.deleted_at IS NULL
         LEFT JOIN group_local_settings local ON local.group_id = g.group_id
         WHERE g.owner_type = 'group' AND g.deleted_at IS NULL",
    )
    .fetch_all(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;

    let member_rows = sqlx::query(
        "SELECT group_id, agent_id
         FROM group_members 
         ORDER BY group_id, sort_order ASC",
    )
    .fetch_all(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;

    let mut group_members: HashMap<String, Vec<String>> = HashMap::new();
    for mr in member_rows {
        use sqlx::Row;
        let gid: String = mr.try_get("group_id").map_err(|error| error.to_string())?;
        let aid: String = mr.try_get("agent_id").map_err(|error| error.to_string())?;

        group_members
            .entry(gid.clone())
            .or_default()
            .push(aid.clone());
    }

    let mut groups_list = Vec::new();
    for row in group_rows {
        use sqlx::Row;
        let group_id: String = row.try_get("group_id").map_err(|error| error.to_string())?;
        let avatar_calculated_color = row
            .try_get::<Option<String>, _>("dominant_color")
            .ok()
            .flatten();
        let name = match row.try_get::<Option<String>, _>("name") {
            Ok(Some(value)) => value,
            Ok(None) | Err(sqlx::Error::ColumnDecode { .. }) => {
                crate::vcp_modules::group_types::GroupConfig::default().name
            }
            Err(error) => return Err(error.to_string()),
        };

        let settings_json = match row.try_get::<Option<String>, _>("settings_json") {
            Ok(value) => value,
            Err(sqlx::Error::ColumnDecode { .. }) => None,
            Err(error) => return Err(error.to_string()),
        };
        let local_settings =
            crate::vcp_modules::group_types::parse_group_local_settings(settings_json.as_deref());
        let mode = local_settings.effective_mode();
        let members = group_members.remove(&group_id).unwrap_or_default();
        groups_list.push(GroupListItem {
            id: group_id,
            name,
            avatar_calculated_color,
            members,
            mode,
        });
    }

    // 3. 获取 unread_counts
    let unread_rows = sqlx::query(
        "SELECT owner_type, owner_id,
                CAST(COALESCE(SUM(unread_count), 0) AS INTEGER) as total_count,
                MAX(CASE WHEN unread = 1 THEN 1 ELSE 0 END) as has_unread
         FROM topics 
         WHERE deleted_at IS NULL
         GROUP BY owner_type, owner_id",
    )
    .fetch_all(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;

    let mut unread_counts = HashMap::new();
    for row in unread_rows {
        use sqlx::Row;
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
            unread_counts.insert(format!("{owner_type}:{owner_id}"), value);
        }
    }

    tx.commit().await.map_err(|e| e.to_string())?;

    log::info!(
        "[Profile] get_assistants_snapshot total: {}ms | Agents: {} | Groups: {}",
        start_total.elapsed().as_millis(),
        agents_list.len(),
        groups_list.len()
    );

    Ok(AssistantsSnapshot {
        agents: agents_list,
        groups: groups_list,
        unread_counts,
    })
}

/// 导出非删除智能体的本机专属配置（移动端提示词、温度开关、音色）。
pub async fn export_agent_local_settings(
    db: &DbState,
) -> Result<BTreeMap<String, AgentLocalPreferences>, String> {
    use sqlx::Row;
    let rows = sqlx::query(
        "SELECT agent_id, mobile_system_prompt, use_temperature, voice_config
         FROM agents
         WHERE owner_type = 'agent' AND deleted_at IS NULL
         ORDER BY agent_id ASC",
    )
    .fetch_all(&db.pool)
    .await
    .map_err(|error| error.to_string())?;

    let mut preferences = BTreeMap::new();
    for row in rows {
        let agent_id: String = row.try_get("agent_id").map_err(|e| e.to_string())?;
        let mobile_system_prompt: String = row.try_get("mobile_system_prompt").unwrap_or_default();
        let use_temperature = match row.try_get::<i64, _>("use_temperature") {
            Ok(val) => val != 0,
            Err(_) => false,
        };
        let voice_raw: String = row.try_get("voice_config").unwrap_or_default();
        let voice = agent_repository::decode_voice_config(&voice_raw);

        preferences.insert(
            agent_id,
            AgentLocalPreferences {
                mobile_system_prompt,
                use_temperature,
                voice,
            },
        );
    }
    Ok(preferences)
}

#[tauri::command]
pub async fn export_agent_preferences(
    app_handle: AppHandle,
) -> Result<BTreeMap<String, AgentLocalPreferences>, String> {
    let db = app_handle.state::<DbState>();
    export_agent_local_settings(&db).await
}

pub fn parse_agent_local_settings_backup(
    value: &serde_json::Value,
) -> Result<BTreeMap<String, AgentLocalSettingsPatch>, String> {
    let mut preferences = BTreeMap::new();
    let Some(source) = value.as_object() else {
        return Ok(preferences);
    };
    for (agent_id, val) in source {
        if agent_id.trim().is_empty() {
            continue;
        }
        let patch = AgentLocalSettingsPatch::parse(val)?;
        if !patch.is_empty() {
            preferences.insert(agent_id.clone(), patch);
        }
    }
    Ok(preferences)
}

/// 调用者持有 settings 导入事务；本函数不提交。
/// 缺失字段保留原值；仅恢复本机已有存活 Agent。
/// 返回是否有尚未同步的智能体 (missing_agents: bool)，墓碑静默跳过。
pub async fn import_agent_local_settings(
    connection: &mut SqliteConnection,
    preferences: &BTreeMap<String, AgentLocalSettingsPatch>,
) -> Result<bool, String> {
    let mut missing_agents = false;

    for (agent_id, patch) in preferences {
        if agent_id.trim().is_empty() || patch.is_empty() {
            continue;
        }

        use sqlx::Row;
        let row = sqlx::query(
            "SELECT deleted_at, mobile_system_prompt, use_temperature, voice_config
             FROM agents WHERE owner_type = 'agent' AND agent_id = ?",
        )
        .bind(agent_id)
        .fetch_optional(&mut *connection)
        .await
        .map_err(|error| error.to_string())?;

        let row = match row {
            None => {
                missing_agents = true;
                continue;
            }
            Some(r) => r,
        };

        let deleted_at: Option<i64> = row.try_get("deleted_at").map_err(|e| e.to_string())?;
        if deleted_at.is_some() {
            continue;
        }

        let current = AgentLocalPreferences {
            mobile_system_prompt: row.try_get("mobile_system_prompt").unwrap_or_default(),
            use_temperature: match row.try_get::<i64, _>("use_temperature") {
                Ok(val) => val != 0,
                Err(_) => false,
            },
            voice: {
                let raw: String = row.try_get("voice_config").unwrap_or_default();
                agent_repository::decode_voice_config(&raw)
            },
        };

        let restored = patch.apply(&current)?;
        let voice_json = serde_json::to_string(&restored.voice).map_err(|e| e.to_string())?;

        sqlx::query(
            "UPDATE agents SET
                mobile_system_prompt = ?,
                use_temperature = ?,
                voice_config = ?
             WHERE owner_type = 'agent' AND agent_id = ? AND deleted_at IS NULL",
        )
        .bind(&restored.mobile_system_prompt)
        .bind(if restored.use_temperature { 1i64 } else { 0i64 })
        .bind(&voice_json)
        .bind(agent_id)
        .execute(&mut *connection)
        .await
        .map_err(|error| error.to_string())?;
    }

    Ok(missing_agents)
}

#[cfg(test)]
mod cache_generation_tests {
    use super::*;

    #[test]
    fn sync_invalidation_rejects_a_stale_cache_fill() {
        let state = AgentConfigState::new();
        let stale_generation = state.current_cache_generation();

        state.invalidate_cache();
        state.insert_cache_if_current(
            "agent-stale".to_string(),
            create_default_config("agent-stale"),
            stale_generation,
        );

        assert!(!state.caches.contains_key("agent-stale"));

        let current_generation = state.current_cache_generation();
        state.insert_cache_if_current(
            "agent-current".to_string(),
            create_default_config("agent-current"),
            current_generation,
        );
        assert!(state.caches.contains_key("agent-current"));
    }

    /// 列表快照只呈现本机模式；共享 mode 的取值与后续变化不影响手机执行。
    #[tokio::test]
    async fn snapshot_groups_carry_speaking_mode() {
        use crate::vcp_modules::group_types::GroupChatMode;
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("in-memory pool");

        // 快照查询涉及的最小 v2 schema（agents/avatars/groups/group_members/topics）
        sqlx::query(
            "CREATE TABLE agents (owner_type TEXT, agent_id TEXT, name TEXT, model TEXT, deleted_at INTEGER, PRIMARY KEY (owner_type, agent_id));
             CREATE TABLE avatars (owner_id TEXT, owner_type TEXT, dominant_color TEXT, deleted_at INTEGER);
             CREATE TABLE groups (owner_type TEXT, group_id TEXT, name TEXT, mode TEXT, deleted_at INTEGER, PRIMARY KEY (owner_type, group_id));
             CREATE TABLE group_local_settings (group_id TEXT PRIMARY KEY, settings_json TEXT NOT NULL, updated_at INTEGER NOT NULL);
             CREATE TABLE group_members (group_id TEXT, agent_id TEXT, sort_order INTEGER);
             CREATE TABLE topics (owner_type TEXT, owner_id TEXT, unread_count INTEGER, unread INTEGER, deleted_at INTEGER);",
        )
        .execute(&pool)
        .await
        .expect("create schema");

        sqlx::query(
            "INSERT INTO agents (owner_type, agent_id, name, model) VALUES ('agent', 'a1', 'Nova', 'm');
             INSERT INTO groups (owner_type, group_id, name, mode) VALUES ('group', 'g1', '邀约群', 'invite_only');
             INSERT INTO groups (owner_type, group_id, name, mode) VALUES ('group', 'g2', '老群', NULL);
             INSERT INTO group_members (group_id, agent_id, sort_order) VALUES ('g1', 'a1', 0);",
        )
        .execute(&pool)
        .await
        .expect("seed rows");

        let snapshot = build_assistants_snapshot(&pool).await.expect("snapshot");

        let g1 = snapshot.groups.iter().find(|g| g.id == "g1").expect("g1");
        assert_eq!(g1.mode, GroupChatMode::Sequential);
        assert_eq!(g1.members, vec!["a1".to_string()]);

        // 没有本机设置的群使用手机默认，不读取共享 mode。
        let g2 = snapshot.groups.iter().find(|g| g.id == "g2").expect("g2");
        assert_eq!(g2.mode, GroupChatMode::Sequential);

        sqlx::query("INSERT INTO group_local_settings VALUES ('g1', ?, 1)")
            .bind(r#"{"chatMode":"jev"}"#)
            .execute(&pool)
            .await
            .expect("set mobile mode");
        let snapshot = build_assistants_snapshot(&pool)
            .await
            .expect("local snapshot");
        assert_eq!(
            snapshot
                .groups
                .iter()
                .find(|g| g.id == "g1")
                .expect("g1")
                .mode,
            GroupChatMode::Jev
        );
        let shared_mode: String =
            sqlx::query_scalar("SELECT mode FROM groups WHERE group_id = 'g1'")
                .fetch_one(&pool)
                .await
                .expect("shared mode");
        assert_eq!(shared_mode, "invite_only");
        sqlx::query("UPDATE groups SET mode = 'naturerandom' WHERE group_id = 'g1'")
            .execute(&pool)
            .await
            .expect("change shared mode");
        let snapshot = build_assistants_snapshot(&pool)
            .await
            .expect("snapshot after remote edit");
        assert_eq!(
            snapshot
                .groups
                .iter()
                .find(|group| group.id == "g1")
                .expect("g1")
                .mode,
            GroupChatMode::Jev
        );
    }

    #[tokio::test]
    async fn test_import_agent_local_settings_preserves_hash_and_updated_at() {
        use sqlx::sqlite::SqlitePoolOptions;
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();

        sqlx::query(
            "CREATE TABLE agents (
                owner_type TEXT NOT NULL CHECK (owner_type = 'agent'),
                agent_id TEXT NOT NULL,
                name TEXT NOT NULL,
                system_prompt TEXT NOT NULL DEFAULT '',
                mobile_system_prompt TEXT NOT NULL DEFAULT '',
                model TEXT NOT NULL,
                temperature REAL NOT NULL DEFAULT 1,
                context_token_limit INTEGER NOT NULL DEFAULT 0,
                max_output_tokens INTEGER NOT NULL DEFAULT 0,
                stream_output INTEGER NOT NULL DEFAULT 1,
                use_temperature INTEGER NOT NULL DEFAULT 0,
                voice_config TEXT NOT NULL DEFAULT '{}',
                config_hash TEXT NOT NULL DEFAULT '',
                content_hash TEXT NOT NULL DEFAULT '',
                updated_at BIGINT NOT NULL,
                deleted_at BIGINT,
                PRIMARY KEY (owner_type, agent_id)
            );",
        )
        .execute(&pool)
        .await
        .unwrap();

        let original_hash = "fixed_hash_12345";
        let original_content_hash = "fixed_content_67890";
        let original_updated_at: i64 = 1700000000000;

        sqlx::query(
            "INSERT INTO agents (owner_type, agent_id, name, model, mobile_system_prompt, use_temperature, voice_config, config_hash, content_hash, updated_at, deleted_at)
             VALUES ('agent', 'a1', 'Nova', 'gpt', '旧提示词', 0, '{\"enabled\":false}', ?, ?, ?, NULL);"
        )
        .bind(original_hash)
        .bind(original_content_hash)
        .bind(original_updated_at)
        .execute(&pool)
        .await
        .unwrap();

        let mut patch_map = serde_json::Map::new();
        patch_map.insert(
            "mobileSystemPrompt".into(),
            serde_json::json!("全新移动端提示词"),
        );
        patch_map.insert("useTemperature".into(), serde_json::json!(true));
        patch_map.insert(
            "voice".into(),
            serde_json::json!({
                "enabled": true,
                "mode": "builtin",
                "voice": "Mia",
                "stream": true,
                "cloneSampleId": ""
            }),
        );

        let mut prefs = BTreeMap::new();
        prefs.insert("a1".to_string(), AgentLocalSettingsPatch(patch_map));

        let mut tx = pool.begin().await.unwrap();
        let missing = import_agent_local_settings(&mut tx, &prefs).await.unwrap();
        tx.commit().await.unwrap();

        assert!(!missing, "已有实体不应标记为 missing");

        // 校验数据库更新结果
        use sqlx::Row;
        let row = sqlx::query("SELECT mobile_system_prompt, use_temperature, voice_config, config_hash, content_hash, updated_at FROM agents WHERE agent_id = 'a1'")
            .fetch_one(&pool)
            .await
            .unwrap();

        assert_eq!(
            row.get::<String, _>("mobile_system_prompt"),
            "全新移动端提示词"
        );
        assert_eq!(row.get::<i64, _>("use_temperature"), 1);
        assert_eq!(
            row.get::<String, _>("config_hash"),
            original_hash,
            "【铁律】config_hash 严禁修改"
        );
        assert_eq!(
            row.get::<String, _>("content_hash"),
            original_content_hash,
            "【铁律】content_hash 严禁修改"
        );
        assert_eq!(
            row.get::<i64, _>("updated_at"),
            original_updated_at,
            "【铁律】updated_at 严禁修改"
        );
    }

    #[tokio::test]
    async fn test_import_agent_local_settings_handles_missing_and_tombstones() {
        use sqlx::sqlite::SqlitePoolOptions;
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();

        sqlx::query(
            "CREATE TABLE agents (
                owner_type TEXT NOT NULL CHECK (owner_type = 'agent'),
                agent_id TEXT NOT NULL,
                name TEXT NOT NULL,
                system_prompt TEXT NOT NULL DEFAULT '',
                mobile_system_prompt TEXT NOT NULL DEFAULT '',
                model TEXT NOT NULL,
                temperature REAL NOT NULL DEFAULT 1,
                context_token_limit INTEGER NOT NULL DEFAULT 0,
                max_output_tokens INTEGER NOT NULL DEFAULT 0,
                stream_output INTEGER NOT NULL DEFAULT 1,
                use_temperature INTEGER NOT NULL DEFAULT 0,
                voice_config TEXT NOT NULL DEFAULT '{}',
                config_hash TEXT NOT NULL DEFAULT '',
                content_hash TEXT NOT NULL DEFAULT '',
                updated_at BIGINT NOT NULL,
                deleted_at BIGINT,
                PRIMARY KEY (owner_type, agent_id)
            );",
        )
        .execute(&pool)
        .await
        .unwrap();

        // 插入 1 个墓碑 agent
        sqlx::query(
            "INSERT INTO agents (owner_type, agent_id, name, model, mobile_system_prompt, use_temperature, voice_config, updated_at, deleted_at)
             VALUES ('agent', 'tombstone_1', 'Dead', 'gpt', '墓碑旧词', 0, '{}', 1000, 2000);"
        )
        .execute(&pool)
        .await
        .unwrap();

        let mut prefs = BTreeMap::new();
        // 1. 空机缺失的 agent
        let mut patch1 = serde_json::Map::new();
        patch1.insert("mobileSystemPrompt".into(), serde_json::json!("未同步"));
        prefs.insert(
            "missing_agent_1".to_string(),
            AgentLocalSettingsPatch(patch1),
        );

        // 2. 墓碑 agent
        let mut patch2 = serde_json::Map::new();
        patch2.insert(
            "mobileSystemPrompt".into(),
            serde_json::json!("试图修改墓碑"),
        );
        prefs.insert("tombstone_1".to_string(), AgentLocalSettingsPatch(patch2));

        let mut tx = pool.begin().await.unwrap();
        let missing = import_agent_local_settings(&mut tx, &prefs).await.unwrap();
        tx.commit().await.unwrap();

        // 验证 missing 标志
        assert!(missing, "发现本地缺失的 agent，必须返回 missing = true");

        // 验证墓碑未被篡改
        let tomb_prompt: String = sqlx::query_scalar(
            "SELECT mobile_system_prompt FROM agents WHERE agent_id = 'tombstone_1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(tomb_prompt, "墓碑旧词", "墓碑实体绝不应被修改");
    }
}

// AgentService: 处理智能体(Agent)配置的核心模块 (Facade 层)
// 职责: 作为应用层 Facade，协调数据库存储与业务逻辑，完全面向 SQLite 存储。

use crate::vcp_modules::agent_types::{AgentConfig, AgentListItem};
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::group::group_service::GroupManagerState;
use crate::vcp_modules::group::group_types::GroupListItem;
use crate::vcp_modules::sync_dto::AgentSyncDTO;
use crate::vcp_modules::sync_hash::HashAggregator;
use crate::vcp_modules::sync_service::{SyncCommand, SyncState};
use crate::vcp_modules::sync_types::SyncDataType;
use crate::vcp_modules::topic_types::Topic;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
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
        system_prompt: "".to_string(),
        mobile_system_prompt: "".to_string(),
        model: "gemini-2.5-flash".to_string(),
        temperature: 1.0,
        context_token_limit: 1000000,
        max_output_tokens: 64000,
        stream_output: true,
        use_temperature: false,
        avatar_calculated_color: None,
        topics: vec![],
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
    let cache_generation = state.current_cache_generation();

    let db_state = app_handle.state::<DbState>();
    let pool = &db_state.pool;

    let agent_row = sqlx::query(
        "SELECT a.name, a.system_prompt, a.mobile_system_prompt, a.model, a.temperature, a.context_token_limit, a.max_output_tokens, a.stream_output, a.use_temperature, av.dominant_color 
         FROM agents a
         LEFT JOIN avatars av ON av.owner_id = a.agent_id AND av.owner_type = 'agent' AND av.deleted_at IS NULL
         WHERE a.agent_id = ? AND a.deleted_at IS NULL"
    )
    .bind(agent_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| e.to_string())?;

    if let Some(row) = agent_row {
        use sqlx::Row;
        let avatar_calculated_color: Option<String> = row.get("dominant_color");

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

        let config = AgentConfig {
            id: agent_id.to_string(),
            name: row.get("name"),
            system_prompt: row.get("system_prompt"),
            mobile_system_prompt: row.get("mobile_system_prompt"),
            model: row.get("model"),
            temperature: row.get("temperature"),
            context_token_limit: row.get("context_token_limit"),
            max_output_tokens: row.get("max_output_tokens"),
            stream_output: row.get::<i32, _>("stream_output") != 0,
            use_temperature: row.get::<i32, _>("use_temperature") != 0,
            avatar_calculated_color,
            topics,
        };

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
    state: State<'_, AgentConfigState>,
    mut agent: AgentConfig,
) -> Result<bool, String> {
    let agent_id = if agent.id.is_empty() {
        return Err("Agent ID cannot be empty".to_string());
    } else {
        agent.id.clone()
    };

    let mutex = state.acquire_lock(&agent_id).await;
    let _lock = mutex.lock().await;

    // 🛡️ 防擦除合并：从内存缓存或数据库中加载原有的 system_prompt 以防空值覆写
    if let Some(cached) = state.caches.get(&agent_id) {
        agent.system_prompt = cached.value().system_prompt.clone();
    } else {
        if let Ok(db_config) =
            read_agent_config_locked(&app_handle, &state, &agent_id, Some(false)).await
        {
            agent.system_prompt = db_config.system_prompt;
        }
    }

    internal_write_agent_config(&app_handle, &state, &agent_id, &agent, false, false).await
}

#[tauri::command]
pub async fn get_agents(
    app_handle: AppHandle,
    _state: State<'_, AgentConfigState>,
) -> Result<Vec<AgentConfig>, String> {
    let start_total = std::time::Instant::now();
    let db_state = app_handle.state::<DbState>();
    let pool = &db_state.pool;

    let start_agents = std::time::Instant::now();
    // 1. 一次性查询所有未删除的 agents 基础配置 (包括 avatars 主色)
    let agent_rows = sqlx::query(
        "SELECT a.agent_id, a.name, a.system_prompt, a.mobile_system_prompt, a.model, a.temperature, a.context_token_limit, a.max_output_tokens, a.stream_output, a.use_temperature, av.dominant_color 
         FROM agents a
         LEFT JOIN avatars av ON av.owner_id = a.agent_id AND av.owner_type = 'agent' AND av.deleted_at IS NULL
         WHERE a.deleted_at IS NULL"
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
        let agent_id: String = row.get("agent_id");
        let avatar_calculated_color: Option<String> = row.get("dominant_color");

        let config = AgentConfig {
            id: agent_id.clone(),
            name: row.get("name"),
            system_prompt: row.get("system_prompt"),
            mobile_system_prompt: row.get("mobile_system_prompt"),
            model: row.get("model"),
            temperature: row.get("temperature"),
            context_token_limit: row.get("context_token_limit"),
            max_output_tokens: row.get("max_output_tokens"),
            stream_output: row.get::<i32, _>("stream_output") != 0,
            use_temperature: row.get::<i32, _>("use_temperature") != 0,
            avatar_calculated_color,
            topics: vec![], // 优化：不加载 topics 列表，改由前端点击时流式按需懒加载
        };

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
    state: State<'_, AgentConfigState>,
    agent_id: String,
    updates: serde_json::Value,
) -> Result<AgentConfig, String> {
    let mutex = state.acquire_lock(&agent_id).await;
    let _lock = mutex.lock().await;

    // 1. 读取当前配置
    let mut config = read_agent_config_locked(&app_handle, &state, &agent_id, Some(true)).await?;
    config.system_prompt.clear();

    // 2. 将更新合并到当前配置 (JSON 层级合并)
    let mut config_val = serde_json::to_value(&config).map_err(|e| e.to_string())?;
    if let Some(updates_obj) = updates.as_object() {
        if let Some(config_obj) = config_val.as_object_mut() {
            for (k, v) in updates_obj {
                config_obj.insert(k.clone(), v.clone());
            }
        }
    }

    let new_config: AgentConfig = serde_json::from_value(config_val).map_err(|e| e.to_string())?;

    // 3. 写入数据库 (原子化更新)
    internal_write_agent_config(&app_handle, &state, &agent_id, &new_config, false, false).await?;

    Ok(new_config)
}
async fn internal_write_agent_config<R: Runtime>(
    app_handle: &AppHandle<R>,
    state: &AgentConfigState,
    agent_id: &str,
    new_config: &AgentConfig,
    skip_bubble: bool,
    from_sync: bool,
) -> Result<bool, String> {
    let cache_generation = state.current_cache_generation();
    let db_state = app_handle.state::<DbState>();
    let pool = &db_state.pool;
    let now = crate::vcp_modules::infra::utils::now_millis();

    // 🛡️ 防擦除防线：如果前端发回的对象提示词为空，从 caches 或 SQLite 数据库提取原有 system_prompt 兜底
    let mut final_config = new_config.clone();
    if final_config.system_prompt.is_empty() {
        if let Some(cached) = state.caches.get(agent_id) {
            final_config.system_prompt = cached.value().system_prompt.clone();
        } else {
            let row = sqlx::query(
                "SELECT system_prompt FROM agents WHERE agent_id = ? AND deleted_at IS NULL",
            )
            .bind(agent_id)
            .fetch_optional(pool)
            .await
            .map_err(|e| e.to_string())?;
            if let Some(r) = row {
                use sqlx::Row;
                final_config.system_prompt = r.get("system_prompt");
            }
        }
    }

    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;

    let deleted_at =
        sqlx::query_scalar::<_, Option<i64>>("SELECT deleted_at FROM agents WHERE agent_id = ?")
            .bind(agent_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
    if matches!(deleted_at, Some(Some(_))) {
        return Err(format!("Agent {agent_id} has been deleted"));
    }

    // 计算基于 DTO 的决定性哈希
    let dto = AgentSyncDTO::from(&final_config);
    let config_hash = HashAggregator::compute_agent_config_hash(&dto);

    // 只有非同步来源且哈希发生变化时，才通知同步中心
    if !from_sync {
        if let Some(sync_state) = app_handle.try_state::<SyncState>() {
            let rows = sqlx::query("SELECT config_hash FROM agents WHERE agent_id = ?")
                .bind(agent_id)
                .fetch_optional(pool)
                .await
                .map_err(|e| e.to_string())?;

            let old_hash = rows.and_then(|r| {
                use sqlx::Row;
                r.get::<Option<String>, _>("config_hash")
            });

            if old_hash.as_ref() != Some(&config_hash) {
                let _ = sync_state.ws_sender.send(SyncCommand::NotifyLocalChange {
                    id: agent_id.to_string(),
                    data_type: SyncDataType::Agent,
                    hash: config_hash.clone(),
                    ts: now,
                });
            }
        }
    }

    sqlx::query(
        "INSERT INTO agents (
            agent_id, name, system_prompt, mobile_system_prompt, model, temperature, 
            context_token_limit, max_output_tokens, 
            stream_output, use_temperature, config_hash, updated_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(agent_id) DO UPDATE SET
            name = excluded.name, 
            system_prompt = excluded.system_prompt,
            mobile_system_prompt = excluded.mobile_system_prompt,
            model = excluded.model, 
            temperature = excluded.temperature, 
            context_token_limit = excluded.context_token_limit, 
            max_output_tokens = excluded.max_output_tokens, 
            stream_output = excluded.stream_output, 
            use_temperature = excluded.use_temperature,
            config_hash = excluded.config_hash,
            updated_at = excluded.updated_at",
    )
    .bind(agent_id)
    .bind(&final_config.name)
    .bind(&final_config.system_prompt)
    .bind(&final_config.mobile_system_prompt)
    .bind(&final_config.model)
    .bind(final_config.temperature)
    .bind(final_config.context_token_limit)
    .bind(final_config.max_output_tokens)
    .bind(if final_config.stream_output { 1 } else { 0 })
    .bind(if final_config.use_temperature { 1 } else { 0 })
    .bind(&config_hash)
    .bind(now)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;

    // 更新话题 (Upsert)
    if !new_config.topics.is_empty() {
        for topic in &new_config.topics {
            sqlx::query(
                "INSERT INTO topics (
                    topic_id, owner_type, owner_id, title,
                    created_at, updated_at, locked, unread
                ) VALUES (?, 'agent', ?, ?, ?, ?, ?, ?)
                 ON CONFLICT(topic_id) DO UPDATE SET
                    title = excluded.title,
                    locked = excluded.locked,
                    unread = excluded.unread,
                    updated_at = excluded.updated_at",
            )
            .bind(&topic.id)
            .bind(agent_id)
            .bind(&topic.name)
            .bind(topic.created_at)
            .bind(now)
            .bind(topic.locked)
            .bind(topic.unread)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;

            // 初始化/更新 Topic 自身哈希 (config_hash, content_hash)
            HashAggregator::bubble_topic_hash(&mut tx, &topic.id).await?;
        }
    }

    // 触发聚合哈希冒泡 (更新 agents.content_hash)
    // 只有在明确要求冒泡且话题列表不为空（即话题可能有变动）时才执行
    if !skip_bubble && !new_config.topics.is_empty() {
        HashAggregator::bubble_agent_hash(&mut tx, agent_id).await?;
    }
    tx.commit().await.map_err(|e| e.to_string())?;

    state.insert_cache_if_current(agent_id.to_string(), final_config.clone(), cache_generation);

    Ok(true)
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
            data_type: SyncDataType::Agent,
            id: agent_id,
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
    let pool = &db_state.pool;
    let now = requested_deleted_at.unwrap_or_else(crate::vcp_modules::infra::utils::now_millis);
    if now < 0 {
        return Err("Agent delete requires a non-negative deletedAt".to_string());
    }
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;

    let existing_deleted_at: Option<Option<i64>> =
        sqlx::query_scalar("SELECT deleted_at FROM agents WHERE agent_id = ?")
            .bind(agent_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
    match existing_deleted_at {
        None => return Err(format!("Agent {agent_id} does not exist")),
        Some(Some(existing)) => {
            tx.commit().await.map_err(|e| e.to_string())?;
            state.caches.remove(agent_id);
            return Ok(existing);
        }
        Some(None) => {}
    }

    let agent_delete =
        sqlx::query("UPDATE agents SET deleted_at = ? WHERE agent_id = ? AND deleted_at IS NULL")
            .bind(now)
            .bind(agent_id)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
    if agent_delete.rows_affected() != 1 {
        return Err(format!("Agent {agent_id} disappeared during delete"));
    }

    // 级联将该 Agent 下的所有话题标记为逻辑删除
    sqlx::query("UPDATE topics SET deleted_at = ? WHERE owner_id = ? AND owner_type = 'agent' AND deleted_at IS NULL")
        .bind(now)
        .bind(agent_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

    // 级联将该 Agent 下所有话题的所有消息标记为逻辑删除
    sqlx::query("UPDATE messages SET deleted_at = ? WHERE topic_id IN (SELECT topic_id FROM topics WHERE owner_id = ? AND owner_type = 'agent') AND deleted_at IS NULL")
        .bind(now)
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

    // 级联清除该 Agent 下的所有活跃生成，杜绝已删除消息复活
    sqlx::query("DELETE FROM active_generations WHERE owner_id = ? AND owner_type = 'agent'")
        .bind(agent_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

    tx.commit().await.map_err(|e| e.to_string())?;

    state.caches.remove(agent_id);
    Ok(now)
}

/// 创建 Agent (原子化数据库插入)
#[tauri::command]
pub async fn create_agent(
    app_handle: AppHandle,
    state: State<'_, AgentConfigState>,
    name: String,
    initial_config: Option<serde_json::Value>,
) -> Result<AgentConfig, String> {
    let cache_generation = state.current_cache_generation();
    let timestamp = crate::vcp_modules::infra::utils::now_millis();

    let base_id = name
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
        .collect::<String>();
    let agent_id = format!("{}_{}", base_id, timestamp);
    let default_topic_id = format!("topic_{}", timestamp);

    let db_state = app_handle.state::<DbState>();
    let pool = &db_state.pool;

    let config = if let Some(init) = initial_config {
        let mut c: AgentConfig = serde_json::from_value(init).map_err(|e| e.to_string())?;
        c.id = agent_id.clone();
        c.name = name;
        c
    } else {
        AgentConfig {
            id: agent_id.clone(),
            name: name.clone(),
            system_prompt: format!("你是 {}。", name),
            mobile_system_prompt: format!("你是 {}。", name),
            model: "gemini-2.5-flash".to_string(),
            temperature: 0.7,
            context_token_limit: 1000000,
            max_output_tokens: 60000,
            stream_output: true,
            use_temperature: false,
            avatar_calculated_color: None,
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
        }
    };

    log::info!("[AgentService] Creating agent '{}' atomically.", agent_id);

    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;

    // 1. 插入 agents 表
    let dto = AgentSyncDTO::from(&config);
    let config_hash = HashAggregator::compute_agent_config_hash(&dto);
    sqlx::query(
        "INSERT INTO agents (agent_id, name, system_prompt, mobile_system_prompt, model, temperature, context_token_limit, max_output_tokens, stream_output, use_temperature, config_hash, updated_at) 
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
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
        HashAggregator::bubble_topic_hash(&mut tx, &topic.id).await?;
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
    _agent_state: State<'_, AgentConfigState>,
    _group_state: State<'_, GroupManagerState>,
    db_state: State<'_, DbState>,
) -> Result<AssistantsSnapshot, String> {
    build_assistants_snapshot(&db_state.pool).await
}

/// 快照查询主体（与 Tauri State 解耦，便于内存库单测）。
async fn build_assistants_snapshot(
    pool: &sqlx::SqlitePool,
) -> Result<AssistantsSnapshot, String> {
    let start_total = std::time::Instant::now();

    // 1. 获取 agents。批量快照不写实体缓存，避免并发删除后迟到回填 ghost。
    let agent_rows = sqlx::query(
        "SELECT a.agent_id, a.name, a.model, av.dominant_color
         FROM agents a
         LEFT JOIN avatars av ON av.owner_id = a.agent_id AND av.owner_type = 'agent' AND av.deleted_at IS NULL
         WHERE a.deleted_at IS NULL",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| e.to_string())?;

    let mut agents_list = Vec::new();
    for row in agent_rows {
        use sqlx::Row;
        let agent_id: String = row.get("agent_id");
        let avatar_calculated_color: Option<String> = row.get("dominant_color");
        let model: String = row.get("model");
        let name: String = row.get("name");

        agents_list.push(AgentListItem {
            id: agent_id,
            name,
            model,
            avatar_calculated_color,
        });
    }

    // 2. 获取 groups，同样只生成列表快照而不预热实体缓存。
    // mode 必须随快照下发：前端邀约横条/@选择器的模式判定只读列表快照。
    let group_rows = sqlx::query(
        "SELECT g.group_id, g.name, COALESCE(g.mode, 'sequential') AS mode, av.dominant_color
         FROM groups g
         LEFT JOIN avatars av ON av.owner_id = g.group_id AND av.owner_type = 'group' AND av.deleted_at IS NULL
         WHERE g.deleted_at IS NULL",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| e.to_string())?;

    let member_rows = sqlx::query(
        "SELECT group_id, agent_id
         FROM group_members 
         ORDER BY group_id, sort_order ASC",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| e.to_string())?;

    let mut group_members: HashMap<String, Vec<String>> = HashMap::new();
    for mr in member_rows {
        use sqlx::Row;
        let gid: String = mr.get("group_id");
        let aid: String = mr.get("agent_id");

        group_members
            .entry(gid.clone())
            .or_default()
            .push(aid.clone());
    }

    let mut groups_list = Vec::new();
    for row in group_rows {
        use sqlx::Row;
        let group_id: String = row.get("group_id");
        let avatar_calculated_color: Option<String> = row.get("dominant_color");
        let name: String = row.get("name");

        let mode: String = row.get("mode");
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

    let mut unread_counts = HashMap::new();
    for row in unread_rows {
        use sqlx::Row;
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
            unread_counts.insert(owner_id, value);
        }
    }

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

    /// 回归契约：列表快照必须携带 groups.mode——前端邀约横条/@选择器的
    /// 模式判定只读快照，DTO 丢字段曾导致 invite_only 群重启后永不识别。
    #[tokio::test]
    async fn snapshot_groups_carry_speaking_mode() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("in-memory pool");

        // 快照查询涉及的最小 schema（agents/avatars/groups/group_members/topics）
        sqlx::query(
            "CREATE TABLE agents (agent_id TEXT PRIMARY KEY, name TEXT, model TEXT, deleted_at INTEGER);
             CREATE TABLE avatars (owner_id TEXT, owner_type TEXT, dominant_color TEXT, deleted_at INTEGER);
             CREATE TABLE groups (group_id TEXT PRIMARY KEY, name TEXT, mode TEXT, deleted_at INTEGER);
             CREATE TABLE group_members (group_id TEXT, agent_id TEXT, sort_order INTEGER);
             CREATE TABLE topics (owner_id TEXT, unread_count INTEGER, unread INTEGER, deleted_at INTEGER);",
        )
        .execute(&pool)
        .await
        .expect("create schema");

        sqlx::query(
            "INSERT INTO agents (agent_id, name, model) VALUES ('a1', 'Nova', 'm');
             INSERT INTO groups (group_id, name, mode) VALUES ('g1', '邀约群', 'invite_only');
             INSERT INTO groups (group_id, name, mode) VALUES ('g2', '老群', NULL);
             INSERT INTO group_members (group_id, agent_id, sort_order) VALUES ('g1', 'a1', 0);",
        )
        .execute(&pool)
        .await
        .expect("seed rows");

        let snapshot = build_assistants_snapshot(&pool).await.expect("snapshot");

        let g1 = snapshot.groups.iter().find(|g| g.id == "g1").expect("g1");
        assert_eq!(g1.mode, "invite_only");
        assert_eq!(g1.members, vec!["a1".to_string()]);

        // 历史数据 mode 可能为 NULL：必须回退 sequential 而非查询失败
        let g2 = snapshot.groups.iter().find(|g| g.id == "g2").expect("g2");
        assert_eq!(g2.mode, "sequential");
    }
}

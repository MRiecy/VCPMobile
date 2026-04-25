// AgentService: 处理智能体(Agent)配置的核心模块 (Facade 层)
// 职责: 作为应用层 Facade，协调数据库存储与业务逻辑，完全面向 SQLite 存储。

use crate::vcp_modules::agent_types::AgentConfig;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::sync_dto::AgentSyncDTO;
use crate::vcp_modules::sync_hash::HashAggregator;
use crate::vcp_modules::sync_service::{SyncCommand, SyncState};
use crate::vcp_modules::sync_types::SyncDataType;
use crate::vcp_modules::topic_types::Topic;
use dashmap::DashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager, Runtime, State};
use tokio::sync::Mutex;

/// AgentConfigState 的全局状态
pub struct AgentConfigState {
    /// 配置缓存: agent_id -> AgentConfig
    pub caches: DashMap<String, AgentConfig>,
    /// 任务队列锁: agent_id -> Mutex
    pub locks: DashMap<String, Arc<Mutex<()>>>,
}

impl AgentConfigState {
    pub fn new() -> Self {
        Self {
            caches: DashMap::new(),
            locks: DashMap::new(),
        }
    }

    pub async fn acquire_lock(&self, agent_id: &str) -> Arc<Mutex<()>> {
        self.locks
            .entry(agent_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .value()
            .clone()
    }
}

pub fn create_default_config(agent_id: &str) -> AgentConfig {
    AgentConfig {
        id: agent_id.to_string(),
        name: "New Agent".to_string(),
        system_prompt: "".to_string(),
        model: "gemini-2.5-flash".to_string(),
        temperature: 1.0,
        context_token_limit: 1000000,
        max_output_tokens: 64000,
        stream_output: true,
        avatar_calculated_color: None,
        topics: vec![],
        current_topic_id: None,
    }
}

#[tauri::command]
pub async fn read_agent_config<R: Runtime>(
    app_handle: AppHandle<R>,
    state: State<'_, AgentConfigState>,
    agent_id: String,
    allow_default: Option<bool>,
) -> Result<AgentConfig, String> {
    read_agent_config_internal(&app_handle, &state, &agent_id, allow_default).await
}

pub async fn read_agent_config_internal<R: Runtime>(
    app_handle: &AppHandle<R>,
    state: &AgentConfigState,
    agent_id: &str,
    allow_default: Option<bool>,
) -> Result<AgentConfig, String> {
    if let Some(cached) = state.caches.get(agent_id) {
        return Ok(cached.value().clone());
    }

    let db_state = app_handle.state::<DbState>();
    let pool = &db_state.pool;

    let agent_row = sqlx::query(
        "SELECT a.name, a.system_prompt, a.model, a.temperature, a.context_token_limit, a.max_output_tokens, a.stream_output, a.current_topic_id, av.dominant_color 
         FROM agents a
         LEFT JOIN avatars av ON av.owner_id = a.agent_id AND av.owner_type = 'agent'
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
            model: row.get("model"),
            temperature: row.get("temperature"),
            context_token_limit: row.get("context_token_limit"),
            max_output_tokens: row.get("max_output_tokens"),
            stream_output: row.get::<i32, _>("stream_output") != 0,
            avatar_calculated_color,
            topics,
            current_topic_id: row.get("current_topic_id"),
        };

        state.caches.insert(agent_id.to_string(), config.clone());
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
    agent: AgentConfig,
) -> Result<bool, String> {
    let agent_id = if agent.id.is_empty() {
        return Err("Agent ID cannot be empty".to_string());
    } else {
        agent.id.clone()
    };

    let mutex = state.acquire_lock(&agent_id).await;
    let _lock = mutex.lock().await;

    internal_write_agent_config(&app_handle, &state, &agent_id, &agent, false, false).await
}

#[tauri::command]
pub async fn get_agents(
    app_handle: AppHandle,
    state: State<'_, AgentConfigState>,
) -> Result<Vec<AgentConfig>, String> {
    let db_state = app_handle.state::<DbState>();
    let pool = &db_state.pool;

    let rows = sqlx::query("SELECT agent_id FROM agents WHERE deleted_at IS NULL")
        .fetch_all(pool)
        .await
        .map_err(|e| e.to_string())?;

    let mut agents = Vec::new();
    for row in rows {
        use sqlx::Row;
        let agent_id: String = row.get("agent_id");
        if let Ok(config) =
            read_agent_config(app_handle.clone(), state.clone(), agent_id, None).await
        {
            agents.push(config);
        }
    }

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
    let config = read_agent_config(
        app_handle.clone(),
        state.clone(),
        agent_id.clone(),
        Some(true),
    )
    .await?;

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
/// 接收来自同步中心的 DTO 并局部应用到本地 (同步专用)
#[allow(dead_code)]
pub async fn apply_sync_update<R: Runtime>(
    app_handle: &AppHandle<R>,
    state: &AgentConfigState,
    agent_id: &str,
    dto: AgentSyncDTO,
    skip_bubble: bool,
    from_sync: bool,
) -> Result<(), String> {
    let mutex = state.acquire_lock(agent_id).await;
    let _lock = mutex.lock().await;

    // 1. 读取当前配置 (如果不存在则创建默认)
    let mut config = read_agent_config_internal(app_handle, state, agent_id, Some(true)).await?;

    // 2. 局部覆盖：将 DTO 字段应用到 config
    config.name = dto.name;
    config.system_prompt = dto.system_prompt;
    config.model = dto.model;
    config.temperature = dto.temperature;
    config.context_token_limit = dto.context_token_limit;
    config.max_output_tokens = dto.max_output_tokens;
    config.stream_output = dto.stream_output;

    // 3. 写入数据库
    internal_write_agent_config(app_handle, state, agent_id, &config, skip_bubble, from_sync)
        .await?;

    Ok(())
}

async fn internal_write_agent_config<R: Runtime>(
    app_handle: &AppHandle<R>,
    state: &AgentConfigState,
    agent_id: &str,
    new_config: &AgentConfig,
    skip_bubble: bool,
    from_sync: bool,
) -> Result<bool, String> {
    let db_state = app_handle.state::<DbState>();
    let pool = &db_state.pool;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;

    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;

    // 计算基于 DTO 的决定性哈希
    let dto = AgentSyncDTO::from(new_config);
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
            agent_id, name, system_prompt, model, temperature, 
            context_token_limit, max_output_tokens, 
            stream_output, config_hash, current_topic_id, updated_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(agent_id) DO UPDATE SET
            name = excluded.name, 
            system_prompt = excluded.system_prompt, 
            model = excluded.model, 
            temperature = excluded.temperature, 
            context_token_limit = excluded.context_token_limit, 
            max_output_tokens = excluded.max_output_tokens, 
            stream_output = excluded.stream_output, 
            config_hash = excluded.config_hash,
            current_topic_id = excluded.current_topic_id,
            updated_at = excluded.updated_at",
    )
    .bind(agent_id)
    .bind(&new_config.name)
    .bind(&new_config.system_prompt)
    .bind(&new_config.model)
    .bind(new_config.temperature)
    .bind(new_config.context_token_limit)
    .bind(new_config.max_output_tokens)
    .bind(if new_config.stream_output { 1 } else { 0 })
    .bind(&config_hash)
    .bind(&new_config.current_topic_id)
    .bind(now)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;

    // 更新话题 (Upsert)
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
    }

    tx.commit().await.map_err(|e| e.to_string())?;

    // 触发聚合哈希冒泡 (更新 agents.content_hash)
    if !skip_bubble {
        let mut bubble_tx = pool.begin().await.map_err(|e| e.to_string())?;
        HashAggregator::bubble_agent_hash(&mut bubble_tx, agent_id).await?;
        bubble_tx.commit().await.map_err(|e| e.to_string())?;
    }

    state
        .caches
        .insert(agent_id.to_string(), new_config.clone());

    Ok(true)
}

/// 删除 Agent
#[tauri::command]
pub async fn delete_agent(
    app_handle: AppHandle,
    state: State<'_, AgentConfigState>,
    agent_id: String,
) -> Result<bool, String> {
    let db_state = app_handle.state::<DbState>();
    let pool = &db_state.pool;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;

    sqlx::query("UPDATE agents SET deleted_at = ? WHERE agent_id = ?")
        .bind(now)
        .bind(&agent_id)
        .execute(pool)
        .await
        .map_err(|e| e.to_string())?;

    state.caches.remove(&agent_id);

    if let Some(sync_state) = app_handle.try_state::<SyncState>() {
        let _ = sync_state.ws_sender.send(SyncCommand::NotifyDelete {
            data_type: SyncDataType::Agent,
            id: agent_id.clone(),
        });
    }

    Ok(true)
}

/// 创建 Agent (原子化数据库插入)
#[tauri::command]
pub async fn create_agent(
    app_handle: AppHandle,
    state: State<'_, AgentConfigState>,
    name: String,
    initial_config: Option<serde_json::Value>,
) -> Result<AgentConfig, String> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;

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
            model: "gemini-2.5-flash".to_string(),
            temperature: 0.7,
            context_token_limit: 1000000,
            max_output_tokens: 60000,
            stream_output: true,
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
            current_topic_id: Some(default_topic_id.clone()),
        }
    };

    log::info!("[AgentService] Creating agent '{}' atomically.", agent_id);

    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;

    // 1. 插入 agents 表
    let dto = AgentSyncDTO::from(&config);
    let config_hash = HashAggregator::compute_agent_config_hash(&dto);
    sqlx::query(
        "INSERT INTO agents (agent_id, name, system_prompt, model, temperature, context_token_limit, max_output_tokens, stream_output, config_hash, current_topic_id, updated_at) 
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
    )
    .bind(&agent_id)
    .bind(&config.name)
    .bind(&config.system_prompt)
    .bind(&config.model)
    .bind(config.temperature)
    .bind(config.context_token_limit)
    .bind(config.max_output_tokens)
    .bind(if config.stream_output { 1 } else { 0 })
    .bind(&config_hash)
    .bind(&config.current_topic_id)
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
    }

    tx.commit().await.map_err(|e| e.to_string())?;

    // 触发聚合哈希冒泡
    let mut bubble_tx = pool.begin().await.map_err(|e| e.to_string())?;
    HashAggregator::bubble_agent_hash(&mut bubble_tx, &agent_id).await?;
    bubble_tx.commit().await.map_err(|e| e.to_string())?;

    state.caches.insert(agent_id.clone(), config.clone());

    Ok(config)
}

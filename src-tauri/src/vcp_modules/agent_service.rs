// AgentService: 处理智能体(Agent)配置的核心模块 (Facade 层)
// 职责: 作为应用层 Facade，协调数据库存储与业务逻辑，完全面向 SQLite 存储。

use crate::vcp_modules::agent_types::{AgentConfig, RegexRule};
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::topic_types::Topic;
use dashmap::DashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager, State};
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
        top_p: None,
        top_k: None,
        stream_output: true,
        tts_voice_primary: None,
        tts_regex_primary: None,
        tts_voice_secondary: None,
        tts_regex_secondary: None,
        tts_speed: 1.0,
        avatar_border_color: None,
        avatar_calculated_color: None,
        name_text_color: None,
        custom_css: None,
        card_css: None,
        chat_css: None,
        disable_custom_colors: false,
        use_theme_colors_in_chat: true,
        ui_collapse_states: None,
        strip_regexes: vec![],
        topics: vec![],
        extra: serde_json::Map::new(),
    }
}

#[tauri::command]
pub async fn read_agent_config(
    app_handle: AppHandle,
    state: State<'_, AgentConfigState>,
    agent_id: String,
    allow_default: Option<bool>,
) -> Result<AgentConfig, String> {
    if let Some(cached) = state.caches.get(&agent_id) {
        return Ok(cached.value().clone());
    }

    let db_state = app_handle.state::<DbState>();
    let pool = &db_state.pool;

    let agent_row = sqlx::query(
        "SELECT a.name, a.system_prompt, a.model, a.temperature, a.context_token_limit, a.max_output_tokens, a.extra_json, av.dominant_color 
         FROM agents a
         LEFT JOIN avatars av ON av.owner_id = a.agent_id AND av.owner_type = 'agent'
         WHERE a.agent_id = ? AND a.deleted_at IS NULL"
    )
    .bind(&agent_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| e.to_string())?;

    if let Some(row) = agent_row {
        use sqlx::Row;
        let avatar_calculated_color: Option<String> = row.get("dominant_color");
        let mut extra: serde_json::Map<String, serde_json::Value> =
            if let Some(ej) = row.get::<Option<String>, _>("extra_json") {
                serde_json::from_str(&ej).unwrap_or_default()
            } else {
                serde_json::Map::new()
            };

        let rule_rows = sqlx::query(
            "SELECT rule_id, title, find_pattern, replace_with, apply_to_roles, apply_to_frontend, apply_to_context, min_depth, max_depth 
             FROM agent_regex_rules WHERE agent_id = ? AND deleted_at IS NULL ORDER BY sort_order ASC"
        )
        .bind(&agent_id)
        .fetch_all(pool)
        .await
        .map_err(|e| e.to_string())?;

        let mut strip_regexes = Vec::new();
        for rr in rule_rows {
            let roles_json: String = rr.get("apply_to_roles");
            let apply_to_roles: Vec<String> = serde_json::from_str(&roles_json).unwrap_or_default();
            strip_regexes.push(RegexRule {
                id: rr.get("rule_id"),
                title: rr.get("title"),
                find_pattern: rr.get("find_pattern"),
                replace_with: rr.get("replace_with"),
                apply_to_roles,
                apply_to_frontend: rr.get::<i32, _>("apply_to_frontend") != 0,
                apply_to_context: rr.get::<i32, _>("apply_to_context") != 0,
                min_depth: rr.get("min_depth"),
                max_depth: rr.get("max_depth"),
            });
        }

        let topic_rows = sqlx::query(
            "SELECT topic_id, title, created_at, locked, unread, extra_json 
             FROM topics WHERE owner_type = 'agent' AND owner_id = ? AND deleted_at IS NULL ORDER BY updated_at DESC"
        )
        .bind(&agent_id)
        .fetch_all(pool)
        .await
        .map_err(|e| e.to_string())?;

        let mut topics = Vec::new();
        for tr in topic_rows {
            let extra_json: Option<String> = tr.get("extra_json");
            let mut extra_fields: serde_json::Map<String, serde_json::Value> =
                if let Some(ej) = extra_json {
                    serde_json::from_str(&ej).unwrap_or_default()
                } else {
                    serde_json::Map::new()
                };
            extra_fields.insert(
                "locked".to_string(),
                serde_json::Value::Bool(tr.get::<i32, _>("locked") != 0),
            );
            extra_fields.insert(
                "unread".to_string(),
                serde_json::Value::Bool(tr.get::<i32, _>("unread") != 0),
            );

            topics.push(Topic {
                id: tr.get("topic_id"),
                name: tr.get("title"),
                created_at: tr.get("created_at"),
                locked: tr.get::<i32, _>("locked") != 0,
                unread: tr.get::<i32, _>("unread") != 0,
                unread_count: 0,
                msg_count: 0,
                extra_fields,
            });
        }

            let config = AgentConfig {
                id: agent_id.clone(),
                name: row.get("name"),
                system_prompt: row.get("system_prompt"),
                model: row.get("model"),
                temperature: row.get("temperature"),
                context_token_limit: row.get("context_token_limit"),
                max_output_tokens: row.get("max_output_tokens"),
                top_p: extra
                    .remove("top_p")
                    .and_then(|v| v.as_f64())
                    .map(|f| f as f32),
                top_k: extra
                    .remove("top_k")
                    .and_then(|v| v.as_i64())
                    .map(|i| i as i32),
                stream_output: extra
                    .remove("streamOutput")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true),
                tts_voice_primary: extra
                    .remove("ttsVoicePrimary")
                    .and_then(|v| v.as_str().map(|s| s.to_string())),
                tts_regex_primary: extra
                    .remove("ttsRegexPrimary")
                    .and_then(|v| v.as_str().map(|s| s.to_string())),
                tts_voice_secondary: extra
                    .remove("ttsVoiceSecondary")
                    .and_then(|v| v.as_str().map(|s| s.to_string())),
                tts_regex_secondary: extra
                    .remove("ttsRegexSecondary")
                    .and_then(|v| v.as_str().map(|s| s.to_string())),
                tts_speed: extra
                    .remove("ttsSpeed")
                    .and_then(|v| v.as_f64())
                    .map(|f| f as f32)
                    .unwrap_or(1.0),
                avatar_border_color: extra
                    .remove("avatarBorderColor")
                    .and_then(|v| v.as_str().map(|s| s.to_string())),
                avatar_calculated_color,
                name_text_color: extra
                    .remove("nameTextColor")
                    .and_then(|v| v.as_str().map(|s| s.to_string())),
                custom_css: extra
                    .remove("customCss")
                    .and_then(|v| v.as_str().map(|s| s.to_string())),
                card_css: extra
                    .remove("cardCss")
                    .and_then(|v| v.as_str().map(|s| s.to_string())),
                chat_css: extra
                    .remove("chatCss")
                    .and_then(|v| v.as_str().map(|s| s.to_string())),
                disable_custom_colors: extra
                    .remove("disableCustomColors")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                use_theme_colors_in_chat: extra
                    .remove("useThemeColorsInChat")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true),
                ui_collapse_states: extra
                    .remove("uiCollapseStates")
                    .and_then(|v| serde_json::from_value(v).ok()),
                strip_regexes,
                topics,
                extra,
            };

        state.caches.insert(agent_id.clone(), config.clone());
        return Ok(config);
    }

    if allow_default.unwrap_or(false) {
        return Ok(create_default_config(&agent_id));
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

    internal_write_agent_config(&app_handle, &state, &agent_id, &agent).await
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
pub async fn update_agent_config(
    app_handle: AppHandle,
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
    internal_write_agent_config(&app_handle, &state, &agent_id, &new_config).await?;
    
    Ok(new_config)
}

async fn internal_write_agent_config(
    app_handle: &AppHandle,
    state: &AgentConfigState,
    agent_id: &str,
    new_config: &AgentConfig,
) -> Result<bool, String> {
    let db_state = app_handle.state::<DbState>();
    let pool = &db_state.pool;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;

    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;

    // 更新主表
    // 自动同步：将结构体转为 Map，并剔除掉在数据库中有独立列的字段
    let mut extra_map = serde_json::to_value(new_config)
        .ok()
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    
    // 移除在 agents 表中有独立字段的 Key，以及动态聚合字段，防止冗余
    let main_columns = vec![
        "id", "name", "systemPrompt", "model", "temperature", 
        "contextTokenLimit", "maxOutputTokens", "extra", "topics", "stripRegexes",
        "avatarCalculatedColor"
    ];
    for col in main_columns {
        extra_map.remove(col);
    }

    let extra_json = serde_json::to_string(&extra_map).ok();
    sqlx::query(
        "UPDATE agents SET 
            name = ?, 
            system_prompt = ?, 
            model = ?, 
            temperature = ?, 
            context_token_limit = ?, 
            max_output_tokens = ?, 
            extra_json = ?, 
            updated_at = ?
         WHERE agent_id = ?",
    )
    .bind(&new_config.name)
    .bind(&new_config.system_prompt)
    .bind(&new_config.model)
    .bind(new_config.temperature)
    .bind(new_config.context_token_limit)
    .bind(new_config.max_output_tokens)
    .bind(extra_json)
    .bind(now)
    .bind(agent_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;

    // 更新正则规则 (全量覆写)
    sqlx::query("DELETE FROM agent_regex_rules WHERE agent_id = ?")
        .bind(agent_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

    for rule in &new_config.strip_regexes {
        let roles_json = serde_json::to_string(&rule.apply_to_roles).unwrap_or_else(|_| "[]".to_string());
        sqlx::query(
            "INSERT INTO agent_regex_rules (
                rule_id, agent_id, title, find_pattern, replace_with,
                apply_to_roles, apply_to_frontend, apply_to_context,
                min_depth, max_depth, created_at, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&rule.id)
        .bind(agent_id)
        .bind(&rule.title)
        .bind(&rule.find_pattern)
        .bind(&rule.replace_with)
        .bind(roles_json)
        .bind(rule.apply_to_frontend)
        .bind(rule.apply_to_context)
        .bind(rule.min_depth)
        .bind(rule.max_depth)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    }

    // 更新话题 (Upsert)
    for topic in &new_config.topics {
        let topic_extra = serde_json::to_string(&topic.extra_fields).ok();
        sqlx::query(
            "INSERT INTO topics (
                topic_id, owner_type, owner_id, title,
                created_at, updated_at, locked, unread, extra_json
            ) VALUES (?, 'agent', ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(topic_id) DO UPDATE SET
                title = excluded.title,
                locked = excluded.locked,
                unread = excluded.unread,
                extra_json = excluded.extra_json,
                updated_at = excluded.updated_at",
        )
        .bind(&topic.id)
        .bind(agent_id)
        .bind(&topic.name)
        .bind(topic.created_at)
        .bind(now)
        .bind(topic.extra_fields.get("locked").and_then(|v| v.as_bool()).unwrap_or(false))
        .bind(topic.extra_fields.get("unread").and_then(|v| v.as_bool()).unwrap_or(false))
        .bind(topic_extra)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    }

    tx.commit().await.map_err(|e| e.to_string())?;
    state.caches.insert(agent_id.to_string(), new_config.clone());

    Ok(true)
}

/// 删除 Agent
#[tauri::command]
pub async fn delete_agent(
    app_handle: AppHandle,
    state: State<'_, AgentConfigState>,
    agent_id: String,
) -> Result<bool, String> {
    // 1. 删除 DB 记录 (软删除)
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

    // 2. 清理缓存
    state.caches.remove(&agent_id);

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
            model: "gemini-2.0-flash".to_string(),
            temperature: 0.7,
            context_token_limit: 1000000,
            max_output_tokens: 60000,
            top_p: None,
            top_k: None,
            stream_output: true,
            tts_voice_primary: None,
            tts_regex_primary: None,
            tts_voice_secondary: None,
            tts_regex_secondary: None,
            tts_speed: 1.0,
            avatar_border_color: None,
            name_text_color: None,
            custom_css: None,
            card_css: None,
            chat_css: None,
            avatar_calculated_color: None,
            disable_custom_colors: false,
            use_theme_colors_in_chat: true,
            ui_collapse_states: None,
            strip_regexes: vec![],
            topics: vec![Topic {
                id: default_topic_id.clone(),
                name: "主要对话".to_string(),
                created_at: timestamp,
                locked: false,
                unread: false,
                unread_count: 0,
                msg_count: 0,
                extra_fields: serde_json::Map::new(),
            }],
            extra: serde_json::Map::new(),
        }
    };

    log::info!("[AgentService] Creating agent '{}' atomically.", agent_id);

    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
    
    // 1. 插入 agents 表
    let extra_json = serde_json::to_string(&config.extra).ok();
    sqlx::query(
        "INSERT INTO agents (agent_id, name, system_prompt, model, temperature, context_token_limit, max_output_tokens, extra_json, created_at, updated_at) 
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
    )
    .bind(&agent_id)
    .bind(&config.name)
    .bind(&config.system_prompt)
    .bind(&config.model)
    .bind(config.temperature)
    .bind(config.context_token_limit)
    .bind(config.max_output_tokens)
    .bind(extra_json)
    .bind(timestamp)
    .bind(timestamp)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;

    // 2. 插入初始话题
    for topic in &config.topics {
        sqlx::query(
            "INSERT INTO topics (topic_id, owner_type, owner_id, title, created_at, updated_at) 
             VALUES (?, 'agent', ?, ?, ?, ?)"
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
    state.caches.insert(agent_id.clone(), config.clone());

    Ok(config)
}

#[tauri::command]
pub async fn save_avatar_color(
    app_handle: AppHandle,
    owner_type: String,
    owner_id: String,
    color: String,
) -> Result<bool, String> {
    let db_state = app_handle.state::<DbState>();
    let pool = &db_state.pool;

    sqlx::query("UPDATE avatars SET dominant_color = ? WHERE owner_type = ? AND owner_id = ?")
        .bind(color)
        .bind(owner_type)
        .bind(owner_id)
        .execute(pool)
        .await
        .map_err(|e| e.to_string())?;

    Ok(true)
}

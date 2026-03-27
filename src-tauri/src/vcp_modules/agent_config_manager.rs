// AgentConfigManager: 处理智能体(Agent)配置的核心模块
// 源 JS 逻辑参考: ../VCPChat/modules/utils/agentConfigManager.js
// 职责: 管理 Agent 的 config.json 及其备份，实现原子写入、多重恢复机制、缓存管理及并发控制。

use crate::vcp_modules::db_manager::DbState;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager, State};
use tokio::sync::Mutex;
use tokio::time::sleep;

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct TopicInfo {
    /// 话题唯一标识符
    #[serde(default)]
    pub id: String,
    /// 话题名称 (如: "主要对话")
    #[serde(alias = "title", default)]
    pub name: String,
    /// 话题创建时间戳 (ms)
    #[serde(rename = "createdAt", default)]
    pub created_at: i64,
    /// 捕获并保留所有额外的动态字段 (如 locked, unread, creatorSource, _creator 等)
    #[serde(flatten)]
    pub extra_fields: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RegexRule {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(rename = "findPattern", default)]
    pub find_pattern: String,
    #[serde(rename = "replaceWith", default)]
    pub replace_with: String,
    #[serde(rename = "applyToRoles", default)]
    pub apply_to_roles: Vec<String>,
    #[serde(rename = "applyToFrontend", default = "default_true")]
    pub apply_to_frontend: bool,
    #[serde(rename = "applyToContext", default = "default_true")]
    pub apply_to_context: bool,
    #[serde(rename = "minDepth", default)]
    pub min_depth: i32,
    #[serde(rename = "maxDepth", default = "default_neg_one")]
    pub max_depth: i32,
}

impl<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> for RegexRule {
    fn from_row(row: &'r sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        let roles_json: String = row.try_get("apply_to_roles")?;
        let apply_to_roles: Vec<String> =
            serde_json::from_str(&roles_json).map_err(|e| sqlx::Error::ColumnDecode {
                index: "apply_to_roles".to_string(),
                source: Box::new(e),
            })?;

        Ok(RegexRule {
            id: row.try_get("rule_id")?,
            title: row.try_get("title")?,
            find_pattern: row.try_get("find_pattern")?,
            replace_with: row.try_get("replace_with")?,
            apply_to_roles,
            apply_to_frontend: row.try_get("apply_to_frontend")?,
            apply_to_context: row.try_get("apply_to_context")?,
            min_depth: row.try_get("min_depth")?,
            max_depth: row.try_get("max_depth")?,
        })
    }
}

fn default_true() -> bool {
    true
}
fn default_neg_one() -> i32 {
    -1
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct UiCollapseStates {
    #[serde(rename = "paramsCollapsed", default)]
    pub params_collapsed: bool,
    #[serde(rename = "ttsCollapsed", default)]
    pub tts_collapsed: bool,
}

/// 智能体(Agent)的完整配置结构
/// 对齐 VCPChat 桌面端的丰富配置字段
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AgentConfig {
    /// 智能体 ID
    #[serde(default)]
    pub id: String,
    /// 智能体名称
    #[serde(default = "default_agent_name")]
    pub name: String,
    /// 系统提示词 (System Prompt)
    #[serde(rename = "systemPrompt", default)]
    pub system_prompt: String,
    /// 使用的模型 (如: "gemini-2.0-flash")
    #[serde(default = "default_model")]
    pub model: String,
    /// 模型采样温度 (0.0-2.0)
    #[serde(default = "default_temperature")]
    pub temperature: f32,
    /// 上下文 Token 限制
    #[serde(rename = "contextTokenLimit", default = "default_context_limit")]
    pub context_token_limit: i32,
    /// 单次输出最大 Token 数
    #[serde(rename = "maxOutputTokens", default = "default_max_output")]
    pub max_output_tokens: i32,

    // --- 新增对齐 JS 的字段 ---
    #[serde(rename = "top_p", default)]
    pub top_p: Option<f32>,
    #[serde(rename = "top_k", default)]
    pub top_k: Option<i32>,
    #[serde(rename = "streamOutput", default = "default_true")]
    pub stream_output: bool,

    // TTS 设置
    #[serde(rename = "ttsVoicePrimary", default)]
    pub tts_voice_primary: Option<String>,
    #[serde(rename = "ttsRegexPrimary", default)]
    pub tts_regex_primary: Option<String>,
    #[serde(rename = "ttsVoiceSecondary", default)]
    pub tts_voice_secondary: Option<String>,
    #[serde(rename = "ttsRegexSecondary", default)]
    pub tts_regex_secondary: Option<String>,
    #[serde(rename = "ttsSpeed", default = "default_one_f32")]
    pub tts_speed: f32,

    // 样式设置
    #[serde(rename = "avatarBorderColor", default)]
    pub avatar_border_color: Option<String>,
    #[serde(rename = "nameTextColor", default)]
    pub name_text_color: Option<String>,
    #[serde(rename = "customCss", default)]
    pub custom_css: Option<String>,
    #[serde(rename = "cardCss", default)]
    pub card_css: Option<String>,
    #[serde(rename = "chatCss", default)]
    pub chat_css: Option<String>,
    #[serde(rename = "disableCustomColors", default)]
    pub disable_custom_colors: bool,
    #[serde(rename = "useThemeColorsInChat", default)]
    pub use_theme_colors_in_chat: bool,

    #[serde(rename = "uiCollapseStates", default)]
    pub ui_collapse_states: Option<UiCollapseStates>,

    #[serde(rename = "stripRegexes", default)]
    pub strip_regexes: Vec<RegexRule>,

    #[serde(rename = "avatarUrl", default)]
    pub avatar_url: Option<String>,
    #[serde(rename = "avatarCalculatedColor", default)]
    pub avatar_calculated_color: Option<String>,

    /// 话题列表
    #[serde(default)]
    pub topics: Vec<TopicInfo>,

    /// 捕获所有未定义的字段，确保 config.json 的完整性（真实之源）
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

fn default_agent_name() -> String {
    "Unnamed Agent".to_string()
}
fn default_model() -> String {
    "gemini-2.0-flash".to_string()
}

fn default_one_f32() -> f32 {
    1.0
}
fn default_temperature() -> f32 {
    1.0
}
fn default_context_limit() -> i32 {
    1000000
}
fn default_max_output() -> i32 {
    64000
}

/// Agent 相关的路径管理集合
pub struct AgentPaths {
    pub agent_path: PathBuf,
    pub config_path: PathBuf,
    #[allow(dead_code)]
    pub lock_file: PathBuf,
    pub backup_file: PathBuf,
    pub temp_file: PathBuf,
}

/// AgentConfigManager 的全局状态，由 Tauri 管理
pub struct AgentConfigState {
    /// 配置缓存: agent_id -> AgentConfig
    pub caches: DashMap<String, AgentConfig>,
    /// 缓存时间戳: agent_id -> mtime_ms
    pub cache_timestamps: DashMap<String, u64>,
    /// 任务队列锁: agent_id -> Mutex
    /// 确保针对同一 Agent 的更新操作是串行的
    pub locks: DashMap<String, Arc<Mutex<()>>>,
}

impl AgentConfigState {
    pub fn new() -> Self {
        Self {
            caches: DashMap::new(),
            cache_timestamps: DashMap::new(),
            locks: DashMap::new(),
        }
    }

    /// 获取针对特定 Agent 的锁
    pub async fn acquire_lock(&self, agent_id: &str) -> Arc<Mutex<()>> {
        self.locks
            .entry(agent_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .value()
            .clone()
    }
}

/// 获取指定智能体相关的所有文件路径
fn get_agent_paths(app_handle: &AppHandle, agent_id: &str) -> Result<AgentPaths, String> {
    let mut agent_path = app_handle
        .path()
        .app_config_dir()
        .map_err(|e| e.to_string())?;
    agent_path.push("Agents");
    agent_path.push(agent_id);

    let config_path = agent_path.join("config.json");
    let lock_file = agent_path.join("config.json.lock");
    let backup_file = agent_path.join("config.json.backup");
    let temp_file = agent_path.join("config.json.tmp");

    log::debug!(
        "[AgentConfigManager] Resolved agent '{}' config path to {:?}; topic history must still be aggregated from UserData/data",
        agent_id,
        config_path
    );

    Ok(AgentPaths {
        agent_path,
        config_path,
        lock_file,
        backup_file,
        temp_file,
    })
}

/// 指数退避重试读取文件
/// 对齐 JS: retryDelays = [50, 100, 200]
async fn retry_read_to_string(path: &Path) -> Result<String, String> {
    let delays = [50, 100, 200];

    for &delay in delays.iter() {
        match fs::read_to_string(path) {
            Ok(content) => return Ok(content),
            Err(e) => {
                if e.kind() == std::io::ErrorKind::NotFound {
                    // 如果文件不存在，可能是正在原子替换中，等待后重试
                    sleep(Duration::from_millis(delay)).await;
                    continue;
                }
                return Err(e.to_string());
            }
        }
    }

    // 最后尝试一次
    fs::read_to_string(path).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn read_agent_config(
    app_handle: AppHandle,
    state: State<'_, AgentConfigState>,
    agent_id: String,
    allow_default: Option<bool>,
) -> Result<AgentConfig, String> {
    let paths = get_agent_paths(&app_handle, &agent_id)?;
    let allow_default = allow_default.unwrap_or(false);

    // 1. 尝试从缓存读取
    if let Ok(metadata) = fs::metadata(&paths.config_path) {
        let mtime = metadata
            .modified()
            .map(|t| t.duration_since(UNIX_EPOCH).unwrap().as_millis() as u64)
            .unwrap_or(0);

        if let Some(cached_config) = state.caches.get(&agent_id) {
            if let Some(cached_mtime) = state.cache_timestamps.get(&agent_id) {
                if mtime <= *cached_mtime {
                    return Ok(cached_config.clone());
                }
            }
        }
    }

    // 2. 文件不存在处理
    if !paths.config_path.exists() {
        if allow_default {
            return Ok(AgentConfig {
                id: agent_id.clone(),
                name: agent_id.clone(),
                system_prompt: format!("你是 {}。", agent_id),
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
                disable_custom_colors: true,
                use_theme_colors_in_chat: true,
                ui_collapse_states: None,
                strip_regexes: vec![],
                avatar_url: None,
                avatar_calculated_color: None,
                topics: vec![],
                extra: serde_json::Map::new(),
            });
        }

        // 尝试从缓存恢复（最后手段）
        if let Some(cached) = state.caches.get(&agent_id) {
            return Ok(cached.clone());
        }

        return Err(format!("Agent config for {} not found", agent_id));
    }

    // 3. 读取并解析 (带重试逻辑)
    let content = retry_read_to_string(&paths.config_path).await?;
    let config_res: Result<AgentConfig, serde_json::Error> = serde_json::from_str(&content);

    let mut config = match config_res {
        Ok(c) => c,
        Err(e) => {
            // [恢复逻辑] 尝试从备份恢复
            let mut backup_recovered = None;
            if paths.backup_file.exists() {
                if let Ok(backup_content) = fs::read_to_string(&paths.backup_file) {
                    if let Ok(backup_config) = serde_json::from_str::<AgentConfig>(&backup_content)
                    {
                        backup_recovered = Some(backup_config);
                    }
                }
            }

            match backup_recovered {
                Some(bc) => bc,
                None => return Err(e.to_string()),
            }
        }
    };

    // [重大对齐] 读取外部 regex_rules.json (如果存在)
    let regex_path = paths.agent_path.join("regex_rules.json");
    if regex_path.exists() {
        if let Ok(regex_content) = fs::read_to_string(&regex_path) {
            if let Ok(regexes) = serde_json::from_str::<Vec<RegexRule>>(&regex_content) {
                config.strip_regexes = regexes;
            }
        }
    }

    if config.id.is_empty() {
        config.id = agent_id.clone();
    }

    // [重大对齐] 为被恢复工具恢复的不完整话题补充默认字段
    for topic in &mut config.topics {
        if !topic.extra_fields.contains_key("locked") {
            topic
                .extra_fields
                .insert("locked".to_string(), serde_json::Value::Bool(true));
        }
        if !topic.extra_fields.contains_key("unread") {
            topic
                .extra_fields
                .insert("unread".to_string(), serde_json::Value::Bool(false));
        }
        if !topic.extra_fields.contains_key("creatorSource") {
            topic.extra_fields.insert(
                "creatorSource".to_string(),
                serde_json::Value::String("ui".to_string()),
            );
        }
    }

    // 动态替换桌面端的绝对路径为手机端的绝对路径
    let config_dir = app_handle
        .path()
        .app_config_dir()
        .map_err(|e| e.to_string())?;
    let config_dir_str = config_dir.to_string_lossy().replace("\\", "/");

    if let Some(avatar_url) = &mut config.avatar_url {
        if avatar_url.contains("AppData/Agents") || avatar_url.contains("AppData\\Agents") {
            let parts: Vec<&str> = avatar_url.split(&['/', '\\'][..]).collect();
            if let Some(agent_idx) = parts.iter().position(|&r| r == "Agents") {
                let relative_path = parts[agent_idx + 1..].join("/");
                *avatar_url = format!("{}/Agents/{}", config_dir_str, relative_path);
            }
        }
    } else {
        // [回退机制] 如果 config.json 中没有 avatarUrl，自动探测目录下的头像文件
        let extensions = ["png", "jpg", "jpeg", "webp", "gif"];
        for ext in extensions {
            let avatar_path = paths.agent_path.join(format!("avatar.{}", ext));
            if avatar_path.exists() {
                config.avatar_url = Some(avatar_path.to_string_lossy().replace("\\", "/"));
                break;
            }
        }
    }

    // 4. 更新缓存
    let mtime = fs::metadata(&paths.config_path)
        .and_then(|m| m.modified())
        .map(|t| t.duration_since(UNIX_EPOCH).unwrap().as_millis() as u64)
        .unwrap_or_else(|_| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64
        });

    state.caches.insert(agent_id.clone(), config.clone());
    state.cache_timestamps.insert(agent_id, mtime);

    Ok(config)
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

    // 获取锁，确保串行写入
    let mutex = state.acquire_lock(&agent_id).await;
    let _lock = mutex.lock().await;

    internal_write_agent_config(&app_handle, &state, &agent_id, &agent).await
}

#[tauri::command]
pub async fn write_agent_config(
    app_handle: AppHandle,
    state: State<'_, AgentConfigState>,
    agent_id: String,
    config: AgentConfig,
) -> Result<bool, String> {
    // 获取锁，确保串行写入
    let mutex = state.acquire_lock(&agent_id).await;
    let _lock = mutex.lock().await;

    internal_write_agent_config(&app_handle, &state, &agent_id, &config).await
}

#[tauri::command]
pub async fn get_agents(
    app_handle: AppHandle,
    state: State<'_, AgentConfigState>,
) -> Result<Vec<AgentConfig>, String> {
    let mut agents_dir = app_handle
        .path()
        .app_config_dir()
        .map_err(|e| e.to_string())?;
    agents_dir.push("Agents");

    if !agents_dir.exists() {
        return Ok(vec![]);
    }

    let mut agents = Vec::new();
    for entry in fs::read_dir(agents_dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let agent_id = entry.file_name().to_string_lossy().to_string();
        let config_path = path.join("config.json");
        if !config_path.exists() {
            log::warn!(
                "[AgentConfigManager] Skipping agent directory '{}' because config.json is missing at {:?}",
                agent_id,
                config_path
            );
            continue;
        }

        match read_agent_config(app_handle.clone(), state.clone(), agent_id.clone(), Some(false)).await {
            Ok(config) => agents.push(config),
            Err(err) => {
                log::error!(
                    "[AgentConfigManager] Failed to read agent '{}' from {:?}: {}",
                    agent_id,
                    config_path,
                    err
                );
            }
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
    // 获取锁，确保整个“读取-修改-写入”过程是原子的
    let mutex = state.acquire_lock(&agent_id).await;
    let _lock = mutex.lock().await;

    // 1. 读取当前配置
    let config = read_agent_config(
        app_handle.clone(),
        state.clone(),
        agent_id.clone(),
        Some(false),
    )
    .await?;

    // 2. 合并更新字段
    // 使用 serde_json 的合并逻辑
    let mut config_val = serde_json::to_value(&config).map_err(|e| e.to_string())?;

    if let Some(updates_obj) = updates.as_object() {
        if let Some(config_obj) = config_val.as_object_mut() {
            for (k, v) in updates_obj {
                config_obj.insert(k.clone(), v.clone());
            }
        }
    }

    let new_config: AgentConfig = serde_json::from_value(config_val).map_err(|e| e.to_string())?;

    // 3. 持久化 (内部调用 write_agent_config 的逻辑，由于已经持有锁，这里直接写文件)
    // 注意：write_agent_config 命令也需要获取锁，但 Rust 的 Mutex 不支持重入。
    // 所以这里我们要提取出不带锁的内部写入逻辑。

    internal_write_agent_config(&app_handle, &state, &agent_id, &new_config).await?;

    Ok(new_config)
}

/// 内部使用的写入逻辑，不包含锁（调用者必须持有锁）
async fn internal_write_agent_config(
    app_handle: &AppHandle,
    state: &AgentConfigState,
    agent_id: &str,
    config: &AgentConfig,
) -> Result<bool, String> {
    let config = config.clone();

    // 移除了旧版的正则解毒 (Regex Detox) Hack，直接依赖 serde_json 的标准转义机制。
    // serde_json::to_string_pretty 会自动处理好必要的转义。

    let paths = get_agent_paths(app_handle, agent_id)?;
    fs::create_dir_all(&paths.agent_path).map_err(|e| e.to_string())?;

    // 2. 物理层保持分离: 独立保存 regex_rules.json 以兼容桌面端
    if !config.strip_regexes.is_empty() {
        let regex_path = paths.agent_path.join("regex_rules.json");
        let regex_content =
            serde_json::to_string_pretty(&config.strip_regexes).map_err(|e| e.to_string())?;
        fs::write(regex_path, regex_content).map_err(|e| e.to_string())?;
    } else {
        let regex_path = paths.agent_path.join("regex_rules.json");
        if regex_path.exists() {
            let _ = fs::remove_file(regex_path);
        }
    }

    // 3. 写入 config.json (移除 stripRegexes 字段)
    let mut config_val = serde_json::to_value(&config).map_err(|e| e.to_string())?;
    if let Some(obj) = config_val.as_object_mut() {
        obj.remove("stripRegexes");
    }

    let content = serde_json::to_string_pretty(&config_val).map_err(|e| e.to_string())?;
    fs::write(&paths.temp_file, &content).map_err(|e| e.to_string())?;
    let _: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| format!("Temp file validation failed: {}", e))?;

    if paths.config_path.exists() {
        fs::copy(&paths.config_path, &paths.backup_file).map_err(|e| e.to_string())?;
    }
    fs::rename(&paths.temp_file, &paths.config_path).map_err(|e| e.to_string())?;

    let mtime = fs::metadata(&paths.config_path)
        .and_then(|m| m.modified())
        .map(|t| t.duration_since(UNIX_EPOCH).unwrap().as_millis() as u64)
        .unwrap_or_else(|_| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64
        });

    // 4. 更新内存缓存 (逻辑层保持合并)
    state.caches.insert(agent_id.to_string(), config.clone());
    state.cache_timestamps.insert(agent_id.to_string(), mtime);

    // 5. 影子数据库同步 (Shadow DB Sync)
    let db_state = app_handle.state::<DbState>();
    let pool = &db_state.pool;

    // 使用事务保证物理文件与数据库索引的最终一致性
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;

    // 5a. 更新 agent_index
    sqlx::query(
        "INSERT INTO agent_index (agent_id, name, mtime) VALUES (?, ?, ?)
         ON CONFLICT(agent_id) DO UPDATE SET name=excluded.name, mtime=excluded.mtime",
    )
    .bind(agent_id)
    .bind(&config.name)
    .bind(mtime as i64)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;

    // 5b. 同步正则规则到数据库 (Shadow DB for high performance lookup)
    sqlx::query("DELETE FROM agent_regex_rules WHERE agent_id = ?")
        .bind(agent_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

    for rule in &config.strip_regexes {
        let roles_json =
            serde_json::to_string(&rule.apply_to_roles).unwrap_or_else(|_| "[]".to_string());
        sqlx::query(
            "INSERT INTO agent_regex_rules (
                rule_id, agent_id, title, find_pattern, replace_with, 
                apply_to_roles, apply_to_frontend, apply_to_context, min_depth, max_depth
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    }

    // 5c. 同步话题列表到数据库 (Shadow DB for high performance lookup)
    // 注意：这里只做插入或更新基本信息，不覆盖 msg_count 等动态数据
    for topic in &config.topics {
        sqlx::query(
            "INSERT INTO topic_index (topic_id, agent_id, title, mtime, locked, unread, unread_count)
             VALUES (?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(topic_id) DO UPDATE SET
                title=excluded.title,
                mtime=excluded.mtime"
        )
        .bind(&topic.id)
        .bind(agent_id)
        .bind(&topic.name)
        .bind(topic.created_at)
        .bind(false)
        .bind(false)
        .bind(0)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    }

    tx.commit().await.map_err(|e| e.to_string())?;

    Ok(true)
}

/// 重建整个影子数据库索引 (通常在全量同步后调用)
#[tauri::command]
pub async fn rebuild_db_index(
    app_handle: AppHandle,
    state: State<'_, AgentConfigState>,
) -> Result<usize, String> {
    let mut agents_dir = app_handle
        .path()
        .app_config_dir()
        .map_err(|e| e.to_string())?;
    agents_dir.push("Agents");

    if !agents_dir.exists() {
        return Ok(0);
    }

    let mut count = 0;
    for entry in fs::read_dir(agents_dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.is_dir() {
            let agent_id = entry.file_name().to_string_lossy().to_string();
            // 读取配置 (这会触发路径替换等逻辑)
            if let Ok(config) = read_agent_config(
                app_handle.clone(),
                state.clone(),
                agent_id.clone(),
                Some(false),
            )
            .await
            {
                // 强制写入一次，触发 internal_write_agent_config 里的数据库同步逻辑
                let mutex = state.acquire_lock(&agent_id).await;
                let _lock = mutex.lock().await;
                if internal_write_agent_config(&app_handle, &state, &agent_id, &config)
                    .await
                    .is_ok()
                {
                    count += 1;
                }
            }
        }
    }

    Ok(count)
}

// GroupManager: 处理群组(Agent Group)配置与生命周期的核心模块
// 源 JS 逻辑参考: ../VCPChat/modules/groupchat.js
// 职责: 管理群组的 config.json，实现双轨目录结构支持，并为前端提供高性能缓存。

use dashmap::DashMap;
use log::{error, info};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{AppHandle, Manager, Runtime};
use tokio::sync::Mutex;

use crate::vcp_modules::topic_list_manager::Topic;

/// 群组成员简要结构 (用于强类型化)
#[derive(Debug, Serialize, Deserialize, Clone)]
#[allow(dead_code)]
pub struct GroupMember {
    pub id: String,
    pub tag: Option<String>,
}

/// 群组完整配置结构 (对齐桌面端 config.json)
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GroupConfig {
    /// 群组 ID (通常是 ____123 格式)
    pub id: String,
    /// 群组名称
    #[serde(default)]
    pub name: String,
    /// 头像路径 (相对或绝对)
    #[serde(default)]
    pub avatar: Option<String>,
    /// 自动提取的头像主色调
    #[serde(default)]
    pub avatar_calculated_color: Option<String>,
    /// 成员 Agent ID 列表
    #[serde(default)]
    pub members: Vec<String>,
    /// 发言模式 (sequential, naturerandom, invite_only)
    #[serde(default)]
    pub mode: String,
    /// 成员标签 (映射 agentId -> tags)
    #[serde(default)]
    pub member_tags: Option<serde_json::Value>,
    /// 群组全局提示词
    #[serde(default)]
    pub group_prompt: Option<String>,
    /// 邀请发言提示词
    #[serde(default)]
    pub invite_prompt: Option<String>,
    /// 是否使用统一模型
    #[serde(default)]
    pub use_unified_model: bool,
    /// 统一模型名称
    #[serde(default)]
    pub unified_model: Option<String>,
    /// 创建时间戳
    #[serde(default)]
    pub created_at: i64,
    /// 话题列表
    #[serde(default)]
    pub topics: Vec<Topic>,
    /// 标签匹配模式 (strict, fuzzy)
    #[serde(default)]
    pub tag_match_mode: Option<String>,
    /// 捕获所有未定义的字段
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// GroupManager 的全局状态
pub struct GroupManagerState {
    /// 配置缓存: group_id -> GroupConfig
    pub caches: DashMap<String, GroupConfig>,
    /// 任务队列锁: group_id -> Mutex
    #[allow(dead_code)]
    pub locks: DashMap<String, Arc<Mutex<()>>>,
}

impl GroupManagerState {
    pub fn new() -> Self {
        Self {
            caches: DashMap::new(),
            locks: DashMap::new(),
        }
    }

    /// 获取群组配置 (优先从缓存读取)
    pub fn get_group(&self, group_id: &str) -> Option<GroupConfig> {
        self.caches.get(group_id).map(|c| c.clone())
    }
}

// --- 路径辅助逻辑 ---

/// 获取 AppData/AgentGroups 目录
pub fn get_groups_base_path<R: Runtime>(app: &AppHandle<R>) -> PathBuf {
    let mut path = app
        .path()
        .app_config_dir()
        .unwrap_or_else(|_| PathBuf::from("AppData"));
    path.push("AgentGroups");
    path
}

/// 获取 UserData/{groupId} 目录
#[allow(dead_code)]
pub fn get_group_user_data_path<R: Runtime>(app: &AppHandle<R>, group_id: &str) -> PathBuf {
    let mut path = app
        .path()
        .app_config_dir()
        .unwrap_or_else(|_| PathBuf::from("AppData"));
    path.push("UserData");
    path.push(group_id);
    path
}

// --- 业务逻辑 ---

/// 路径转换辅助: 针对群组头像
fn resolve_group_avatar_path<R: Runtime>(app: &AppHandle<R>, config: &mut GroupConfig) {
    if let Some(avatar) = &mut config.avatar {
        // 如果是相对路径 (如 "avatar.png")，拼接上群组目录
        if !avatar.contains('/') && !avatar.contains('\\') {
            let mut path = get_groups_base_path(app);
            path.push(&config.id);
            path.push(&avatar);
            *avatar = path.to_string_lossy().replace("\\", "/");
        }
        // 如果是桌面端的绝对路径，进行转换
        else if avatar.contains("AppData/AgentGroups") || avatar.contains("AppData\\AgentGroups")
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
        // 自动探测
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

/// 加载所有群组配置到缓存，并同步话题索引到数据库
pub async fn load_all_groups<R: Runtime>(
    app: &AppHandle<R>,
    state: &GroupManagerState,
    db: &sqlx::Pool<sqlx::Sqlite>,
) -> Result<(), String> {
    let base_path = get_groups_base_path(app);
    if !base_path.exists() {
        fs::create_dir_all(&base_path).map_err(|e| e.to_string())?;
        return Ok(());
    }

    let entries = fs::read_dir(base_path).map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let config_path = path.join("config.json");
            if config_path.exists() {
                if let Ok(content) = fs::read_to_string(&config_path) {
                    if let Ok(mut config) = serde_json::from_str::<GroupConfig>(&content) {
                        // 路径转换
                        resolve_group_avatar_path(app, &mut config);

                        // 同步话题到数据库
                        for topic in &config.topics {
                            let exists: bool =
                                sqlx::query("SELECT 1 FROM topic_index WHERE topic_id = ?")
                                    .bind(&topic.id)
                                    .fetch_optional(db)
                                    .await
                                    .map_err(|e| e.to_string())?
                                    .is_some();

                            if !exists {
                                sqlx::query(
                                        "INSERT INTO topic_index (topic_id, agent_id, title, mtime, locked, unread, unread_count, last_msg_preview, msg_count) 
                                         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)"
                                    )
                                    .bind(&topic.id)
                                    .bind(&config.id)
                                    .bind(&topic.name)
                                    .bind(topic.created_at)
                                    .bind(topic.locked)
                                    .bind(topic.unread)
                                    .bind(topic.unread_count)
                                    .bind(&topic.last_msg_preview)
                                    .bind(topic.msg_count)
                                    .execute(db)
                                    .await
                                    .map_err(|e| e.to_string())?;
                            }
                        }

                        state.caches.insert(config.id.clone(), config);
                    }
                }
            }
        }
    }

    println!(
        "[GroupManager] Loaded {} groups and synced topics to DB.",
        state.caches.len()
    );
    Ok(())
}

/// 获取话题目录路径 (支持双轨结构与 UserData/data 兼容)
pub fn resolve_topic_dir<R: Runtime>(app: &AppHandle<R>, item_id: &str, topic_id: &str) -> PathBuf {
    let config_dir = app
        .path()
        .app_config_dir()
        .unwrap_or_else(|_| PathBuf::from("AppData"));

    // 兼容性探测：优先 UserData (桌面端标准)，次选 data (移动端同步标准)
    let mut path = config_dir.join("UserData");
    if !path.exists() {
        let alt_path = config_dir.join("data");
        if alt_path.exists() {
            path = alt_path;
        }
    }

    path.push(item_id);
    path.push("topics");

    // 如果是群组，且话题ID不带 group_ 前缀，则增加 group_ 前缀
    if (item_id.starts_with("____") || item_id.starts_with("___N_P_"))
        && !topic_id.starts_with("group_")
    {
        path.push(format!("group_{}", topic_id));
    } else {
        path.push(topic_id);
    }
    path
}
/// 修改后的历史记录路径获取逻辑 (支持双轨结构)
pub fn resolve_history_path<R: Runtime>(
    app: &AppHandle<R>,
    item_id: &str,
    topic_id: &str,
) -> PathBuf {
    let mut path = resolve_topic_dir(app, item_id, topic_id);
    path.push("history.json");
    info!(
        "[GroupManager] Resolved history path for {}/{}: {:?}",
        item_id, topic_id, path
    );
    path
}

// --- Tauri Commands ---

#[tauri::command]
pub async fn get_groups(
    state: tauri::State<'_, GroupManagerState>,
) -> Result<Vec<GroupConfig>, String> {
    Ok(state
        .caches
        .iter()
        .map(|entry| entry.value().clone())
        .collect())
}

#[tauri::command]
pub async fn read_group_config(
    app: AppHandle,
    state: tauri::State<'_, GroupManagerState>,
    group_id: String,
) -> Result<GroupConfig, String> {
    if let Some(mut config) = state.get_group(&group_id) {
        resolve_group_avatar_path(&app, &mut config);
        return Ok(config);
    }

    // 缓存未命中，尝试磁盘读取
    let config_path = get_groups_base_path(&app)
        .join(&group_id)
        .join("config.json");
    if !config_path.exists() {
        error!(
            "[GroupManager] Group config NOT FOUND at: {:?}",
            config_path
        );
        return Err(format!("Group config not found: {}", group_id));
    }

    let content = fs::read_to_string(&config_path).map_err(|e| {
        error!(
            "[GroupManager] Failed to read config at {:?}: {}",
            config_path, e
        );
        e.to_string()
    })?;

    let mut config: GroupConfig = serde_json::from_str(&content).map_err(|e| {
        error!(
            "[GroupManager] Failed to parse GroupConfig for {}: {}. Content sample: {}",
            group_id,
            e,
            &content[..content.len().min(100)]
        );
        e.to_string()
    })?;

    resolve_group_avatar_path(&app, &mut config);
    state.caches.insert(config.id.clone(), config.clone());
    Ok(config)
}

#[tauri::command]
pub async fn create_group(
    app_handle: AppHandle,
    state: tauri::State<'_, GroupManagerState>,
    name: String,
) -> Result<GroupConfig, String> {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    
    // ID 生成逻辑对齐桌面端: 名称过滤 + 时间戳
    let base_id = name
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
        .collect::<String>();
    let group_id = format!("_____{}_{}", base_id, timestamp); // 手机端强制 ____ 前缀标识群组

    let base_path = get_groups_base_path(&app_handle).join(&group_id);
    fs::create_dir_all(&base_path).map_err(|e| e.to_string())?;

    let default_topic_id = format!("group_topic_{}", timestamp);
    let default_topic = Topic {
        id: default_topic_id.clone(),
        name: "主要群聊".to_string(),
        created_at: (timestamp / 1000) as i64,
        locked: false,
        unread: false,
        unread_count: 0,
        last_msg_preview: None,
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
        invite_prompt: Some("现在轮到你{{VCPChatAgentName}}发言了。系统已经为大家添加[xxx的发言：]这样的标记头，以用于区分不同发言来自谁。大家不用自己再输出自己的发言标记头，也不需要讨论发言标记系统，正常聊天即可。".to_string()),
        use_unified_model: false,
        unified_model: None,
        created_at: (timestamp / 1000) as i64,
        topics: vec![default_topic.clone()],
        tag_match_mode: Some("strict".to_string()),
        extra: serde_json::Map::new(),
    };

    // 写入 config.json
    let config_path = base_path.join("config.json");
    let content = serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?;
    fs::write(config_path, content).map_err(|e| e.to_string())?;

    // 初始化话题目录及 history.json
    let topic_dir = resolve_topic_dir(&app_handle, &group_id, &default_topic_id);
    fs::create_dir_all(&topic_dir).map_err(|e| e.to_string())?;
    fs::write(topic_dir.join("history.json"), "[]").map_err(|e| e.to_string())?;

    // 更新缓存
    state.caches.insert(group_id, config.clone());

    Ok(config)
}


use log::error;
use serde::{Deserialize, Serialize};
use std::fs;
use tauri::{AppHandle, Manager, Runtime};

pub use crate::vcp_modules::path_topology_service::get_groups_base_path;
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

/// 路径转换辅助: 针对群组头像
pub fn resolve_group_avatar_path<R: Runtime>(app: &AppHandle<R>, config: &mut GroupConfig) {
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

/// 从磁盘读取群组配置
pub fn read_group_config<R: Runtime>(
    app: &AppHandle<R>,
    group_id: &str,
) -> Result<GroupConfig, String> {
    let config_path = get_groups_base_path(app).join(group_id).join("config.json");

    if !config_path.exists() {
        return Err(format!("Group config not found: {}", group_id));
    }

    let content = fs::read_to_string(&config_path).map_err(|e| {
        error!(
            "[GroupConfigRepo] Failed to read config at {:?}: {}",
            config_path, e
        );
        e.to_string()
    })?;

    let mut config: GroupConfig = serde_json::from_str(&content).map_err(|e| {
        error!(
            "[GroupConfigRepo] Failed to parse GroupConfig for {}: {}",
            group_id, e
        );
        e.to_string()
    })?;

    resolve_group_avatar_path(app, &mut config);
    Ok(config)
}

/// 将群组配置写入磁盘
pub fn write_group_config<R: Runtime>(
    app: &AppHandle<R>,
    config: &GroupConfig,
) -> Result<(), String> {
    let base_path = get_groups_base_path(app).join(&config.id);
    if !base_path.exists() {
        fs::create_dir_all(&base_path).map_err(|e| e.to_string())?;
    }

    let config_path = base_path.join("config.json");
    let content = serde_json::to_string_pretty(config).map_err(|e| e.to_string())?;
    fs::write(config_path, content).map_err(|e| e.to_string())?;

    Ok(())
}

/// 创建群组目录结构及初始化文件
pub fn create_group_directory_structure<R: Runtime>(
    app: &AppHandle<R>,
    group_id: &str,
    default_topic_id: &str,
) -> Result<(), String> {
    let base_path = get_groups_base_path(app).join(group_id);
    fs::create_dir_all(&base_path).map_err(|e| e.to_string())?;

    // 初始化话题目录及 history.json
    let topic_dir = base_path.join("topics").join(default_topic_id);
    fs::create_dir_all(&topic_dir).map_err(|e| e.to_string())?;
    fs::write(topic_dir.join("history.json"), "[]").map_err(|e| e.to_string())?;

    Ok(())
}

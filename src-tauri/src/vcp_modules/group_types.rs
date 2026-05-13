use crate::vcp_modules::topic_types::Topic;
use serde::{Deserialize, Serialize};

fn default_group_name() -> String {
    "Unnamed Group".to_string()
}

fn default_group_mode() -> String {
    "sequential".to_string()
}

fn default_tag_match_mode() -> Option<String> {
    Some("strict".to_string())
}

/// 群组完整配置结构 (对齐桌面端 config.json)
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GroupConfig {
    /// 群组 ID (通常是 ____123 格式)
    pub id: String,
    /// 群组名称
    #[serde(default = "default_group_name")]
    pub name: String,
    /// 自动提取的头像主色调 (从 avatars 表动态获取)
    #[serde(default)]
    pub avatar_calculated_color: Option<String>,
    /// 成员 Agent ID 列表
    #[serde(default)]
    pub members: Vec<String>,
    /// 发言模式 (sequential, naturerandom, invite_only)
    #[serde(default = "default_group_mode")]
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
    /// 话题列表
    #[serde(default)]
    pub topics: Vec<Topic>,
    /// 标签匹配模式 (strict, natural)
    #[serde(default = "default_tag_match_mode")]
    pub tag_match_mode: Option<String>,
    /// 创建时间戳
    #[serde(default)]
    pub created_at: i64,
    /// 当前活跃话题 ID
    #[serde(default)]
    pub current_topic_id: Option<String>,
}

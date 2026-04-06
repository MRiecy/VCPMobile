use crate::vcp_modules::topic_list_manager::Topic;
use serde::{Deserialize, Serialize};

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

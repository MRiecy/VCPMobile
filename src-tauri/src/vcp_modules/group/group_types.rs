use crate::vcp_modules::topic_types::Topic;
use serde::de::Error as _;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub type MemberTags = BTreeMap<String, String>;

pub(crate) fn validate_member_tags(member_tags: &MemberTags) -> Result<(), String> {
    if member_tags.keys().any(String::is_empty) {
        return Err("memberTags keys must be non-empty strings".to_string());
    }
    Ok(())
}

pub(crate) fn parse_member_tags(raw: &str) -> Result<MemberTags, String> {
    let member_tags = serde_json::from_str(raw).map_err(|error| error.to_string())?;
    validate_member_tags(&member_tags)?;
    Ok(member_tags)
}

pub(crate) fn serialize_member_tags(member_tags: Option<&MemberTags>) -> Result<String, String> {
    match member_tags {
        Some(member_tags) => serde_json::to_string(member_tags).map_err(|error| error.to_string()),
        None => Ok("{}".to_string()),
    }
}

pub(crate) fn deserialize_member_tags<'de, D>(
    deserializer: D,
) -> Result<Option<MemberTags>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let member_tags = Option::<MemberTags>::deserialize(deserializer)?;
    if let Some(member_tags) = &member_tags {
        validate_member_tags(member_tags).map_err(D::Error::custom)?;
    }
    Ok(member_tags)
}

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
    /// 完整成员标签映射；已移除成员的 Tag 仍保留供重新加入时恢复。
    #[serde(default, deserialize_with = "deserialize_member_tags")]
    pub member_tags: Option<MemberTags>,
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
}

/// 群组的轻量列表结构
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GroupListItem {
    pub id: String,
    pub name: String,
    pub avatar_calculated_color: Option<String>,
    pub members: Vec<String>,
    /// 发言模式 (sequential, naturerandom, invite_only)。
    /// 前端邀约横条/@选择器的模式判定以列表快照为准，缺此字段会导致
    /// invite_only 群在重启后永远识别不到（血训：曾被快照 DTO 丢弃）。
    #[serde(default = "default_group_mode")]
    pub mode: String,
}

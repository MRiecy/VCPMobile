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

/// 本机群行为配置。它不属于 GroupSyncDTO，也不改变共享群的版本。
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct GroupLocalSettings {
    pub mode_override: Option<String>,
    pub mode_settings: GroupModeSettings,
    pub enable_context_message_window: bool,
    pub context_message_window_size: u32,
}

impl Default for GroupLocalSettings {
    fn default() -> Self {
        Self {
            mode_override: None,
            mode_settings: GroupModeSettings::default(),
            enable_context_message_window: false,
            context_message_window_size: 100,
        }
    }
}

impl GroupLocalSettings {
    pub fn effective_mode<'a>(&'a self, shared_mode: &'a str) -> &'a str {
        self.mode_override.as_deref().unwrap_or(shared_mode)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.mode_override.as_deref().is_some_and(|mode| {
            !matches!(mode, "sequential" | "naturerandom" | "invite_only" | "jev")
        }) {
            return Err("本机群聊模式不受支持".into());
        }
        if !(1..=10_000).contains(&self.context_message_window_size) {
            return Err("群成员历史窗口必须为 1–10000 条".into());
        }
        if self
            .mode_settings
            .sequential
            .speaker_order
            .iter()
            .any(|id| id.trim().is_empty())
        {
            return Err("发言顺序不能包含空成员 ID".into());
        }
        self.mode_settings.jev.validate()
    }
}

/// 备份恢复保留字段是否出现；缺项不能在解析时提前填成默认值。
#[derive(Debug, Serialize, Clone, PartialEq)]
#[serde(transparent)]
pub struct GroupLocalSettingsPatch(serde_json::Map<String, serde_json::Value>);

impl GroupLocalSettingsPatch {
    pub fn parse(value: &serde_json::Value) -> Result<Self, String> {
        let defaults = GroupLocalSettings::default();
        let shape = serde_json::to_value(&defaults).map_err(|error| error.to_string())?;
        let normalized = normalize_local_settings_patch(value, &shape)?;
        let patch = Self(match normalized {
            Some(serde_json::Value::Object(object)) => object,
            None => serde_json::Map::new(),
            _ => return Err("备份群本机设置必须为对象".into()),
        });
        // 校验明确提供的值，但只保留原补丁，不把默认值写进恢复请求。
        patch.apply(&defaults)?;
        Ok(patch)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn apply(&self, current: &GroupLocalSettings) -> Result<GroupLocalSettings, String> {
        let shape = serde_json::to_value(GroupLocalSettings::default())
            .map_err(|error| error.to_string())?;
        let mut merged = serde_json::to_value(current).map_err(|error| error.to_string())?;
        merge_local_settings_patch(
            &mut merged,
            &serde_json::Value::Object(self.0.clone()),
            &shape,
        )?;
        let settings: GroupLocalSettings =
            serde_json::from_value(merged).map_err(|error| error.to_string())?;
        settings.validate()?;
        Ok(settings)
    }
}

fn normalize_local_settings_patch(
    value: &serde_json::Value,
    shape: &serde_json::Value,
) -> Result<Option<serde_json::Value>, String> {
    let Some(fields) = shape.as_object().filter(|fields| !fields.is_empty()) else {
        // 空对象模板是 memberStyles 这种动态映射；它和数组都是整体替换叶值。
        return Ok(Some(value.clone()));
    };
    let source = value
        .as_object()
        .ok_or_else(|| "备份群本机设置结构必须为对象".to_string())?;
    let mut patch = serde_json::Map::new();
    for (key, value) in source {
        let field_shape = fields
            .get(key)
            .ok_or_else(|| "备份群本机设置包含未知字段".to_string())?;
        if let Some(value) = normalize_local_settings_patch(value, field_shape)? {
            patch.insert(key.clone(), value);
        }
    }
    Ok((!patch.is_empty()).then_some(serde_json::Value::Object(patch)))
}

fn merge_local_settings_patch(
    target: &mut serde_json::Value,
    patch: &serde_json::Value,
    shape: &serde_json::Value,
) -> Result<(), String> {
    let Some(fields) = shape.as_object().filter(|fields| !fields.is_empty()) else {
        *target = patch.clone();
        return Ok(());
    };
    let source = patch
        .as_object()
        .ok_or_else(|| "备份群本机设置补丁必须为对象".to_string())?;
    let target = target
        .as_object_mut()
        .ok_or_else(|| "当前群本机设置结构无效".to_string())?;
    for (key, value) in source {
        let shape = fields
            .get(key)
            .ok_or_else(|| "备份群本机设置包含未知字段".to_string())?;
        let current = target
            .get_mut(key)
            .ok_or_else(|| "当前群本机设置缺少字段".to_string())?;
        merge_local_settings_patch(current, value, shape)?;
    }
    Ok(())
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct GroupModeSettings {
    pub sequential: SequentialModeSettings,
    pub jev: JevModeSettings,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct SequentialModeSettings {
    pub speaker_order: Vec<String>,
}

impl SequentialModeSettings {
    /// 成员关系由共享配置拥有；本机顺序只排列仍在群内的成员。
    pub fn ordered_members(&self, members: &[String]) -> Vec<String> {
        let mut ordered = Vec::with_capacity(members.len());
        for id in self.speaker_order.iter().chain(members) {
            if members.contains(id) && !ordered.contains(id) {
                ordered.push(id.clone());
            }
        }
        ordered
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct JevModeSettings {
    pub navigator_prompt: String,
    pub speaker_threshold: f64,
    pub continue_threshold: f64,
    pub stop_threshold: f64,
    pub max_speakers_per_round: u32,
    pub max_autonomous_rounds: u32,
    pub history_window: u32,
    pub continue_debounce_ms: u32,
    pub fallback_policy: String,
    pub member_styles: BTreeMap<String, String>,
}

impl Default for JevModeSettings {
    fn default() -> Self {
        Self {
            navigator_prompt: concat!(
                "你是智能群聊的发言导航员。请根据当前群聊的最新内容、成员身份与成员各自的发言触发风格，判断谁最有必要继续发言。",
                "优先考虑：被直接询问或点名、具备相关知识、能够补充新信息、需要回应误解或冲突、能够自然承接当前话题的成员。",
                "降低以下成员的优先级：刚刚已经充分表达、只会重复已有内容、与当前话题无关、没有实质推进价值的成员。",
                "如果问题已经解决、话题自然结束、继续回复只会重复或尴尬延长对话，请优先选择结束群聊。",
                "不要为了维持活跃而强行安排发言。"
            ).into(),
            speaker_threshold: 0.16,
            continue_threshold: 0.45,
            stop_threshold: 0.55,
            max_speakers_per_round: 3,
            max_autonomous_rounds: 12,
            history_window: 12,
            continue_debounce_ms: 800,
            fallback_policy: "stop".into(),
            member_styles: BTreeMap::new(),
        }
    }
}

impl JevModeSettings {
    pub fn validate(&self) -> Result<(), String> {
        for (name, value) in [
            ("发言权重阈值", self.speaker_threshold),
            ("继续讨论阈值", self.continue_threshold),
            ("结束阈值", self.stop_threshold),
        ] {
            if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                return Err(format!("{name}必须为 0–1 的有限数字"));
            }
        }
        if !(1..=32).contains(&self.max_speakers_per_round)
            || !(1..=100).contains(&self.max_autonomous_rounds)
            || !(1..=200).contains(&self.history_window)
            || self.continue_debounce_ms > 10_000
        {
            return Err("JEV 人数、轮数、历史窗口或防抖设置超出范围".into());
        }
        if !matches!(self.fallback_policy.as_str(), "stop" | "top_speaker") {
            return Err("JEV 回退策略不受支持".into());
        }
        if self.member_styles.keys().any(|id| id.trim().is_empty()) {
            return Err("JEV 成员风格不能包含空成员 ID".into());
        }
        Ok(())
    }
}

pub fn parse_group_local_settings(raw: Option<&str>) -> Result<GroupLocalSettings, String> {
    let settings: GroupLocalSettings = match raw {
        Some(raw) => serde_json::from_str(raw).map_err(|error| error.to_string())?,
        None => GroupLocalSettings::default(),
    };
    settings.validate()?;
    Ok(settings)
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
    /// 共享发言模式原值；本机执行入口必须使用 effective_mode()。
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
    /// 仅用于本机行为与显式备份，IPC 展平沿用模式参数名称。
    #[serde(flatten)]
    pub local_settings: GroupLocalSettings,
}

impl GroupConfig {
    pub fn effective_mode(&self) -> &str {
        self.local_settings.effective_mode(&self.mode)
    }
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

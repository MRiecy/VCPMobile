use crate::vcp_modules::topic_types::Topic;
use serde::de::{DeserializeOwned, Error as _};
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

#[derive(Debug, Serialize, Clone, Copy, Default, PartialEq, Eq)]
pub enum GroupChatMode {
    #[default]
    #[serde(rename = "sequential")]
    Sequential,
    #[serde(rename = "naturerandom")]
    NaturalRandom,
    #[serde(rename = "invite_only")]
    InviteOnly,
    #[serde(rename = "jev")]
    Jev,
}

impl GroupChatMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sequential => "sequential",
            Self::NaturalRandom => "naturerandom",
            Self::InviteOnly => "invite_only",
            Self::Jev => "jev",
        }
    }
}

impl std::fmt::Display for GroupChatMode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for GroupChatMode {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        Ok(match value.as_str() {
            Some("naturerandom") => Self::NaturalRandom,
            Some("invite_only") => Self::InviteOnly,
            Some("jev") => Self::Jev,
            _ => Self::default(),
        })
    }
}

#[derive(Debug, Serialize, Clone, Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JevFallbackPolicy {
    #[default]
    Stop,
    TopSpeaker,
}

impl<'de> Deserialize<'de> for JevFallbackPolicy {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        Ok(match value.as_str() {
            Some("top_speaker") => Self::TopSpeaker,
            _ => Self::default(),
        })
    }
}

fn local_field<T: DeserializeOwned>(value: &serde_json::Value, name: &str, default: T) -> T {
    value.get(name).and_then(|field| serde_json::from_value(field.clone()).ok()).unwrap_or(default)
}

fn default_tag_match_mode() -> Option<String> {
    Some("strict".to_string())
}

/// 本机群行为配置。它不属于 GroupSyncDTO，也不改变共享群的版本。
#[derive(Debug, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GroupLocalSettings {
    pub chat_mode: GroupChatMode,
    pub mode_settings: GroupModeSettings,
    pub enable_context_message_window: bool,
    pub context_message_window_size: u32,
}

impl Default for GroupLocalSettings {
    fn default() -> Self {
        Self {
            chat_mode: GroupChatMode::default(),
            mode_settings: GroupModeSettings::default(),
            enable_context_message_window: false,
            context_message_window_size: 100,
        }
    }
}

impl GroupLocalSettings {
    pub fn effective_mode(&self) -> GroupChatMode {
        self.chat_mode
    }

    pub fn normalize(&mut self) {
        if !(1..=10_000).contains(&self.context_message_window_size) {
            self.context_message_window_size = Self::default().context_message_window_size;
        }
        self.mode_settings.sequential.normalize();
        self.mode_settings.jev.normalize();
    }
}

impl<'de> Deserialize<'de> for GroupLocalSettings {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        let defaults = Self::default();
        let mut settings = Self {
            chat_mode: local_field(&value, "chatMode", defaults.chat_mode),
            mode_settings: local_field(&value, "modeSettings", defaults.mode_settings),
            enable_context_message_window: local_field(&value, "enableContextMessageWindow", defaults.enable_context_message_window),
            context_message_window_size: local_field(&value, "contextMessageWindowSize", defaults.context_message_window_size),
        };
        settings.normalize();
        Ok(settings)
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
        let Some(presence) = normalize_local_settings_patch(value, &shape) else {
            return Ok(Self(serde_json::Map::new()));
        };
        let mut merged = shape.clone();
        merge_local_settings_patch(&mut merged, &presence, &shape);
        let settings: GroupLocalSettings = serde_json::from_value(merged).map_err(|error| error.to_string())?;
        let canonical = serde_json::to_value(settings).map_err(|error| error.to_string())?;
        // 只投影明确出现的字段；默认化非法值不会把缺项也变成覆盖请求。
        let projected = project_local_settings_patch(&presence, &canonical, &shape);
        Ok(Self(projected.as_object().cloned().unwrap_or_default()))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn apply(&self, current: &GroupLocalSettings) -> Result<GroupLocalSettings, String> {
        let shape = serde_json::to_value(GroupLocalSettings::default())
            .map_err(|error| error.to_string())?;
        let mut merged = serde_json::to_value(current).map_err(|error| error.to_string())?;
        merge_local_settings_patch(&mut merged, &serde_json::Value::Object(self.0.clone()), &shape);
        serde_json::from_value(merged).map_err(|error| error.to_string())
    }
}

fn normalize_local_settings_patch(
    value: &serde_json::Value,
    shape: &serde_json::Value,
) -> Option<serde_json::Value> {
    let Some(fields) = shape.as_object().filter(|fields| !fields.is_empty()) else {
        // 空对象模板是 memberStyles 这种动态映射；它和数组都是整体替换叶值。
        return Some(value.clone());
    };
    let Some(source) = value.as_object() else { return Some(shape.clone()); };
    let mut patch = serde_json::Map::new();
    for (key, value) in source {
        let Some(field_shape) = fields.get(key) else { continue; };
        if let Some(value) = normalize_local_settings_patch(value, field_shape) {
            patch.insert(key.clone(), value);
        }
    }
    (!patch.is_empty()).then_some(serde_json::Value::Object(patch))
}

fn merge_local_settings_patch(
    target: &mut serde_json::Value,
    patch: &serde_json::Value,
    shape: &serde_json::Value,
) {
    let Some(fields) = shape.as_object().filter(|fields| !fields.is_empty()) else {
        *target = patch.clone();
        return;
    };
    let Some(source) = patch.as_object() else { return; };
    let Some(target) = target.as_object_mut() else { return; };
    for (key, value) in source {
        let Some(shape) = fields.get(key) else { continue; };
        let current = target.entry(key.clone()).or_insert_with(|| shape.clone());
        merge_local_settings_patch(current, value, shape);
    }
}

fn project_local_settings_patch(
    presence: &serde_json::Value,
    canonical: &serde_json::Value,
    shape: &serde_json::Value,
) -> serde_json::Value {
    let Some(fields) = shape.as_object().filter(|fields| !fields.is_empty()) else {
        return canonical.clone();
    };
    let mut projected = serde_json::Map::new();
    if let Some(presence) = presence.as_object() {
        for (key, field_shape) in fields {
            if let (Some(supplied), Some(value)) = (presence.get(key), canonical.get(key)) {
                projected.insert(key.clone(), project_local_settings_patch(supplied, value, field_shape));
            }
        }
    }
    serde_json::Value::Object(projected)
}

#[derive(Debug, Serialize, Clone, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GroupModeSettings {
    pub sequential: SequentialModeSettings,
    pub jev: JevModeSettings,
}

impl<'de> Deserialize<'de> for GroupModeSettings {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        Ok(Self {
            sequential: local_field(&value, "sequential", SequentialModeSettings::default()),
            jev: local_field(&value, "jev", JevModeSettings::default()),
        })
    }
}

#[derive(Debug, Serialize, Clone, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SequentialModeSettings {
    pub speaker_order: Vec<String>,
}

impl SequentialModeSettings {
    fn normalize(&mut self) {
        let mut seen = std::collections::BTreeSet::new();
        self.speaker_order.retain(|id| !id.trim().is_empty() && seen.insert(id.clone()));
    }
}

impl<'de> Deserialize<'de> for SequentialModeSettings {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        let mut settings = Self {
            speaker_order: value.get("speakerOrder").and_then(serde_json::Value::as_array)
                .map(|ids| ids.iter().filter_map(serde_json::Value::as_str).map(str::to_owned).collect())
                .unwrap_or_default(),
        };
        settings.normalize();
        Ok(settings)
    }
}

#[derive(Debug, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct JevModeSettings {
    pub navigator_prompt: String,
    pub speaker_threshold: f64,
    pub continue_threshold: f64,
    pub stop_threshold: f64,
    pub max_speakers_per_round: u32,
    pub max_autonomous_rounds: u32,
    pub history_window: u32,
    pub continue_debounce_ms: u32,
    pub fallback_policy: JevFallbackPolicy,
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
            fallback_policy: JevFallbackPolicy::default(),
            member_styles: BTreeMap::new(),
        }
    }
}

impl JevModeSettings {
    fn normalize(&mut self) {
        let defaults = Self::default();
        for (value, default) in [
            (&mut self.speaker_threshold, defaults.speaker_threshold),
            (&mut self.continue_threshold, defaults.continue_threshold),
            (&mut self.stop_threshold, defaults.stop_threshold),
        ] {
            if !value.is_finite() || !(0.0..=1.0).contains(value) {
                *value = default;
            }
        }
        for (value, max, default) in [
            (&mut self.max_speakers_per_round, 32, defaults.max_speakers_per_round),
            (&mut self.max_autonomous_rounds, 100, defaults.max_autonomous_rounds),
            (&mut self.history_window, 200, defaults.history_window),
        ] {
            if !(1..=max).contains(value) { *value = default; }
        }
        if self.continue_debounce_ms > 10_000 { self.continue_debounce_ms = defaults.continue_debounce_ms; }
        self.member_styles.retain(|id, _| !id.trim().is_empty());
    }
}

impl<'de> Deserialize<'de> for JevModeSettings {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        let defaults = Self::default();
        let mut settings = Self {
            navigator_prompt: local_field(&value, "navigatorPrompt", defaults.navigator_prompt),
            speaker_threshold: local_field(&value, "speakerThreshold", defaults.speaker_threshold),
            continue_threshold: local_field(&value, "continueThreshold", defaults.continue_threshold),
            stop_threshold: local_field(&value, "stopThreshold", defaults.stop_threshold),
            max_speakers_per_round: local_field(&value, "maxSpeakersPerRound", defaults.max_speakers_per_round),
            max_autonomous_rounds: local_field(&value, "maxAutonomousRounds", defaults.max_autonomous_rounds),
            history_window: local_field(&value, "historyWindow", defaults.history_window),
            continue_debounce_ms: local_field(&value, "continueDebounceMs", defaults.continue_debounce_ms),
            fallback_policy: local_field(&value, "fallbackPolicy", defaults.fallback_policy),
            member_styles: value.get("memberStyles").and_then(serde_json::Value::as_object)
                .map(|styles| styles.iter().map(|(id, style)| (id.clone(), style.as_str().unwrap_or_default().to_string())).collect())
                .unwrap_or_default(),
        };
        settings.normalize();
        Ok(settings)
    }
}

pub fn parse_group_local_settings(raw: Option<&str>) -> GroupLocalSettings {
    raw.and_then(|raw| serde_json::from_str(raw).ok()).unwrap_or_default()
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
    /// 仅保留共享存储/同步原值；本机执行只使用独立的 chatMode。
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
    pub fn effective_mode(&self) -> GroupChatMode {
        self.local_settings.effective_mode()
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
    /// 本机发言模式 (sequential, naturerandom, invite_only, jev)。
    /// 前端邀约横条/@选择器的模式判定以列表快照为准，缺此字段会导致
    /// invite_only 群在重启后永远识别不到（血训：曾被快照 DTO 丢弃）。
    #[serde(default)]
    pub mode: GroupChatMode,
}

use serde::{Deserialize, Serialize};

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

    /// 捕获所有未定义的字段
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

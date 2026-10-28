use crate::vcp_modules::topic_types::Topic;
use crate::vcp_modules::settings_manager::{VoiceTtsConfig, VoiceTtsMode, VOICE_TTS_VOICES};
use serde::{Deserialize, Serialize};

fn default_true() -> bool {
    true
}

/// Device-local voice identity; never included in AgentSyncDTO or its hash.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct AgentVoiceConfig {
    pub enabled: bool,
    pub mode: VoiceTtsMode,
    pub voice: String,
    pub clone_sample_id: String,
    pub stream: bool,
}

impl Default for AgentVoiceConfig {
    fn default() -> Self {
        let tts = VoiceTtsConfig::default();
        Self {
            enabled: true,
            mode: tts.mode,
            voice: tts.voice,
            clone_sample_id: tts.clone_sample_id,
            stream: tts.stream,
        }
    }
}

impl AgentVoiceConfig {
    /// Stored data is repaired one typed field at a time. This is deliberately
    /// separate from IPC deserialization: strings and numbers are not coerced.
    pub(crate) fn from_stored(value: &serde_json::Value) -> Self {
        fn field<T: serde::de::DeserializeOwned>(value: &serde_json::Value, key: &str, default: T) -> T {
            value.get(key).and_then(|value| serde_json::from_value(value.clone()).ok()).unwrap_or(default)
        }
        let defaults = Self::default();
        let mut result = Self {
            enabled: field(value, "enabled", defaults.enabled),
            mode: field(value, "mode", defaults.mode),
            voice: field(value, "voice", defaults.voice.clone()),
            clone_sample_id: field(value, "cloneSampleId", defaults.clone_sample_id.clone()),
            stream: field(value, "stream", defaults.stream),
        };
        if !VOICE_TTS_VOICES.contains(&result.voice.as_str()) { result.voice = defaults.voice; }
        result
    }
}

/// 智能体(Agent)的完整配置结构
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentConfig {
    /// 智能体 ID
    #[serde(default)]
    pub id: String,
    /// 智能体名称
    #[serde(default = "default_agent_name")]
    pub name: String,
    /// 系统提示词 (System Prompt)
    #[serde(default)]
    pub system_prompt: String,
    /// 移动端专用系统提示词 (不同步，仅本机生效)
    #[serde(default)]
    pub mobile_system_prompt: String,
    /// 使用的模型 (如: "gemini-2.0-flash")
    #[serde(default = "default_model")]
    pub model: String,
    /// 模型采样温度 (0.0-2.0)
    #[serde(default = "default_temperature")]
    pub temperature: f64,
    /// 上下文 Token 限制
    #[serde(default = "default_context_limit")]
    pub context_token_limit: i32,
    /// 单次输出最大 Token 数
    #[serde(default = "default_max_output")]
    pub max_output_tokens: i32,

    #[serde(default = "default_true")]
    pub stream_output: bool,

    /// 是否在请求中发送 temperature 参数（关闭可兼容不支持温度的模型）
    #[serde(default)]
    pub use_temperature: bool,

    /// 本机独立音色；关闭时继承全局默认配置。
    #[serde(default)]
    pub voice: AgentVoiceConfig,

    // avatars 表派生字段
    #[serde(default)]
    pub avatar_calculated_color: Option<String>,

    /// 话题列表
    #[serde(default)]
    pub topics: Vec<Topic>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            id: String::new(), name: default_agent_name(), system_prompt: String::new(),
            mobile_system_prompt: String::new(), model: default_model(), temperature: default_temperature(),
            context_token_limit: default_context_limit(), max_output_tokens: default_max_output(),
            stream_output: true, use_temperature: false, voice: AgentVoiceConfig::default(),
            avatar_calculated_color: None, topics: Vec::new(),
        }
    }
}

fn default_agent_name() -> String {
    "Unnamed Agent".to_string()
}
fn default_model() -> String {
    "gemini-2.5-flash".to_string()
}
fn default_temperature() -> f64 {
    1.0
}
fn default_context_limit() -> i32 {
    1000000
}
fn default_max_output() -> i32 {
    64000
}

/// 智能体(Agent)的轻量列表结构
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentListItem {
    pub id: String,
    pub name: String,
    pub model: String,
    pub avatar_calculated_color: Option<String>,
}

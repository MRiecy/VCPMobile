use crate::vcp_modules::settings_manager::{VoiceTtsConfig, VoiceTtsMode, VOICE_TTS_VOICES};
use crate::vcp_modules::topic_types::Topic;
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
        fn field<T: serde::de::DeserializeOwned>(
            value: &serde_json::Value,
            key: &str,
            default: T,
        ) -> T {
            value
                .get(key)
                .and_then(|value| serde_json::from_value(value.clone()).ok())
                .unwrap_or(default)
        }
        let defaults = Self::default();
        let mut result = Self {
            enabled: field(value, "enabled", defaults.enabled),
            mode: field(value, "mode", defaults.mode),
            voice: field(value, "voice", defaults.voice.clone()),
            clone_sample_id: field(value, "cloneSampleId", defaults.clone_sample_id.clone()),
            stream: field(value, "stream", defaults.stream),
        };
        if !VOICE_TTS_VOICES.contains(&result.voice.as_str()) {
            result.voice = defaults.voice;
        }
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
            id: String::new(),
            name: default_agent_name(),
            system_prompt: String::new(),
            mobile_system_prompt: String::new(),
            model: default_model(),
            temperature: default_temperature(),
            context_token_limit: default_context_limit(),
            max_output_tokens: default_max_output(),
            stream_output: true,
            use_temperature: false,
            voice: AgentVoiceConfig::default(),
            avatar_calculated_color: None,
            topics: Vec::new(),
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

use std::collections::BTreeMap;

/// 本机智能体专属配置（移动端提示词、模型采样温度开关、独立音色配置）。
/// 纯本地数据，不属于 AgentSyncDTO，绝不参与 config_hash 或 content_hash 计算，
/// 也绝不改变共享实体的 updated_at。
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct AgentLocalPreferences {
    /// 移动端专用系统提示词 (不同步，仅本机生效)
    #[serde(default)]
    pub mobile_system_prompt: String,

    /// 是否在模型请求中携带 temperature 参数 (不同步，关闭可兼容不支持温度的模型)
    #[serde(default)]
    pub use_temperature: bool,

    /// 本机独立音色配置；关闭时继承全局默认配置 (兼容 voice 与 voiceConfig 键名)
    #[serde(default, alias = "voiceConfig")]
    pub voice: AgentVoiceConfig,
}

/// 语义别名，与 GroupLocalSettings 保持命名一致性
#[allow(dead_code)]
pub type AgentLocalSettings = AgentLocalPreferences;

/// 导出类型契约
#[allow(dead_code)]
pub type AgentPreferencesExport = BTreeMap<String, AgentLocalPreferences>;

/// 导入补丁映射表
#[allow(dead_code)]
pub type AgentPreferencesImport = BTreeMap<String, AgentLocalSettingsPatch>;

/// 智能体本地设置稀疏补丁对象。
/// 备份恢复时只记录明确出现的字段；缺项绝不能在解析阶段提前补全为默认值，
/// 必须在 apply 时保留本地数据库已有现存值。
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
#[serde(transparent)]
pub struct AgentLocalSettingsPatch(pub serde_json::Map<String, serde_json::Value>);

impl AgentLocalSettingsPatch {
    /// 解析备份 JSON 对象中的智能体配置补丁
    pub fn parse(value: &serde_json::Value) -> Result<Self, String> {
        let mut patch = serde_json::Map::new();
        let Some(obj) = value.as_object() else {
            return Ok(Self(patch));
        };

        // 1. mobileSystemPrompt 兼容驼峰与下划线
        if let Some(prompt) = obj
            .get("mobileSystemPrompt")
            .or_else(|| obj.get("mobile_system_prompt"))
            .and_then(|v| v.as_str())
        {
            patch.insert("mobileSystemPrompt".into(), serde_json::json!(prompt));
        }

        // 2. useTemperature 兼容驼峰与下划线
        if let Some(use_temp) = obj
            .get("useTemperature")
            .or_else(|| obj.get("use_temperature"))
            .and_then(|v| v.as_bool())
        {
            patch.insert("useTemperature".into(), serde_json::json!(use_temp));
        }

        // 3. voice 兼容 voice 与 voiceConfig
        if let Some(voice_val) = obj.get("voice").or_else(|| obj.get("voiceConfig")) {
            let voice = AgentVoiceConfig::from_stored(voice_val);
            patch.insert(
                "voice".into(),
                serde_json::to_value(voice).map_err(|e| e.to_string())?,
            );
        }

        Ok(Self(patch))
    }

    /// 补丁是否为空
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// 将稀疏补丁应用到当前已有配置上，未出现字段严格保留原值
    pub fn apply(&self, current: &AgentLocalPreferences) -> Result<AgentLocalPreferences, String> {
        let mut restored = current.clone();
        if let Some(prompt) = self
            .0
            .get("mobileSystemPrompt")
            .or_else(|| self.0.get("mobile_system_prompt"))
            .and_then(|v| v.as_str())
        {
            restored.mobile_system_prompt = prompt.to_string();
        }
        if let Some(use_temp) = self
            .0
            .get("useTemperature")
            .or_else(|| self.0.get("use_temperature"))
            .and_then(|v| v.as_bool())
        {
            restored.use_temperature = use_temp;
        }
        if let Some(voice_val) = self.0.get("voice").or_else(|| self.0.get("voiceConfig")) {
            restored.voice = AgentVoiceConfig::from_stored(voice_val);
        }
        Ok(restored)
    }
}

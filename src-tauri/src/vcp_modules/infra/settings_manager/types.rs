//! Persisted configuration and IPC contracts. Defaults live with their domain.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
pub use tauri_plugin_vcp_mobile::voice::{VoiceVadConfig, VoiceVadSensitivity};

pub const VOICE_RULE_MAX_CHARS: usize = 8000;
pub const VOICE_GUIDANCE_MAX_CHARS: usize = 2000;
pub const VOICE_LANGUAGES: &[&str] = &["auto", "zh", "en"];
pub const VOICE_TTS_VOICES: &[&str] = &[
    "mimo_default",
    "冰糖",
    "茉莉",
    "苏打",
    "白桦",
    "Mia",
    "Chloe",
    "Milo",
    "Dean",
];
pub const SYNC_LOG_LEVELS: &[&str] = &["TRACE", "DEBUG", "INFO", "WARN", "ERROR"];
pub const DEFAULT_VOICE_AUDIO_TAG_RULES: &str = "回复将用于语音通话。保持当前角色和正常对话内容，保留已选音色，只控制情绪、语气和节奏；只在确有表达需要时添加简短音频标签。整体风格写在正文开头的半角圆括号内，例如（温柔 平静）应写为 (温柔 平静)；局部声音或情绪写在对应文字前的方括号内，例如 [轻笑]、[叹气]、[低声]。标签使用自然语言，完整闭合，不解释标签，不使用唱歌模式，不把整句正文放入标签。";
pub const DEFAULT_VOICE_GUIDANCE_RULES: &str = "在每条助手回复的正文之前，为本条回复生成一份自然语言风格指导。依据当前角色、对话场景和情绪，描述语气、语速与必要的停顿；保留已选预置音色或参考样本的声线，不重新设计声音。用一至四句清晰描述，避免互相矛盾的要求，不要求混响、回声等后期效果。指导只描述如何说，不重复待朗读的正文。";

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "config-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub enum ChatEndpointMode {
    #[default]
    Standard,
    VcpTools,
    Raw,
}

impl ChatEndpointMode {
    pub const ALL: [Self; 3] = [Self::Standard, Self::VcpTools, Self::Raw];
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "config-export", derive(ts_rs::TS))]
pub enum VoiceInputEncoding {
    #[default]
    #[serde(rename = "wav")]
    Wav,
    #[serde(rename = "mp3_64")]
    Mp3,
}

impl VoiceInputEncoding {
    pub const ALL: [Self; 2] = [Self::Wav, Self::Mp3];
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "config-export", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum VoiceTtsMode {
    #[default]
    Builtin,
    Clone,
}

impl VoiceTtsMode {
    pub const ALL: [Self; 2] = [Self::Builtin, Self::Clone];
}

// Serde's struct-level default uses exactly the same source as new installs and
// field recovery; there are no separate per-field deserialization defaults.
macro_rules! config_struct {
    ($name:ident { $($field:ident: $ty:ty = $default:expr),* $(,)? }) => {
        #[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
        #[cfg_attr(feature = "config-export", derive(ts_rs::TS))]
        #[serde(default, rename_all = "camelCase")]
        pub struct $name { $(pub $field: $ty,)* }
        impl Default for $name {
            fn default() -> Self { Self { $($field: $default,)* } }
        }
    };
}

config_struct!(IdentityConfig {
    user_name: String = "用户".into()
});
config_struct!(ServerConfig {
    url: String = String::new(),
    api_key: String = String::new(),
    endpoint_mode: ChatEndpointMode = ChatEndpointMode::default(),
    admin_username: String = String::new(),
    admin_password: String = String::new(),
    file_key: String = String::new(),
});
config_struct!(LogConfig {
    url: String = String::new(),
    key: String = String::new()
});
config_struct!(SyncConfig {
    server_url: String = String::new(),
    http_url: String = String::new(),
    token: String = String::new(),
    log_level: String = "INFO".into(),
    prerender_enabled: bool = false,
});
config_struct!(DistributedConfig {
    enabled: bool = false,
    ws_url: String = String::new(),
    vcp_key: String = String::new(),
    device_name: String = "VCPMobile".into(),
});
config_struct!(ConversationConfig {
    topic_summary_model: String = "gemini-3.1-flash-lite".into()
});
config_struct!(VoiceConnectionConfig {
    api_base_url: String = "https://api.xiaomimimo.com/v1".into(),
    api_key: String = String::new(),
});
config_struct!(VoiceAsrConfig {
    stream: bool = true,
    input_encoding: VoiceInputEncoding = VoiceInputEncoding::default(),
    language: String = "auto".into(),
});
config_struct!(VoiceTtsConfig {
    stream: bool = true,
    voice: String = "mimo_default".into(),
    mode: VoiceTtsMode = VoiceTtsMode::default(),
    style_prompt: String = String::new(),
    clone_sample_id: String = String::new(),
});
config_struct!(VoiceGuidanceConfig {
    audio_tag_rules: String = DEFAULT_VOICE_AUDIO_TAG_RULES.into(),
    auto_guidance: bool = true,
    guidance_rules: String = DEFAULT_VOICE_GUIDANCE_RULES.into(),
});
config_struct!(VoiceConfig {
    connection: VoiceConnectionConfig = VoiceConnectionConfig::default(),
    asr: VoiceAsrConfig = VoiceAsrConfig::default(),
    tts: VoiceTtsConfig = VoiceTtsConfig::default(),
    guidance: VoiceGuidanceConfig = VoiceGuidanceConfig::default(),
    vad: VoiceVadConfig = VoiceVadConfig::default(),
});
config_struct!(NavigationConfig {
    agent_order: Vec<String> = Vec::new(),
    group_order: Vec<String> = Vec::new(),
});
config_struct!(Settings {
    identity: IdentityConfig = IdentityConfig::default(),
    server: ServerConfig = ServerConfig::default(),
    log: LogConfig = LogConfig::default(),
    sync: SyncConfig = SyncConfig::default(),
    distributed: DistributedConfig = DistributedConfig::default(),
    conversation: ConversationConfig = ConversationConfig::default(),
    voice: VoiceConfig = VoiceConfig::default(),
    navigation: NavigationConfig = NavigationConfig::default(),
});

impl Settings {
    pub fn voice_vad_config(&self) -> VoiceVadConfig {
        self.voice.vad
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "config-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub enum ConfigDomain {
    Identity,
    Server,
    Log,
    Sync,
    Distributed,
    Conversation,
    Voice,
    Navigation,
}

impl ConfigDomain {
    pub const ALL: [Self; 8] = [
        Self::Identity,
        Self::Server,
        Self::Log,
        Self::Sync,
        Self::Distributed,
        Self::Conversation,
        Self::Voice,
        Self::Navigation,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Identity => "identity",
            Self::Server => "server",
            Self::Log => "log",
            Self::Sync => "sync",
            Self::Distributed => "distributed",
            Self::Conversation => "conversation",
            Self::Voice => "voice",
            Self::Navigation => "navigation",
        }
    }

    pub const fn schema_version(self) -> u32 {
        1
    }
}

impl std::fmt::Display for ConfigDomain {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl std::str::FromStr for ConfigDomain {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|domain| domain.as_str() == value)
            .ok_or_else(|| "未知的配置领域".to_string())
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "config-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub enum DomainHealth {
    Ready,
    Recovered,
    Unavailable,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "config-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct FieldIssue {
    pub domain: ConfigDomain,
    pub field: String,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "config-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct DomainStatus {
    pub revision: String,
    pub schema_version: u32,
    pub issues: Vec<FieldIssue>,
    pub health: DomainHealth,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "config-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ConfigSnapshot {
    pub data: Settings,
    // Repository snapshots always publish a status for every known domain.
    #[cfg_attr(
        feature = "config-export",
        ts(type = "{ [key in ConfigDomain]: DomainStatus }")
    )]
    pub domains: BTreeMap<ConfigDomain, DomainStatus>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "config-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct DomainPatch {
    pub domain: ConfigDomain,
    pub expected_revision: String,
    pub changes: serde_json::Map<String, serde_json::Value>,
    pub base_values: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    pub repair: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "config-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ConfigUpdate {
    pub patches: Vec<DomainPatch>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "config-export", derive(ts_rs::TS))]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum ConfigSaveResult {
    Saved {
        snapshot: ConfigSnapshot,
    },
    Rejected {
        issues: Vec<FieldIssue>,
    },
    Conflict {
        snapshot: ConfigSnapshot,
        conflicts: Vec<FieldIssue>,
    },
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "config-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub enum FieldKind {
    String,
    Boolean,
    Integer,
    StringArray,
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "config-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub enum ConfigEffect {
    Immediate,
    NextRequest,
    NextInput,
    NextNarration,
    NextVoiceTask,
    NextCall,
    NextSync,
    Reconnect,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "config-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct FieldDescriptor {
    pub domain: ConfigDomain,
    pub path: String,
    pub kind: FieldKind,
    pub backup: bool,
    pub secret: bool,
    pub validation_group: String,
    pub effect: ConfigEffect,
    pub options: Vec<String>,
    pub required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "config-export", ts(optional))]
    pub min: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "config-export", ts(optional))]
    pub max: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "config-export", ts(optional))]
    pub max_chars: Option<usize>,
}

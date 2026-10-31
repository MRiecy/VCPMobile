//! Persisted configuration and IPC contracts. Defaults live with their domain.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
pub use tauri_plugin_vcp_mobile::voice::{VoiceVadConfig, VoiceVadSensitivity};

pub const VOICE_RULE_MAX_CHARS: usize = 8000;
pub const VOICE_GUIDANCE_MAX_CHARS: usize = 2000;
pub const VOICE_API_BASE_URL_PRESET: &str = "https://api.xiaomimimo.com/v1";
pub const JEV_APP_TITLE_PRESET: &str = "VCPMobile";

/// Empty persisted overrides select a preset only when consumed.
pub fn value_or_preset<'a>(value: &'a str, preset: &'a str) -> &'a str {
    let value = value.trim();
    if value.is_empty() {
        preset
    } else {
        value
    }
}
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
pub const DEFAULT_VOICE_GUIDANCE_RULES: &str = r#"———VCPMobile语音系统自动注入———
现在你是在VCP的实时语音聊天中，像真实交谈一样口语化聊天就好啦。
VCP现在支持全新语音控制：
1.开头写 <voice_guidance>语音指导块，可以用两三句话描述本轮的声音状态，比如声线、语速、气息和情绪等。例如：
<voice_guidance>
声音放松，语速稍慢，带一点刚睡醒的懒意。
</voice_guidance>

2.换行后，以 (风格) 开始正文，简单标出本轮的整体感觉，如：
(平静)、(温柔 慵懒)、(带着哽咽的笑意)、(压抑的愤怒)。

3.正文中可以顺手加入少量 [音频标签]，让情绪转折更自然，例如：
[轻笑]、[叹气]、[吸气]、[小声]。
标签在合适的地方点一下就好，不加也没问题；只描述声音，不把动作写进去。

整体参考：
<voice_guidance>
延续柔和的青年音，语速放慢，疲惫里带着一点被陪伴后的安心。
</voice_guidance>
(温柔 疲惫)怎么还没睡？……[轻笑]我没事，就是今天有点累。你陪我说会儿话，好不好？"#;

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

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "config-export", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum JevProvider {
    #[default]
    Typesafe,
    Openrouter,
}

impl JevProvider {
    pub const ALL: [Self; 2] = [Self::Typesafe, Self::Openrouter];

    pub const fn api_url_preset(self) -> &'static str {
        match self {
            Self::Typesafe => "https://api.typesafe.ai/v1/systemone",
            Self::Openrouter => "https://openrouter.ai/api/alpha/decisions",
        }
    }

    pub const fn model_preset(self) -> &'static str {
        match self {
            Self::Typesafe => "jev-latest",
            Self::Openrouter => "~typesafe/jev-latest",
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "config-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct JevProviderConfig {
    pub api_url: String,
    pub api_key: String,
    pub model: String,
}

// Serde's struct-level default uses exactly the same source as new installs and
// field normalization; there are no separate per-field defaults.
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
    api_base_url: String = String::new(),
    api_key: String = String::new(),
});
config_struct!(VoiceAsrConfig {
    stream: bool = true,
    input_encoding: VoiceInputEncoding = VoiceInputEncoding::default(),
    language: String = "auto".into(),
});
config_struct!(VoiceTtsConfig {
    stream: bool = true,
    voice: String = "冰糖".into(),
    mode: VoiceTtsMode = VoiceTtsMode::default(),
    clone_sample_id: String = String::new(),
});
config_struct!(VoiceGuidanceConfig {
    rules: String = DEFAULT_VOICE_GUIDANCE_RULES.into(),
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
config_struct!(JevConfig {
    enabled: bool = false,
    provider: JevProvider = JevProvider::default(),
    typesafe: JevProviderConfig = JevProviderConfig::default(),
    openrouter: JevProviderConfig = JevProviderConfig::default(),
    timeout_ms: u32 = 30_000,
    max_retries: u32 = 2,
    retry_base_delay_ms: u32 = 500,
    proxy_url: String = String::new(),
    http_referer: String = String::new(),
    app_title: String = String::new(),
});

impl JevConfig {
    pub fn active_provider(&self) -> &JevProviderConfig {
        match self.provider {
            JevProvider::Typesafe => &self.typesafe,
            JevProvider::Openrouter => &self.openrouter,
        }
    }
}

config_struct!(Settings {
    identity: IdentityConfig = IdentityConfig::default(),
    server: ServerConfig = ServerConfig::default(),
    log: LogConfig = LogConfig::default(),
    sync: SyncConfig = SyncConfig::default(),
    distributed: DistributedConfig = DistributedConfig::default(),
    conversation: ConversationConfig = ConversationConfig::default(),
    voice: VoiceConfig = VoiceConfig::default(),
    jev: JevConfig = JevConfig::default(),
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
    Jev,
    Navigation,
}

impl ConfigDomain {
    pub const ALL: [Self; 9] = [
        Self::Identity,
        Self::Server,
        Self::Log,
        Self::Sync,
        Self::Distributed,
        Self::Conversation,
        Self::Voice,
        Self::Jev,
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
            Self::Jev => "jev",
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

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "config-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ConfigBackupImportResult {
    pub result: ConfigSaveResult,
    pub missing_groups: bool,
    pub missing_agents: bool,
}

#[allow(dead_code)]
pub type SettingsImportRequest = ConfigUpdate;

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
    // Runtime fallback, also shown as the editor placeholder. Not stored input.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "config-export", ts(optional))]
    pub preset: Option<String>,
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

//! Input recognition and narration have independent resource owners. Only
//! explicit call actions coordinate chat cancellation with narration stopping.

#![cfg_attr(
    not(any(target_os = "android", target_os = "ios", test)),
    allow(dead_code)
)]

mod audio_codec;
mod batch;
mod cache;
mod call;
mod chat_feed;
mod document;
mod input;
mod network;
mod pipeline;
mod readable;
mod samples;
mod segmentation;
mod sse;
mod tts;
mod wav;
mod workers;

pub(crate) use call::target_deleted;
pub use call::{
    begin_voice_conversation, end_voice_conversation, get_voice_conversation_snapshot,
    voice_call_list_inputs, voice_call_select_input, VoiceCallState,
};
pub use document::{allocate_voice_document, connect_voice_document, VoiceDocumentState};
#[cfg(any(target_os = "android", target_os = "ios"))]
pub use input::handle_audio_event;
#[cfg(any(target_os = "android", target_os = "ios"))]
pub use input::invalidate_document;
pub use input::{close_voice_microphone, start_voice_microphone, VoiceServiceState};
pub use samples::{
    delete_voice_clone_sample, get_voice_clone_sample, import_voice_clone_sample,
    list_voice_clone_samples,
};
#[cfg(any(target_os = "android", target_os = "ios"))]
pub use tts::handle_playback_event;
pub(crate) use tts::stop_bubble_for_messages;
pub use tts::{
    connect_tts_document, feed_chat_event, has_chat_listener, preview_tts,
    preview_voice_clone_sample, start_tts_session, stop_chat_reply_by_origin, stop_tts,
    submit_tts_text, TtsServiceState,
};

use crate::vcp_modules::settings_manager::{
    value_or_preset, Settings, VoiceInputEncoding, VoiceTtsMode, VOICE_API_BASE_URL_PRESET,
    VOICE_LANGUAGES, VOICE_TTS_VOICES,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceServiceConfig {
    pub base_url: String,
    pub api_key: String,
    pub asr_stream: bool,
    #[serde(default)]
    pub input_encoding: VoiceInputEncoding,
    pub tts_stream: bool,
    pub language: String,
    pub voice: String,
    #[serde(default)]
    pub tts_mode: VoiceTtsMode,
    #[serde(default)]
    pub clone_sample_id: String,
}

/// One immutable global snapshot supplies connection, guidance and inherited
/// defaults for a reply. Each message owns its resolved speaker voice and audio.
#[derive(Clone)]
pub(crate) struct VoiceReplyPolicy {
    pub(crate) config: VoiceServiceConfig,
    pub(crate) guidance_rules: String,
}

impl VoiceReplyPolicy {
    pub(crate) fn from_settings(settings: &Settings) -> Self {
        Self {
            guidance_rules: settings.voice.guidance.rules.clone(),
            config: VoiceServiceConfig::from_settings(settings),
        }
    }
}

/// The payload carries its actual format independently of later settings edits.
pub(super) struct InputAudio {
    pub bytes: Vec<u8>,
    pub encoding: VoiceInputEncoding,
}

impl VoiceServiceConfig {
    fn with_agent_voice(&self, voice: &crate::vcp_modules::agent_types::AgentVoiceConfig) -> Self {
        let mut config = self.clone();
        if voice.enabled {
            config.tts_mode = voice.mode;
            config.voice = voice.voice.clone();
            config.clone_sample_id = voice.clone_sample_id.clone();
            config.tts_stream = voice.stream;
        }
        config
    }

    pub(crate) fn from_settings(settings: &Settings) -> Self {
        Self {
            base_url: settings.voice.connection.api_base_url.clone(),
            api_key: settings.voice.connection.api_key.trim().to_string(),
            asr_stream: settings.voice.asr.stream,
            input_encoding: settings.voice.asr.input_encoding,
            tts_stream: settings.voice.tts.stream,
            language: settings.voice.asr.language.clone(),
            voice: settings.voice.tts.voice.clone(),
            tts_mode: settings.voice.tts.mode,
            clone_sample_id: settings.voice.tts.clone_sample_id.clone(),
        }
    }
    fn configured(&self) -> bool {
        !self.api_key.is_empty()
    }
    fn has_selected_voice(&self) -> bool {
        match self.tts_mode {
            VoiceTtsMode::Builtin => true,
            VoiceTtsMode::Clone => !self.clone_sample_id.is_empty(),
        }
    }
    fn endpoint(&self) -> Result<String, String> {
        let base = value_or_preset(&self.base_url, VOICE_API_BASE_URL_PRESET).trim_end_matches('/');
        let endpoint = format!("{base}/chat/completions");
        let parsed = url::Url::parse(&endpoint).map_err(|_| "语音 API 地址无效。")?;
        if !matches!(parsed.scheme(), "https" | "http") || parsed.host_str().is_none() {
            return Err("语音 API 地址需要使用 HTTP 或 HTTPS。".into());
        }
        Ok(endpoint)
    }
    pub(crate) fn validate_asr(&self) -> Result<(), String> {
        if !self.configured() {
            return Ok(());
        }
        self.endpoint()?;
        if !VOICE_LANGUAGES.contains(&self.language.as_str()) {
            return Err("语音识别语言配置无效。".into());
        }
        Ok(())
    }

    pub(crate) fn validate_tts(&self) -> Result<(), String> {
        if !self.has_selected_voice() {
            return Err("当前未选择朗读音色，请先选择音色。".into());
        }
        if self.tts_mode == VoiceTtsMode::Builtin
            && !VOICE_TTS_VOICES.contains(&self.voice.as_str())
        {
            return Err("语音合成音色配置无效。".into());
        }
        if !self.configured() {
            return Ok(());
        }
        self.endpoint()?;
        Ok(())
    }

    pub(super) fn tts_streaming(&self) -> bool {
        self.tts_mode == VoiceTtsMode::Builtin && self.tts_stream
    }
}

#[derive(Serialize)]
pub struct VoiceServiceStart {
    configured: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_and_custom_voice_addresses_resolve_to_valid_endpoints() {
        let mut settings = Settings::default();
        for blank in ["", " \t "] {
            settings.voice.connection.api_base_url = blank.into();
            let config = VoiceServiceConfig::from_settings(&settings);
            assert_eq!(
                config.endpoint().unwrap(),
                "https://api.xiaomimimo.com/v1/chat/completions"
            );
        }
        settings.voice.connection.api_base_url = " https://voice.example/v1/ ".into();
        let mut config = VoiceServiceConfig::from_settings(&settings);
        assert_eq!(
            config.endpoint().unwrap(),
            "https://voice.example/v1/chat/completions"
        );
        config.base_url = "broken-address".into();
        assert!(config.endpoint().is_err());
    }

    #[test]
    fn narration_misconfiguration_cannot_block_asr() {
        let mut settings = Settings::default();
        settings.voice.connection.api_key = "fixture-key".into();
        settings.voice.tts.mode = VoiceTtsMode::Builtin;
        settings.voice.tts.voice = "invalid-voice".into();
        let mut config = VoiceServiceConfig::from_settings(&settings);
        assert!(config.validate_asr().is_ok());
        assert!(config.validate_tts().is_err());
        config.voice = "none".into();
        assert!(config.validate_asr().is_ok());
        assert!(config.has_selected_voice());
        assert!(config.validate_tts().unwrap_err().contains("音色配置无效"));
        config.tts_mode = VoiceTtsMode::Clone;
        assert!(config.validate_asr().is_ok());
        assert!(config.validate_tts().is_err());
        config.tts_mode = VoiceTtsMode::Builtin;
        config.voice = "mimo_default".into();
        config.language = "invalid-language".into();
        assert!(config.validate_asr().is_err());
        assert!(config.validate_tts().is_ok());
    }

    #[test]
    fn only_clone_can_have_an_empty_selection() {
        assert!(serde_json::from_str::<VoiceTtsMode>("\"none\"").is_err());
        let mut config = VoiceServiceConfig::from_settings(&Settings::default());
        assert_eq!(config.tts_mode, VoiceTtsMode::Builtin);
        assert_eq!(config.voice, "冰糖");
        assert!(config.has_selected_voice());
        assert!(config.validate_tts().is_ok());
        config.voice = "none".into();
        assert!(config.has_selected_voice());
        assert!(config.validate_tts().unwrap_err().contains("音色配置无效"));
        config.tts_mode = VoiceTtsMode::Clone;
        assert!(!config.has_selected_voice());
        assert!(config
            .validate_tts()
            .unwrap_err()
            .contains("未选择朗读音色"));
        config.clone_sample_id = "0123456789abcdef0123456789abcdef".into();
        assert!(config.has_selected_voice());
        assert_eq!(config.tts_mode, VoiceTtsMode::Clone);
    }
}

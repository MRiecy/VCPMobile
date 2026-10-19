//! Input recognition and narration have independent resource owners. Only
//! explicit call actions coordinate chat cancellation with narration stopping.

#![cfg_attr(
    not(any(target_os = "android", target_os = "ios", test)),
    allow(dead_code)
)]

mod batch;
mod call;
mod chat_feed;
mod input;
mod network;
mod readable;
mod samples;
mod segmentation;
mod sse;
mod tts;
mod wav;
mod workers;

pub use call::{
    begin_voice_conversation, end_voice_conversation, get_voice_conversation_snapshot,
    VoiceCallState,
};
#[cfg(any(target_os = "android", target_os = "ios", test))]
pub use input::handle_audio_event;
pub use input::{close_voice_microphone, start_voice_microphone, VoiceServiceState};
pub use tts::{
    feed_chat_event, has_chat_listener, preview_tts, start_tts_session, stop_chat_reply_by_origin, stop_tts,
    submit_tts_text, TtsServiceState,
};
pub use samples::{discard_voice_clone_sample, get_voice_clone_sample, import_voice_clone_sample};

use crate::vcp_modules::settings_manager::{
    Settings, VoiceInputEncoding, VoiceTtsMode, DEFAULT_VOICE_AUDIO_TAG_RULES,
    DEFAULT_VOICE_GUIDANCE_RULES, VOICE_LANGUAGES, VOICE_TTS_VOICES,
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
    pub tts_style_prompt: String,
    #[serde(default)]
    pub clone_sample_id: String,
}

/// One immutable preparation snapshot is shared by context injection and every
/// narration in this reply. Imported sample bytes have their own narration lease.
#[derive(Clone)]
pub(crate) struct VoiceReplyPolicy {
    pub(crate) config: VoiceServiceConfig,
    pub(crate) audio_tag_rules: String,
    pub(crate) auto_guidance: bool,
    pub(crate) guidance_rules: String,
}

impl VoiceReplyPolicy {
    pub(crate) fn from_settings(settings: &Settings) -> Self {
        Self {
            audio_tag_rules: settings.voice.guidance.audio_tag_rules.clone(),
            auto_guidance: settings.voice.guidance.auto_guidance,
            guidance_rules: settings.voice.guidance.guidance_rules.clone(),
            config: VoiceServiceConfig::from_settings(settings),
        }
    }

    pub(crate) fn context_injection(&self) -> String {
        let tags = self.audio_tag_rules.trim();
        let tags = if tags.is_empty() {
            DEFAULT_VOICE_AUDIO_TAG_RULES
        } else {
            tags
        };
        if !self.auto_guidance {
            return tags.to_string();
        }
        let guidance = self.guidance_rules.trim();
        let guidance = if guidance.is_empty() {
            DEFAULT_VOICE_GUIDANCE_RULES
        } else {
            guidance
        };
        format!("{tags}\n{guidance}\n风格指导输出格式：在每条助手消息的可朗读正文之前，只输出一次以下控制块。起止标记必须独占一行，内部只放本条回复的完整自然语言指导，不嵌套、不放进代码块、不等待工具回执。控制块之后再输出带音频标签的正常正文。\n<<<[VOICE_GUIDANCE]>>>\n本条回复的风格指导\n<<<[END_VOICE_GUIDANCE]>>>")
    }
}

/// The payload carries its actual format independently of later settings edits.
pub(super) struct InputAudio {
    pub bytes: Vec<u8>,
    pub encoding: VoiceInputEncoding,
}

impl VoiceServiceConfig {
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
            tts_style_prompt: settings.voice.tts.style_prompt.clone(),
            clone_sample_id: settings.voice.tts.clone_sample_id.clone(),
        }
    }
    fn configured(&self) -> bool {
        !self.api_key.is_empty()
    }
    fn endpoint(&self) -> Result<String, String> {
        let base = self.base_url.trim().trim_end_matches('/');
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
        if !self.configured() {
            return Ok(());
        }
        self.endpoint()?;
        if self.tts_mode == VoiceTtsMode::Builtin
            && !VOICE_TTS_VOICES.contains(&self.voice.as_str())
        {
            return Err("语音合成音色配置无效。".into());
        }
        if self.tts_mode == VoiceTtsMode::Clone && self.clone_sample_id.trim().is_empty() {
            return Err("请先导入音色复刻参考音频。".into());
        }
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
    fn narration_misconfiguration_cannot_block_asr() {
        let mut settings = Settings::default();
        settings.voice.connection.api_key = "fixture-key".into();
        settings.voice.tts.voice = "invalid-voice".into();
        let mut config = VoiceServiceConfig::from_settings(&settings);
        assert!(config.validate_asr().is_ok());
        assert!(config.validate_tts().is_err());
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
    fn both_modes_share_one_injection_and_preserve_the_automatic_switch() {
        let mut settings = Settings::default();
        settings.voice.guidance.audio_tag_rules = "标签规则".into();
        settings.voice.guidance.guidance_rules = "指导规则".into();
        for mode in [VoiceTtsMode::Builtin, VoiceTtsMode::Clone] {
            settings.voice.tts.mode = mode;
            settings.voice.guidance.auto_guidance = true;
            let policy = VoiceReplyPolicy::from_settings(&settings);
            let injection = policy.context_injection();
            assert!(injection.starts_with("标签规则\n指导规则\n"));
            assert_eq!(injection.matches("<<<[VOICE_GUIDANCE]>>>").count(), 1);
            settings.voice.guidance.auto_guidance = false;
            let without_automatic = VoiceReplyPolicy::from_settings(&settings);
            assert_eq!(without_automatic.context_injection(), "标签规则");
            assert!(policy.auto_guidance);
        }
    }
}

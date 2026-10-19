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
    feed_chat_event, has_chat_listener, start_tts_session, stop_chat_reply_by_origin, stop_tts,
    submit_tts_text, TtsServiceState,
};

use crate::vcp_modules::settings_manager::Settings;
use serde::{Deserialize, Serialize};

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceServiceConfig {
    pub base_url: String,
    pub api_key: String,
    pub asr_stream: bool,
    pub tts_stream: bool,
    pub language: String,
    pub voice: String,
}

impl VoiceServiceConfig {
    fn from_settings(settings: Settings) -> Self {
        Self {
            base_url: settings.voice_api_base_url,
            api_key: settings.voice_api_key.trim().to_string(),
            asr_stream: settings.voice_asr_stream,
            tts_stream: settings.voice_tts_stream,
            language: settings.voice_language,
            voice: settings.voice_tts_voice,
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
    fn validate(&self) -> Result<(), String> {
        if !self.configured() {
            return Ok(());
        }
        self.endpoint()?;
        if !matches!(self.language.as_str(), "auto" | "zh" | "en") {
            return Err("语音识别语言配置无效。".into());
        }
        if !matches!(
            self.voice.as_str(),
            "mimo_default" | "冰糖" | "茉莉" | "苏打" | "白桦" | "Mia" | "Chloe" | "Milo" | "Dean"
        ) {
            return Err("语音合成音色配置无效。".into());
        }
        Ok(())
    }
}

#[derive(Serialize)]
pub struct VoiceServiceStart {
    configured: bool,
}

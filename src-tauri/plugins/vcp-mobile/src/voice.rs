//! Native audio control and shared VAD decisions. PCM and inference stay in the
//! native capture owner; this module receives bounded frame summaries only.

mod vad;
mod native_audio;

pub use native_audio::{append_playback, end_playback_segment, export_segment, finish_playback, release_segments, start_playback, stop_playback, ExportedVoiceSegment};
pub use vad::{VoiceVadConfig, VoiceVadSensitivity};

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, Runtime};

use vad::VadDetector;

pub const VOICE_AUDIO_EVENT: &str = "vcp-voice-audio";
const CANCELLED_SESSION_LIMIT: usize = 128;

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AecState {
    Enabled,
    Unavailable,
    Failed,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceFrame {
    pub session_id: String,
    pub audio_epoch: u32,
    /// End sample of this 512-sample inference frame in the 16 kHz domain.
    pub sample_index: u64,
    pub probability: f32,
    pub level: f32,
    pub bands: [f32; 32],
    pub aec_state: AecState,
    pub route: String,
    pub playback_active: bool,
    /// TTS output identity at capture time.
    #[serde(default)]
    pub playback_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceNotice {
    pub session_id: String,
    pub audio_epoch: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_index: Option<u64>,
    pub reason: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureStopReceipt {
    pub audio_epoch: u32,
    pub sample_index: u64,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum NativeVoiceAudioEvent {
    Frame(VoiceFrame),
    Stopped(VoiceNotice),
    Error(VoiceNotice),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SpeechTransition {
    Start,
    End,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeechFrame {
    #[serde(flatten)]
    pub frame: VoiceFrame,
    pub speech_active: bool,
    pub speech_transition: Option<SpeechTransition>,
    pub discontinuity: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum VoiceAudioEvent {
    Frame(SpeechFrame),
    Stopped(VoiceNotice),
    Error(VoiceNotice),
}

struct ActiveSession {
    id: String,
    generation: u64,
    detector: VadDetector,
}

#[derive(Default)]
struct Sessions {
    generation: u64,
    current: Option<ActiveSession>,
    cancelled: VecDeque<String>,
}

impl Sessions {
    fn begin(&mut self, id: String, vad_config: VoiceVadConfig) -> Result<u64, String> {
        if self.cancelled.contains(&id) {
            return Err("cancelled: voice audio session was already stopped".into());
        }
        if self.current.is_some() {
            // A late start has no authority to replace another capture owner.
            // A normal stop revokes the slot before waiting for native cleanup.
            return Err(
                "voice_session_busy: stop the active microphone before starting another".into(),
            );
        }
        vad_config.validate()?;
        self.generation = self.generation.wrapping_add(1);
        self.current = Some(ActiveSession {
            id,
            generation: self.generation,
            detector: VadDetector::new(vad_config),
        });
        Ok(self.generation)
    }

    fn remember_cancelled(&mut self, id: &str) {
        if self.cancelled.iter().any(|cancelled| cancelled == id) {
            return;
        }
        if self.cancelled.len() == CANCELLED_SESSION_LIMIT {
            self.cancelled.pop_front();
        }
        self.cancelled.push_back(id.to_string());
    }

    fn owns(&self, id: &str, generation: u64) -> bool {
        self.current
            .as_ref()
            .is_some_and(|session| session.id == id && session.generation == generation)
    }

    fn cancel(&mut self, id: &str, generation: Option<u64>) -> Option<u32> {
        // Cancellation can reach Rust before the matching start future is polled.
        // Remember it even when no capture (or a different one) currently owns us.
        self.remember_cancelled(id);
        let current = self.current.as_ref()?;
        if current.id != id || generation.is_some_and(|value| value != current.generation) {
            return None;
        }
        let epoch = current.detector.audio_epoch().unwrap_or(0);
        self.current = None;
        Some(epoch)
    }

    fn consume_frame(
        &mut self,
        frame: VoiceFrame,
    ) -> Option<VoiceAudioEvent> {
        let session = self.current.as_mut()?;
        if session.id != frame.session_id {
            return None;
        }
        let result = session.detector.process(frame)?;
        Some(VoiceAudioEvent::Frame(result))
    }

    fn consume_terminal(&mut self, notice: VoiceNotice, error: bool) -> Option<VoiceAudioEvent> {
        let current = self.current.as_ref()?;
        if current.id != notice.session_id
            || current
                .detector
                .audio_epoch()
                .is_some_and(|epoch| notice.audio_epoch < epoch)
        {
            return None;
        }
        self.current = None;
        self.remember_cancelled(&notice.session_id);
        Some(if error {
            VoiceAudioEvent::Error(notice)
        } else {
            VoiceAudioEvent::Stopped(notice)
        })
    }
}

#[derive(Default)]
pub(crate) struct VoiceAudioState(Mutex<Sessions>);

fn validate_session(id: &str) -> Result<(), String> {
    if id.is_empty() || id.len() > 128 {
        return Err("voice_audio: invalid sessionId".into());
    }
    Ok(())
}

fn owns<R: Runtime>(app: &AppHandle<R>, id: &str, generation: u64) -> bool {
    app.state::<VoiceAudioState>()
        .0
        .lock()
        .is_ok_and(|sessions| sessions.owns(id, generation))
}

async fn native_call<R: Runtime>(
    app: AppHandle<R>,
    operation: &'static str,
    args: serde_json::Value,
    expected: Option<(String, u64)>,
) -> Result<(), String> {
    native_result_call::<R, serde_json::Value>(app, operation, args, expected)
        .await
        .map(|_| ())
}

async fn native_result_call<R: Runtime, T: serde::de::DeserializeOwned + Send + 'static>(
    app: AppHandle<R>,
    operation: &'static str,
    args: serde_json::Value,
    expected: Option<(String, u64)>,
) -> Result<T, String> {
    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        let handle = app
            .state::<crate::VcpMobileState<R>>()
            .mobile_plugin_handle()?;
        // Keep the native receiver alive even if a frontend invocation is dropped.
        // A separate stop command can cancel a permission/model-start operation.
        tauri::async_runtime::spawn_blocking(move || {
            let allowed = expected
                .as_ref()
                .is_none_or(|(id, generation)| {
                    app.state::<VoiceAudioState>()
                        .0
                        .lock()
                        .is_ok_and(|sessions| sessions.owns(id, *generation))
                });
            if !allowed {
                return Err("cancelled: voice audio session is no longer active".into());
            }
            handle
                .run_mobile_plugin::<T>(operation, args)
                .map_err(|error| format!("voice_audio: {operation}: {error}"))
        })
        .await
        .map_err(|error| format!("voice_audio_worker: {error}"))?
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        let _ = (app, operation, args, expected);
        Err("unsupported: native voice audio requires Android or iOS".into())
    }
}

/// Rust call coordination owns admission; this is not a frontend command.
pub async fn begin_voice_call<R: Runtime>(app: AppHandle<R>, call_id: String) -> Result<(), String> {
    validate_session(&call_id)?;
    native_call(app, "beginVoiceCall", serde_json::json!({ "callId": call_id }), None).await
}

pub async fn end_voice_call<R: Runtime>(app: AppHandle<R>, call_id: String) -> Result<(), String> {
    validate_session(&call_id)?;
    native_call(app, "endVoiceCall", serde_json::json!({ "callId": call_id }), None).await
}

/// A document grants capture admission only; it never owns the logical call.
pub async fn connect_voice_document<R: Runtime>(app: AppHandle<R>, document_id: String) -> Result<(), String> {
    native_call(app, "connectVoiceDocument", serde_json::json!({ "documentId": document_id }), None).await
}

pub async fn invalidate_voice_document<R: Runtime>(app: AppHandle<R>, document_id: String) -> Result<(), String> {
    native_call(app, "invalidateVoiceDocument", serde_json::json!({ "documentId": document_id }), None).await
}

pub async fn start_voice_audio<R: Runtime>(
    app: AppHandle<R>,
    session_id: String,
    call_id: String,
    document_id: String,
    vad_config: VoiceVadConfig,
) -> Result<(), String> {
    validate_session(&session_id)?;
    validate_session(&call_id)?;
    let generation = app
        .state::<VoiceAudioState>()
        .0
        .lock()
        .map_err(|error| error.to_string())?
        .begin(session_id.clone(), vad_config)?;
    let result = native_call(
        app.clone(),
        "startVoiceAudio",
        serde_json::json!({ "sessionId": session_id, "callId": call_id, "documentId": document_id }),
        Some((session_id.clone(), generation)),
    )
    .await;
    if !owns(&app, &session_id, generation) {
        // Native admission may finish after hangup or a replacement call.
        // Its session identity prevents this cleanup from touching the new call.
        let _ = native_call(
            app,
            "stopVoiceAudio",
            serde_json::json!({ "sessionId": session_id }),
            None,
        )
        .await;
        // A native failure can publish its terminal event before the start
        // receiver settles. Keep that concrete failure for the call owner;
        // losing the capture slot alone must not turn it into cancellation.
        return result.and(Err(
            "cancelled: voice audio session was replaced or stopped".into(),
        ));
    }
    if let Err(error) = &result {
        let epoch = app
            .state::<VoiceAudioState>()
            .0
            .lock()
            .map_err(|lock_error| lock_error.to_string())?
            .cancel(&session_id, Some(generation));
        if let Some(audio_epoch) = epoch {
            let _ = app.emit(
                VOICE_AUDIO_EVENT,
                VoiceAudioEvent::Error(VoiceNotice {
                    session_id,
                    audio_epoch,
                    sample_index: None,
                    reason: error.clone(),
                }),
            );
        }
    }
    result
}

pub async fn stop_voice_audio<R: Runtime>(
    app: AppHandle<R>,
    session_id: String,
) -> Result<CaptureStopReceipt, String> {
    validate_session(&session_id)?;
    // Revoke acceptance before awaiting native cleanup, including during start.
    let epoch = app
        .state::<VoiceAudioState>()
        .0
        .lock()
        .map_err(|error| error.to_string())?
        .cancel(&session_id, None);
    let receipt: CaptureStopReceipt = native_result_call(
        app.clone(),
        "stopVoiceAudio",
        serde_json::json!({ "sessionId": session_id }),
        None,
    )
    .await?;
    if epoch.is_some() {
        let _ = app.emit(
            VOICE_AUDIO_EVENT,
            VoiceAudioEvent::Stopped(VoiceNotice {
                session_id,
                audio_epoch: receipt.audio_epoch,
                sample_index: Some(receipt.sample_index),
                reason: "requested".into(),
            }),
        );
    }
    Ok(receipt)
}

/// Called by the existing process-wide native event subscription. No PCM enters
/// this boundary, and no native invocation is made synchronously on its thread.
pub fn handle_native_event<R: Runtime>(app: &AppHandle<R>, event: NativeVoiceAudioEvent) -> Option<VoiceAudioEvent> {
    let event = {
        let state = app.state::<VoiceAudioState>();
        let Ok(mut sessions) = state.0.lock() else {
            return None;
        };
        match event {
            NativeVoiceAudioEvent::Stopped(notice) => sessions.consume_terminal(notice, false),
            NativeVoiceAudioEvent::Error(notice) => sessions.consume_terminal(notice, true),
            NativeVoiceAudioEvent::Frame(frame) => sessions.consume_frame(frame),
        }
    }?;
    let _ = app.emit(VOICE_AUDIO_EVENT, &event);
    Some(event)
}

#[cfg(test)]
mod tests;

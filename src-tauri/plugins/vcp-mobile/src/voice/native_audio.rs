//! Internal audio transport for the application voice service. WAV paths and
//! PCM blocks cross only the Rust/native boundary, never the WebView.

use super::{native_call, native_result_call, validate_session};
use serde::Deserialize;
use tauri::{AppHandle, Runtime};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportedVoiceSegment {
    pub path: String,
    pub sample_rate: u32,
    pub sample_count: u64,
}

pub async fn export_segment<R: Runtime>(
    app: AppHandle<R>, session_id: String, audio_epoch: u32,
    start_sample: u64, end_sample: u64,
) -> Result<ExportedVoiceSegment, String> {
    validate_session(&session_id)?;
    if end_sample <= start_sample || end_sample - start_sample > 30 * 16_000 {
        return Err("voice_segment_range_invalid".into());
    }
    native_result_call(app, "exportVoiceSegment", serde_json::json!({
        "sessionId": session_id, "audioEpoch": audio_epoch,
        "startSample": start_sample, "endSample": end_sample,
    }), None).await
}

/// Closing input stops new samples; releasing segments is separate so an
/// already recorded utterance can finish exporting after the microphone closes.
pub async fn release_segments<R: Runtime>(app: AppHandle<R>, session_id: String) -> Result<(), String> {
    validate_session(&session_id)?;
    native_call(app, "releaseVoiceSegments", serde_json::json!({ "sessionId": session_id }), None).await
}

pub async fn start_playback<R: Runtime>(
    app: AppHandle<R>, playback_id: String,
) -> Result<(), String> {
    validate_session(&playback_id)?;
    native_call(app, "startVoicePlayback", serde_json::json!({
        "playbackId": playback_id,
    }), None).await
}

pub async fn append_playback<R: Runtime>(
    app: AppHandle<R>, playback_id: String, pcm_base64: String,
) -> Result<(), String> {
    validate_session(&playback_id)?;
    // The native boundary also validates decoded PCM alignment and size.
    if pcm_base64.is_empty() || pcm_base64.len() > 43_692 {
        return Err("voice_playback_chunk_invalid".into());
    }
    native_call(app, "appendVoicePlayback", serde_json::json!({
        "playbackId": playback_id, "pcmBase64": pcm_base64,
    }), None).await
}

pub async fn finish_playback<R: Runtime>(
    app: AppHandle<R>, playback_id: String,
) -> Result<(), String> {
    validate_session(&playback_id)?;
    native_call(app, "finishVoicePlayback", serde_json::json!({
        "playbackId": playback_id,
    }), None).await
}

pub async fn stop_playback<R: Runtime>(
    app: AppHandle<R>, playback_id: String,
) -> Result<(), String> {
    validate_session(&playback_id)?;
    // Playback has its own lifetime. Native tombstones also handle a stop that
    // reaches the output owner before its delayed start.
    native_call(app, "stopVoicePlayback", serde_json::json!({
        "playbackId": playback_id,
    }), None).await
}

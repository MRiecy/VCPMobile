use super::ffmpeg_cli::run_ffmpeg;
use base64::Engine as _;
use std::path::Path;

/// 处理音频：提取为 16kHz 单声道 WAV，返回 base64 data URL
pub fn process_audio_for_multimodal(path: &Path) -> Result<String, String> {
    let wav_bytes = run_ffmpeg(&[
        "-i",
        path.to_str().ok_or("Invalid audio path")?,
        "-vn",
        "-acodec",
        "pcm_s16le",
        "-ar",
        "16000",
        "-ac",
        "1",
        "-f",
        "wav",
        "pipe:1",
    ])?;

    let b64 = base64::engine::general_purpose::STANDARD.encode(&wav_bytes);
    Ok(format!("data:audio/wav;base64,{}", b64))
}

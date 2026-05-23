use super::ffmpeg_cli::run_ffmpeg;
use base64::Engine as _;
use std::path::Path;

/// 处理音频：提取为 16kHz 单声道 MP3 (32kbps)，返回 base64 data URL
/// 硬截断最大时长为 3500 秒（约 58 分钟），以确保 Base64 后的请求体在 20MB 以内
pub fn process_audio_for_multimodal(path: &Path) -> Result<String, String> {
    let mp3_bytes = run_ffmpeg(&[
        "-t",
        "3500", // 硬截断：最大 3500 秒
        "-i",
        path.to_str().ok_or("Invalid audio path")?,
        "-vn",
        "-c:a",
        "libmp3lame",
        "-b:a",
        "32k", // 32kbps 码率
        "-ar",
        "16000",
        "-ac",
        "1",
        "-f",
        "mp3",
        "pipe:1",
    ])?;

    // 优化 Base64 拼接：预分配内存并直接编码到 String
    let prefix = "data:audio/mpeg;base64,";
    let b64_len = (mp3_bytes.len() * 4).div_ceil(3);
    let mut result = String::with_capacity(prefix.len() + b64_len);
    result.push_str(prefix);
    base64::engine::general_purpose::STANDARD.encode_string(&mp3_bytes, &mut result);

    Ok(result)
}

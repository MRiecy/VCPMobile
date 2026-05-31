use super::ffmpeg_cli::run_ffmpeg;
use base64::Engine as _;
use std::path::Path;

/// 处理音频：优先提取为 16kHz 单声道 MP3 (32kbps)，返回 base64 data URL
/// 如果 ffmpeg 异常失败（如 Android 10+ 的 SELinux 限制），则自动降级为直接读取原音频的物理二进制字节转 base64
pub fn process_audio_for_multimodal(path: &Path) -> Result<String, String> {
    let mp3_bytes_res = run_ffmpeg(&[
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
    ]);

    let (bytes, mime_type) = match mp3_bytes_res {
        Ok(bytes) => (bytes, "audio/mpeg".to_string()),
        Err(err) => {
            log::warn!(
                "[AudioExtractor] ffmpeg transcode failed: {}. Falling back to direct raw audio bytes.",
                err
            );
            
            let bytes = std::fs::read(path).map_err(|e| format!("Failed to read raw audio: {}", e))?;
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("mp3")
                .to_lowercase();
            
            let mime = match ext.as_str() {
                "mp3" => "audio/mpeg",
                "wav" => "audio/wav",
                "ogg" => "audio/ogg",
                "flac" => "audio/flac",
                "aac" => "audio/aac",
                "m4a" => "audio/mp4",
                "opus" => "audio/opus",
                _ => "audio/mpeg", // 兜底
            };
            (bytes, mime.to_string())
        }
    };

    // 优化 Base64 拼接：预分配内存并直接编码到 String
    let prefix = format!("data:{};base64,", mime_type);
    let b64_len = (bytes.len() * 4).div_ceil(3);
    let mut result = String::with_capacity(prefix.len() + b64_len);
    result.push_str(&prefix);
    base64::engine::general_purpose::STANDARD.encode_string(&bytes, &mut result);

    Ok(result)
}

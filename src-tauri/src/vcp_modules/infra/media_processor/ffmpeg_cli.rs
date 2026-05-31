use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

// =============================================================================
// Android: Native Library Directory Symlink寻址机制
// =============================================================================

/// 获取 ffmpeg 可执行文件路径
pub fn get_ffmpeg_path() -> Result<PathBuf, String> {
    #[cfg(target_os = "android")]
    {
        let temp_str = std::env::temp_dir().to_string_lossy().to_string();
        let parts: Vec<&str> = temp_str.split('/').collect();
        let mut package_name = "com.vcp.avatar".to_string();
        for part in parts {
            if part.starts_with("com.vcp.") {
                package_name = part.to_string();
                break;
            }
        }

        // 优先尝试寻找被系统合法解压且赋予了可执行 (+x) 权限的 Native 库路径
        for base_dir in &[
            format!("/data/data/{}/lib", package_name),
            format!("/data/user/0/{}/lib", package_name),
        ] {
            let path = PathBuf::from(base_dir).join("libffmpeg.so");
            if path.exists() {
                return Ok(path);
            }
        }

        Err("libffmpeg.so not found in JNI dynamic library directories".to_string())
    }

    #[cfg(not(target_os = "android"))]
    {
        for name in [
            "ffmpeg",
            "/usr/bin/ffmpeg",
            "/usr/local/bin/ffmpeg",
            "/opt/homebrew/bin/ffmpeg",
        ] {
            if Command::new(name).arg("-version").output().is_ok() {
                return Ok(PathBuf::from(name));
            }
        }
        Err("ffmpeg not found in PATH".to_string())
    }
}

/// 获取 ffprobe 可执行文件路径
pub fn get_ffprobe_path() -> Result<PathBuf, String> {
    #[cfg(target_os = "android")]
    {
        let temp_str = std::env::temp_dir().to_string_lossy().to_string();
        let parts: Vec<&str> = temp_str.split('/').collect();
        let mut package_name = "com.vcp.avatar".to_string();
        for part in parts {
            if part.starts_with("com.vcp.") {
                package_name = part.to_string();
                break;
            }
        }

        // 优先尝试寻找被系统合法解压且赋予了可执行 (+x) 权限的 Native 库路径
        for base_dir in &[
            format!("/data/data/{}/lib", package_name),
            format!("/data/user/0/{}/lib", package_name),
        ] {
            let path = PathBuf::from(base_dir).join("libffprobe.so");
            if path.exists() {
                return Ok(path);
            }
        }

        Err("libffprobe.so not found in JNI dynamic library directories".to_string())
    }

    #[cfg(not(target_os = "android"))]
    {
        for name in [
            "ffprobe",
            "/usr/bin/ffprobe",
            "/usr/local/bin/ffprobe",
            "/opt/homebrew/bin/ffprobe",
        ] {
            if Command::new(name).arg("-version").output().is_ok() {
                return Ok(PathBuf::from(name));
            }
        }
        Err("ffprobe not found in PATH".to_string())
    }
}

/// 运行 ffmpeg 命令，返回 stdout bytes
pub fn run_ffmpeg(args: &[&str]) -> Result<Vec<u8>, String> {
    let ffmpeg = get_ffmpeg_path()?;
    let output = Command::new(&ffmpeg)
        .args(args)
        .output()
        .map_err(|e| format!("Failed to run ffmpeg: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("ffmpeg error ({}): {}", output.status, stderr));
    }

    Ok(output.stdout)
}

/// 运行 ffprobe 命令，返回解析后的 JSON
pub fn run_ffprobe(args: &[&str]) -> Result<serde_json::Value, String> {
    let ffprobe = get_ffprobe_path()?;
    let output = Command::new(&ffprobe)
        .args(args)
        .output()
        .map_err(|e| format!("Failed to run ffprobe: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("ffprobe error ({}): {}", output.status, stderr));
    }

    serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("Failed to parse ffprobe JSON: {}", e))
}

/// 获取视频时长（秒）
pub fn get_video_duration(path: &Path) -> Result<f64, String> {
    let json = run_ffprobe(&[
        "-v",
        "quiet",
        "-print_format",
        "json",
        "-show_format",
        path.to_str().ok_or("Invalid video path")?,
    ])?;

    let duration = json["format"]["duration"]
        .as_f64()
        .or_else(|| json["format"]["duration"].as_str()?.parse().ok())
        .ok_or("Duration not found in ffprobe output")?;

    Ok(duration)
}

/// 场景切换检测，返回所有场景切换时间点（秒）
pub fn detect_scene_changes(path: &Path) -> Result<Vec<f64>, String> {
    use lazy_static::lazy_static;
    use regex::Regex;

    lazy_static! {
        static ref PTS_TIME_RE: Regex = Regex::new(r"pts_time:([\d.]+)").unwrap();
    }

    let ffmpeg = get_ffmpeg_path()?;
    let output = Command::new(&ffmpeg)
        .args([
            "-i",
            path.to_str().ok_or("Invalid video path")?,
            "-vf",
            "select='gt(scene,0.3)',showinfo",
            "-f",
            "null",
            "-",
        ])
        .output()
        .map_err(|e| format!("Failed to run ffmpeg scene detection: {}", e))?;

    // scene detection writes to stderr
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut timestamps = Vec::new();

    for cap in PTS_TIME_RE.captures_iter(&stderr) {
        if let Ok(ts) = cap[1].parse::<f64>() {
            timestamps.push(ts);
        }
    }

    Ok(timestamps)
}

/// 在指定时间戳提取单帧，返回 JPEG bytes，并在 FFmpeg 层缩放到 1280x720 比例框内
pub fn extract_single_frame(path: &Path, timestamp_secs: f64) -> Result<Vec<u8>, String> {
    run_ffmpeg(&[
        "-ss",
        &format!("{:.3}", timestamp_secs),
        "-i",
        path.to_str().ok_or("Invalid video path")?,
        "-vf",
        "scale='if(gt(iw/ih,1.777778),min(1280,iw),-1)':'if(gt(iw/ih,1.777778),-1,min(720,ih))'",
        "-vframes",
        "1",
        "-q:v",
        "2",
        "-f",
        "image2pipe",
        "-vcodec",
        "mjpeg",
        "pipe:1",
    ])
}

/// 利用 FFmpeg 内存管道将任意格式的头像字节流解码并等比例缩放到 128x128（大图降采样，小图保持原样），输出 Raw RGBA 像素流
#[allow(dead_code)]
pub fn decode_avatar_to_rgba(image_bytes: &[u8]) -> Result<Vec<u8>, String> {
    let ffmpeg = get_ffmpeg_path()?;
    let mut child = Command::new(&ffmpeg)
        .args([
            "-i", "pipe:0",
            "-vf", "scale='if(gt(max(iw,ih),128),if(gt(iw,ih),128,-1),iw)':'if(gt(max(iw,ih),128),if(gt(iw,ih),-1,128),ih)'",
            "-f", "rawvideo",
            "-pix_fmt", "rgba",
            "pipe:1"
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Failed to spawn ffmpeg: {}", e))?;

    // 在另一个线程异步写入 stdin，彻底防止由于 pipe 写满导致的死锁
    let mut stdin = child.stdin.take().ok_or("Failed to open FFmpeg stdin")?;
    let bytes_to_write = image_bytes.to_vec();
    std::thread::spawn(move || {
        let _ = stdin.write_all(&bytes_to_write);
    });

    let output = child
        .wait_with_output()
        .map_err(|e| format!("Failed to wait for FFmpeg: {}", e))?;
    if !output.status.success() {
        return Err("FFmpeg raw image decode failed".to_string());
    }
    Ok(output.stdout)
}

// =============================================================================
// 回归测试：多模态 JPG / PNG 压制抽帧 与 36MB 大 WAV 音频极速压制
// =============================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcp_modules::infra::media_processor::image_extractor::convert_local_image_for_multimodal;
    use crate::vcp_modules::infra::media_processor::audio_extractor::process_audio_for_multimodal;
    use std::path::Path;

    #[test]
    fn test_multimodal_image_processing() {
        let test_dir = Path::new("G:\\VCPMobile\\scripts\\onimi-test");
        if !test_dir.exists() {
            println!("onimi-test assets folder not found, skipping.");
            return;
        }

        // 1. JPG 压缩与等比例 WebP 转换测试
        let jpg_path = test_dir.join("Screenshot_2026-05-22-18-28-35-35_8379d4c9027515b.jpg");
        if jpg_path.exists() {
            println!("Testing JPG compression for Screenshot...");
            let res = convert_local_image_for_multimodal(&jpg_path);
            assert!(res.is_ok(), "JPG scale and WebP convert failed: {:?}", res.err());
            let b64 = res.unwrap();
            assert!(b64.starts_with("data:image/webp;base64,"), "MIME type must be webp");
            println!("JPG Success! WebP Base64 length: {}", b64.len());
        } else {
            println!("JPG test asset not found.");
        }

        // 2. PNG 压缩与等比例 WebP 转换测试
        let png_path = test_dir.join("_G__VCPMobile_releases_v1.0.0_announcement.html.png");
        if png_path.exists() {
            println!("Testing PNG compression for Release announcement...");
            let res = convert_local_image_for_multimodal(&png_path);
            assert!(res.is_ok(), "PNG scale and WebP convert failed: {:?}", res.err());
            let b64 = res.unwrap();
            assert!(b64.starts_with("data:image/webp;base64,"), "MIME type must be webp");
            println!("PNG Success! WebP Base64 length: {}", b64.len());
        } else {
            println!("PNG test asset not found.");
        }
    }

    #[test]
    fn test_multimodal_audio_processing() {
        let test_dir = Path::new("G:\\VCPMobile\\scripts\\onimi-test");
        if !test_dir.exists() {
            println!("onimi-test assets folder not found, skipping.");
            return;
        }

        // 3. 36MB 级长 WAV 音频转码压制为 16kHz Mono 32kbps MP3 测试
        let wav_path = test_dir.join("llCIqeE8c_rUL1Kik5Fu8HfFeUyX.wav");
        if wav_path.exists() {
            println!("Testing 36MB WAV transcoding and mono resampling to MP3...");
            let start = std::time::Instant::now();
            let res = process_audio_for_multimodal(&wav_path);
            assert!(res.is_ok(), "WAV to MP3 compression failed: {:?}", res.err());
            let b64 = res.unwrap();
            assert!(b64.starts_with("data:audio/mpeg;base64,"), "MIME type must be audio/mpeg");
            println!("WAV Success! MP3 Base64 length: {}, Elapsed time: {:?}", b64.len(), start.elapsed());
        } else {
            println!("WAV test asset not found.");
        }
    }
}


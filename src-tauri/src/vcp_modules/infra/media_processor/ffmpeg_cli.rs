use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

// =============================================================================
// Android: embed ffmpeg/ffprobe binaries at compile time
// =============================================================================
#[cfg(target_os = "android")]
mod android {
    pub const FFMPEG_BINARY: &[u8] = include_bytes!("assets/ffmpeg_aarch64");
    pub const FFPROBE_BINARY: &[u8] = include_bytes!("assets/ffprobe_aarch64");
}

#[cfg(target_os = "android")]
fn extract_android_binary(name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    if bytes.len() < 1024 {
        return Err(format!(
            "Embedded {} binary is empty or too small ({} bytes). \
             Please build the real ffmpeg binary for Android and place it at \
             src-tauri/src/vcp_modules/media_processor/assets/{}_aarch64",
            name,
            bytes.len(),
            name
        ));
    }

    let cache_dir = std::env::temp_dir();
    let path = cache_dir.join(name);

    if !path.exists() {
        std::fs::write(&path, bytes)
            .map_err(|e| format!("Failed to write {} to cache: {}", name, e))?;
    }

    Ok(path)
}

/// 获取 ffmpeg 可执行文件路径
pub fn get_ffmpeg_path() -> Result<PathBuf, String> {
    #[cfg(target_os = "android")]
    {
        extract_android_binary("ffmpeg", android::FFMPEG_BINARY)
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
        extract_android_binary("ffprobe", android::FFPROBE_BINARY)
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

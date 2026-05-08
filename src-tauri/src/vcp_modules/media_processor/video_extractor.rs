use super::ffmpeg_cli::{detect_scene_changes, extract_single_frame, get_video_duration, run_ffmpeg};
use base64::Engine as _;
use image::DynamicImage;
use std::path::Path;

/// 内部软上限：防止极端长视频导致 OOM 或 API 超时
const MAX_FRAMES: usize = 300;

/// 去重阈值：时间戳差小于此值（秒）的帧视为重复
const DEDUP_THRESHOLD_SECS: f64 = 1.5;

/// 缩放到 720p 边界框内（最大 1280×720，保持比例）
fn scale_to_720p(img: &DynamicImage) -> DynamicImage {
    let (w, h) = (img.width(), img.height());
    let max_w = 1280u32;
    let max_h = 720u32;

    if w <= max_w && h <= max_h {
        return img.clone();
    }

    let scale = (max_w as f32 / w as f32).min(max_h as f32 / h as f32);
    let new_w = (w as f32 * scale) as u32;
    let new_h = (h as f32 * scale) as u32;

    img.resize(new_w, new_h, image::imageops::FilterType::Lanczos3)
}

/// JPEG bytes → 缩放 → base64 data URL
fn frame_to_data_url(jpeg_bytes: &[u8]) -> Result<String, String> {
    let img = image::load_from_memory(jpeg_bytes).map_err(|e| e.to_string())?;
    let scaled = scale_to_720p(&img);

    let mut buf = Vec::new();
    {
        let mut cursor = std::io::Cursor::new(&mut buf);
        scaled
            .write_to(&mut cursor, image::ImageFormat::Jpeg)
            .map_err(|e| e.to_string())?;
    }

    let b64 = base64::engine::general_purpose::STANDARD.encode(&buf);
    Ok(format!("data:image/jpeg;base64,{}", b64))
}

/// 处理视频：返回帧序列（每张帧为 base64 data URL）
///
/// 策略：
/// - ≤60s：场景切换 + 1fps 均匀采样
/// - >60s：场景切换 + 0.5fps 均匀采样
/// - 合并去重后限制 300 帧
pub fn process_video_for_multimodal(path: &Path) -> Result<Vec<String>, String> {
    // 1. 视频基本信息
    let duration = get_video_duration(path)?;
    let fps = if duration <= 60.0 { 1.0 } else { 0.5 };

    // 2. 场景切换检测
    let scene_times = detect_scene_changes(path)?;

    // 3. 计算均匀采样时间戳
    let mut all_times: Vec<f64> = Vec::new();
    let mut t = 0.0;
    while t < duration {
        all_times.push(t);
        t += 1.0 / fps;
    }

    // 4. 合并场景帧
    all_times.extend(scene_times);
    all_times.sort_by(|a, b| a.partial_cmp(b).unwrap());

    // 5. 去重
    let mut deduped: Vec<f64> = Vec::new();
    for ts in all_times {
        if deduped.is_empty() || (ts - deduped.last().unwrap()).abs() >= DEDUP_THRESHOLD_SECS {
            deduped.push(ts);
        }
    }

    // 6. 限制总帧数（均匀降采样）
    if deduped.len() > MAX_FRAMES {
        let step = deduped.len() as f64 / MAX_FRAMES as f64;
        let mut sampled = Vec::with_capacity(MAX_FRAMES);
        let mut i = 0.0;
        while i < deduped.len() as f64 {
            sampled.push(deduped[i as usize]);
            i += step;
        }
        deduped = sampled;
    }

    // 7. 批量提取均匀帧到临时目录
    let temp_dir = std::env::temp_dir().join(format!("vcp_video_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp_dir)
        .map_err(|e| format!("Failed to create temp dir: {}", e))?;

    let fps_str = if fps == 1.0 { "1" } else { "0.5" };
    run_ffmpeg(&[
        "-i",
        path.to_str().ok_or("Invalid video path")?,
        "-vf",
        &format!("fps={},scale='min(1280,iw)':-1", fps_str),
        "-q:v",
        "2",
        temp_dir.join("frame_%04d.jpg").to_str().ok_or("Invalid temp path")?,
    ])?;

    // 8. 读取均匀帧：帧编号 → JPEG bytes
    let mut uniform_frames: Vec<(usize, Vec<u8>)> = Vec::new();
    for entry in std::fs::read_dir(&temp_dir)
        .map_err(|e| format!("Failed to read temp dir: {}", e))?
    {
        let entry = entry.map_err(|e| e.to_string())?;
        let p = entry.path();
        if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
            if let Ok(idx) = stem.parse::<usize>() {
                let bytes = std::fs::read(&p).map_err(|e| e.to_string())?;
                uniform_frames.push((idx, bytes));
            }
        }
    }
    uniform_frames.sort_by_key(|(idx, _)| *idx);

    // 9. 为每个需要的时间戳匹配最接近的均匀帧，缺失则单独提取
    let mut results = Vec::with_capacity(deduped.len());

    for ts in &deduped {
        // ffmpeg frame_%04d 从 1 开始：frame_0001 → t=0, frame_0002 → t=1/fps, ...
        let expected_idx = (ts * fps).round() as usize + 1;

        let jpeg_bytes = if let Some((_, bytes)) = uniform_frames.iter().find(|(idx, _)| *idx == expected_idx)
        {
            bytes.clone()
        } else {
            // 场景帧不在均匀采样中，单独提取
            extract_single_frame(path, *ts)?
        };

        results.push(frame_to_data_url(&jpeg_bytes)?);
    }

    // 10. 清理临时目录
    let _ = std::fs::remove_dir_all(&temp_dir);

    Ok(results)
}

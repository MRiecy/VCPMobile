use super::ffmpeg_cli::{
    detect_scene_changes, extract_single_frame, get_video_duration, run_ffmpeg,
};
use base64::Engine as _;
use std::path::Path;

/// 内部软上限：防止极端长视频导致 OOM 或 API 超时
const MAX_FRAMES: usize = 300;

/// 去重阈值：时间戳差小于此值（秒）的帧视为重复
const DEDUP_THRESHOLD_SECS: f64 = 1.5;

/// JPEG bytes → base64 data URL，已由 FFmpeg 在提取时完成等比例缩放限制，无需再在 Rust 侧软解
fn frame_to_data_url(jpeg_bytes: &[u8]) -> Result<String, String> {
    let b64 = base64::engine::general_purpose::STANDARD.encode(jpeg_bytes);
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
    std::fs::create_dir_all(&temp_dir).map_err(|e| format!("Failed to create temp dir: {}", e))?;

    let fps_str = if fps == 1.0 { "1" } else { "0.5" };
    run_ffmpeg(&[
        "-i",
        path.to_str().ok_or("Invalid video path")?,
        "-vf",
        &format!("fps={},scale='if(gt(iw/ih,1.777778),min(1280,iw),-1)':'if(gt(iw/ih,1.777778),-1,min(720,ih))'", fps_str),
        "-q:v",
        "2",
        temp_dir
            .join("frame_%04d.jpg")
            .to_str()
            .ok_or("Invalid temp path")?,
    ])?;

    // 8. 移除原有的全量预加载逻辑，改为在循环中按需读取以节省内存

    // 9. 为每个需要的时间戳匹配最接近的均匀帧，缺失则单独提取
    // 增加累加器，当 Base64 总大小超过 18MB 时停止，确保请求体在 20MB 以内
    let mut results = Vec::new();
    let mut total_b64_size = 0;
    const SIZE_LIMIT: usize = 18_000_000; // 18MB 字符上限

    for ts in &deduped {
        // ffmpeg frame_%04d 从 1 开始：frame_0001 → t=0, frame_0002 → t=1/fps, ...
        let expected_idx = (ts * fps).round() as usize + 1;
        let frame_path = temp_dir.join(format!("frame_{:04}.jpg", expected_idx));

        let jpeg_bytes = if frame_path.exists() {
            std::fs::read(&frame_path).map_err(|e| format!("Failed to read frame {}: {}", expected_idx, e))?
        } else {
            // 场景帧可能不在均匀采样网格中，单独提取
            extract_single_frame(path, *ts)?
        };

        let data_url = frame_to_data_url(&jpeg_bytes)?;
        total_b64_size += data_url.len();
        
        results.push(data_url);

        if total_b64_size >= SIZE_LIMIT {
            // 触碰 20MB 边界，硬截断后续帧
            break;
        }
    }

    // 10. 清理临时目录
    let _ = std::fs::remove_dir_all(&temp_dir);

    Ok(results)
}

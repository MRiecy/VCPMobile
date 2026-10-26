use super::native_media::{
    check_cancelled, read_cache, read_output_file, run_native_media, write_cache, VIDEO_MAX_BYTES,
};
use base64::Engine as _;
use std::path::Path;
use tauri::{AppHandle, Runtime};
use tokio_util::sync::CancellationToken;

const FRAME_DATA_PREFIX: &str = "data:image/jpeg;base64,";
const VIDEO_CACHE_KIND: &str = "video-jpeg-dedup-v1";

#[derive(serde::Deserialize)]
struct ProcessVideoResult {
    paths: Vec<String>,
}

/// Both native adapters return bounded JPEG files. An unreadable/empty result
/// fails this attachment; frames are never silently dropped or truncated.
pub async fn process_video_for_multimodal<R: Runtime>(
    app: &AppHandle<R>,
    path: &Path,
    cancellation: &CancellationToken,
) -> Result<Vec<String>, String> {
    check_cancelled(cancellation)?;
    if let Some(cached) =
        read_cache::<_, Vec<String>>(app, path, VIDEO_CACHE_KIND, 17 * 1024 * 1024).await
    {
        check_cancelled(cancellation)?;
        if !cached.is_empty()
            && cached.len() <= 300
            && cached.iter().all(|frame| {
                frame.starts_with(FRAME_DATA_PREFIX) && frame.len() > FRAME_DATA_PREFIX.len()
            })
            && cached.iter().map(String::len).sum::<usize>()
                <= (VIDEO_MAX_BYTES as usize).div_ceil(3) * 4 + 300 * (FRAME_DATA_PREFIX.len() + 4)
        {
            return Ok(cached);
        }
    }
    let frames = run_native_media(
        app,
        path,
        "processVideo",
        VIDEO_MAX_BYTES,
        cancellation,
        |result: ProcessVideoResult, output_dir, token| {
            if result.paths.is_empty() || result.paths.len() > 300 {
                return Err("视频没有可用画面或抽帧数量超过 300 帧".into());
            }
            let mut frames = Vec::with_capacity(result.paths.len());
            let mut total_bytes = 0u64;
            let mut seen_paths = std::collections::HashSet::new();
            for path in result.paths {
                check_cancelled(token)?;
                if !seen_paths.insert(path.clone()) {
                    return Err("视频处理返回重复的帧文件".into());
                }
                let bytes = read_output_file(output_dir, Path::new(&path), VIDEO_MAX_BYTES)?;
                total_bytes = total_bytes
                    .checked_add(bytes.len() as u64)
                    .ok_or("视频帧大小溢出")?;
                if total_bytes > VIDEO_MAX_BYTES {
                    return Err("视频画面超过 12 MB 编码预算，请缩短视频后重试".into());
                }
                if infer::get(&bytes).is_none_or(|format| format.mime_type() != "image/jpeg") {
                    return Err("视频处理结果不是 JPEG 画面".into());
                }
                let prefix = FRAME_DATA_PREFIX;
                let mut data_url = String::with_capacity(prefix.len() + bytes.len().div_ceil(3) * 4);
                data_url.push_str(prefix);
                base64::engine::general_purpose::STANDARD.encode_string(&bytes, &mut data_url);
                frames.push(data_url);
            }
            Ok(frames)
        },
    )
    .await?;
    check_cancelled(cancellation)?;
    let frames = write_cache(app, path, VIDEO_CACHE_KIND, frames).await?;
    check_cancelled(cancellation)?;
    Ok(frames)
}

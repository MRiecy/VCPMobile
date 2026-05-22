use base64::Engine as _;
use std::path::Path;

/// 将本地图片转换为多模态 Base64 data URL
/// 统一使用 ffmpeg 处理所有图片，输出为 webp 格式以保留透明度并减小体积
pub fn convert_local_image_for_multimodal(path: &Path) -> Result<String, String> {
    use super::ffmpeg_cli::run_ffmpeg;

    let webp_bytes = run_ffmpeg(&[
        "-i",
        path.to_str().ok_or("Invalid image path")?,
        "-vf",
        "scale='min(1120,iw)':'min(1120,ih)':force_original_aspect_ratio=decrease:flags=lanczos",
        "-c:v",
        "libwebp",
        "-q:v",
        "80",
        "-f",
        "webp",
        "pipe:1",
    ])?;

    let b64 = base64::engine::general_purpose::STANDARD.encode(&webp_bytes);
    Ok(format!("data:image/webp;base64,{}", b64))
}

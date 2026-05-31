use base64::Engine as _;
use std::path::Path;

/// 将本地图片转换为多模态 Base64 data URL
/// 优先使用 ffmpeg 处理所有图片（输出为 webp 格式以保留透明度并减小体积）
/// 如果 ffmpeg 执行失败（例如 Android 10+ 因 SELinux 限制禁止执行临时目录下的二进制），
/// 则无缝自动降级为直接读取原图的二进制字节并转为 Base64，确保可用性
pub fn convert_local_image_for_multimodal(path: &Path) -> Result<String, String> {
    use super::ffmpeg_cli::run_ffmpeg;

    let webp_bytes_res = run_ffmpeg(&[
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
    ]);

    match webp_bytes_res {
        Ok(webp_bytes) => {
            let b64 = base64::engine::general_purpose::STANDARD.encode(&webp_bytes);
            Ok(format!("data:image/webp;base64,{}", b64))
        }
        Err(err) => {
            log::warn!(
                "[ImageExtractor] ffmpeg scale failed: {}. Falling back to direct raw image bytes.",
                err
            );
            
            // 降级策略：直接读取原图文件字节，转为 base64
            let bytes = std::fs::read(path).map_err(|e| format!("Failed to read raw image: {}", e))?;
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("jpeg")
                .to_lowercase();
            
            // 规范化 MIME 类型子类型
            let subtype = match ext.as_str() {
                "png" => "png",
                "webp" => "webp",
                "gif" => "gif",
                "bmp" => "bmp",
                "ico" => "x-icon",
                "svg" => "svg+xml",
                "avif" => "avif",
                "heic" | "heif" => "heic",
                _ => "jpeg", // 兜底使用 jpeg
            };
            
            let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
            Ok(format!("data:image/{};base64,{}", subtype, b64))
        }
    }
}

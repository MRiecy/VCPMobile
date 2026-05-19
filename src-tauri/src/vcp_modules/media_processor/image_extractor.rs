use base64::Engine as _;
use image::codecs::jpeg::JpegEncoder;
use image::{ExtendedColorType, GenericImageView, ImageEncoder};
use std::path::Path;

const LARGE_IMAGE_THRESHOLD_BYTES: u64 = 2 * 1024 * 1024; // 2MB

/// 将本地图片转换为多模态 Base64 data URL
/// ≤2MB：image crate（避免进程开销）
/// >2MB：ffmpeg（SIMD 加速，大图更快）
pub fn convert_local_image_for_multimodal(path: &Path) -> Result<String, String> {
    let metadata =
        std::fs::metadata(path).map_err(|e| format!("Failed to read image metadata: {}", e))?;

    if metadata.len() > LARGE_IMAGE_THRESHOLD_BYTES {
        process_large_image_with_ffmpeg(path)
    } else {
        process_small_image_with_image_crate(path)
    }
}

/// 小图路径：纯 Rust，无进程开销
fn process_small_image_with_image_crate(path: &Path) -> Result<String, String> {
    let img = image::open(path).map_err(|e| format!("Failed to open image: {}", e))?;
    let (w, h) = img.dimensions();
    let max_dim = w.max(h);

    if max_dim > 1120 {
        let (new_w, new_h) = if w >= h {
            (1120u32, (1120f32 * h as f32 / w as f32).round() as u32)
        } else {
            ((1120f32 * w as f32 / h as f32).round() as u32, 1120u32)
        };

        // 强制转换为 RGB8，因为 JPEG 不支持 Alpha 通道 (RGBA8)
        // 这也解决了 "The encoder or decoder for Jpeg does not support the color type Rgba8" 错误
        let rgb_img = img.to_rgb8();
        let resized =
            image::imageops::resize(&rgb_img, new_w, new_h, image::imageops::FilterType::Lanczos3);

        let mut buf = Vec::new();
        let encoder = JpegEncoder::new_with_quality(&mut buf, 85);
        encoder
            .write_image(
                resized.as_raw(),
                resized.width(),
                resized.height(),
                ExtendedColorType::Rgb8,
            )
            .map_err(|e| format!("JPEG encode failed: {}", e))?;

        let b64 = base64::engine::general_purpose::STANDARD.encode(&buf);
        Ok(format!("data:image/jpeg;base64,{}", b64))
    } else {
        let bytes = std::fs::read(path).map_err(|e| format!("Failed to read file: {}", e))?;
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        let mime = match ext.as_str() {
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "webp" => "image/webp",
            "gif" => "image/gif",
            _ => "image/jpeg",
        };
        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        Ok(format!("data:{};base64,{}", mime, b64))
    }
}

/// 大图路径：ffmpeg SIMD 加速
fn process_large_image_with_ffmpeg(path: &Path) -> Result<String, String> {
    use super::ffmpeg_cli::run_ffmpeg;

    let jpeg_bytes = run_ffmpeg(&[
        "-i",
        path.to_str().ok_or("Invalid image path")?,
        "-vf",
        "scale='min(1120,iw)':'min(1120,ih)':force_original_aspect_ratio=decrease:flags=lanczos",
        "-q:v",
        "2",
        "-f",
        "image2pipe",
        "-vcodec",
        "mjpeg",
        "pipe:1",
    ])?;

    let b64 = base64::engine::general_purpose::STANDARD.encode(&jpeg_bytes);
    Ok(format!("data:image/jpeg;base64,{}", b64))
}

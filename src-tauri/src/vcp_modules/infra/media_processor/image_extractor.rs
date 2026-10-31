use super::native_media::{read_cache_blocking, write_cache_blocking};
use base64::Engine as _;
use std::path::Path;
use tauri::{AppHandle, Runtime};

pub const MODEL_IMAGE_MAX_BYTES: u64 = 4 * 1024 * 1024;
pub const MODEL_REQUEST_IMAGE_DATA_MAX_BYTES: usize = 16 * 1024 * 1024;
const MODEL_IMAGE_MAX_DIMENSION: u32 = 2048;
const MODEL_IMAGE_MAX_PIXELS: u64 = 4 * 1024 * 1024;
const IMAGE_QUALITY: f64 = 0.82;
const IMAGE_PRESERVE_ALPHA: bool = true;
const MODEL_IMAGE_CACHE_KIND: &str = "model-image";
const MODEL_IMAGE_CACHE_MAX_BYTES: u64 = MODEL_IMAGE_MAX_BYTES.div_ceil(3) * 4 + 1024;

#[derive(serde::Serialize, serde::Deserialize, PartialEq)]
struct ModelImageProfile {
    version: u8,
    max_dimension: u32,
    max_pixels: u64,
    max_bytes: u64,
    quality: f64,
    preserve_alpha: bool,
}

fn model_image_profile() -> ModelImageProfile {
    ModelImageProfile {
        // v1 derives JPEG or PNG according to the native alpha policy.
        version: 1,
        max_dimension: MODEL_IMAGE_MAX_DIMENSION,
        max_pixels: MODEL_IMAGE_MAX_PIXELS,
        max_bytes: MODEL_IMAGE_MAX_BYTES,
        quality: IMAGE_QUALITY,
        preserve_alpha: IMAGE_PRESERVE_ALPHA,
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct CachedModelImage {
    profile: ModelImageProfile,
    data_url: String,
}

fn cached_image_is_valid(cached: &CachedModelImage) -> bool {
    if cached.profile != model_image_profile() {
        return false;
    }
    let Some(encoded) = cached
        .data_url
        .strip_prefix("data:image/jpeg;base64,")
        .or_else(|| cached.data_url.strip_prefix("data:image/png;base64,"))
    else {
        return false;
    };
    if encoded.is_empty() || encoded.len() % 4 != 0 {
        return false;
    }
    let padding = encoded
        .bytes()
        .rev()
        .take_while(|byte| *byte == b'=')
        .count();
    padding <= 2
        && encoded[..encoded.len() - padding]
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/'))
        && encoded.len() as u64 / 4 * 3 - padding as u64 <= MODEL_IMAGE_MAX_BYTES
}

fn read_model_image_cache<R: Runtime>(app: &AppHandle<R>, path: &Path) -> Option<String> {
    let cached: CachedModelImage = read_cache_blocking(
        app,
        path,
        MODEL_IMAGE_CACHE_KIND,
        MODEL_IMAGE_CACHE_MAX_BYTES,
    )?;
    cached_image_is_valid(&cached).then_some(cached.data_url)
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct DerivedImage {
    pub path: String,
    pub mime: String,
    pub size: u64,
    pub width: u32,
    pub height: u32,
}

/// Shared by thumbnail, viewer, avatar and model consumers. One decoder is active at a time.
pub fn derive_image<R: Runtime>(
    app: &AppHandle<R>,
    source: &Path,
    output_stem: &Path,
    max_dimension: u32,
    max_pixels: u64,
    max_bytes: u64,
) -> Result<DerivedImage, String> {
    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        static IMAGE_QUEUE: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = IMAGE_QUEUE
            .lock()
            .map_err(|_| "图像处理队列不可用".to_string())?;
        let result = tauri_plugin_vcp_mobile::system::derive_image(
            app,
            &source.to_string_lossy(),
            &output_stem.to_string_lossy(),
            max_dimension,
            max_pixels,
            max_bytes,
            IMAGE_QUALITY,
            IMAGE_PRESERVE_ALPHA,
        )?;
        let path = std::path::PathBuf::from(&result.path);
        if path == source
            || (path != output_stem.with_extension("jpg")
                && path != output_stem.with_extension("png"))
        {
            return Err("原生派生返回了不属于本次输出的路径".into());
        }
        let validated = (|| {
            let metadata = std::fs::metadata(&path).map_err(|e| e.to_string())?;
            let actual = infer::get_from_path(&path)
                .map_err(|e| e.to_string())?
                .ok_or("无法识别派生图像编码")?;
            if !metadata.is_file()
                || metadata.len() != result.size
                || result.size > max_bytes
                || result.width == 0
                || result.height == 0
                || result.width.max(result.height) > max_dimension
                || u64::from(result.width) * u64::from(result.height) > max_pixels
                || actual.mime_type() != result.mime
                || !matches!(result.mime.as_str(), "image/jpeg" | "image/png")
            {
                return Err("派生图像的编码、像素或字节预算不匹配".to_string());
            }
            Ok(())
        })();
        if let Err(error) = validated {
            let _ = std::fs::remove_file(&path);
            return Err(error);
        }
        Ok(DerivedImage {
            path: result.path,
            mime: result.mime,
            size: result.size,
            width: result.width,
            height: result.height,
        })
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        let _ = (
            app,
            source,
            output_stem,
            max_dimension,
            max_pixels,
            max_bytes,
        );
        Err("原生图像派生仅在 Android/iOS 中可用".into())
    }
}

/// 将本地图片转换为多模态 Base64 data URL
/// 两端均经用途派生，失败时保留原件并返回真实错误，不把 HEIC 等原件伪装为 JPEG。
pub fn convert_local_image_for_multimodal<R: Runtime>(
    app: &AppHandle<R>,
    path: &Path,
) -> Result<String, String> {
    use std::io::Read;
    if let Some(cached) = read_model_image_cache(app, path) {
        return Ok(cached);
    }
    // Misses share the existing single-decoder throughput. Keep publication
    // inside this worker's lock so a queued request reuses the completed result.
    static MODEL_CACHE_QUEUE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = MODEL_CACHE_QUEUE
        .lock()
        .map_err(|_| "模型图像缓存队列不可用")?;
    if let Some(cached) = read_model_image_cache(app, path) {
        return Ok(cached);
    }
    let cache = crate::vcp_modules::file_manager::get_cache_root_dir(app)?.join("image-work");
    std::fs::create_dir_all(&cache).map_err(|e| e.to_string())?;
    let image = derive_image(
        app,
        path,
        &cache.join(uuid::Uuid::new_v4().to_string()),
        MODEL_IMAGE_MAX_DIMENSION,
        MODEL_IMAGE_MAX_PIXELS,
        MODEL_IMAGE_MAX_BYTES,
    )?;
    let read_result = (|| {
        let file = std::fs::File::open(&image.path).map_err(|e| e.to_string())?;
        let mut bytes = Vec::with_capacity(image.size as usize);
        file.take(MODEL_IMAGE_MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() as u64 != image.size || bytes.len() as u64 > MODEL_IMAGE_MAX_BYTES {
            return Err("模型图片大小发生变化".to_string());
        }
        let prefix = format!("data:{};base64,", image.mime);
        let mut data_url = String::with_capacity(prefix.len() + bytes.len().div_ceil(3) * 4);
        data_url.push_str(&prefix);
        base64::engine::general_purpose::STANDARD.encode_string(&bytes, &mut data_url);
        Ok(data_url)
    })();
    let _ = std::fs::remove_file(&image.path);
    let data_url = read_result?;
    Ok(write_cache_blocking(
        app,
        path,
        MODEL_IMAGE_CACHE_KIND,
        CachedModelImage {
            profile: model_image_profile(),
            data_url,
        },
    )
    .data_url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_cache_rejects_changed_derivation_profile() {
        let mut cached = CachedModelImage {
            profile: model_image_profile(),
            data_url: "data:image/png;base64,AQID".into(),
        };
        assert!(cached_image_is_valid(&cached));
        cached.profile.quality = 0.9;
        assert!(!cached_image_is_valid(&cached));
        cached.profile = model_image_profile();
        cached.profile.version += 1;
        assert!(!cached_image_is_valid(&cached));
    }

    #[test]
    fn image_cache_enforces_original_byte_limit_even_with_equal_base64_length() {
        let engine = &base64::engine::general_purpose::STANDARD;
        let byte_limit = MODEL_IMAGE_MAX_BYTES as usize;
        let encoded_at_limit = engine.encode(vec![0; byte_limit]);
        let encoded_over_limit = engine.encode(vec![0; byte_limit + 1]);
        assert_eq!(encoded_at_limit.len(), encoded_over_limit.len());
        let mut cached = CachedModelImage {
            profile: model_image_profile(),
            data_url: format!("data:image/jpeg;base64,{encoded_at_limit}"),
        };
        assert!(cached_image_is_valid(&cached));
        cached.data_url = format!("data:image/jpeg;base64,{encoded_over_limit}");
        assert!(!cached_image_is_valid(&cached));
        cached.data_url = "data:image/jpeg;base64,!!!!".into();
        assert!(!cached_image_is_valid(&cached));
    }
}

use base64::Engine as _;
use std::path::Path;
use tauri::{AppHandle, Runtime};

pub const MODEL_IMAGE_MAX_BYTES: u64 = 4 * 1024 * 1024;
pub const MODEL_REQUEST_IMAGE_DATA_MAX_BYTES: usize = 16 * 1024 * 1024;

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
            0.82,
            true,
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
        return Ok(DerivedImage {
            path: result.path,
            mime: result.mime,
            size: result.size,
            width: result.width,
            height: result.height,
        });
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
    let cache = crate::vcp_modules::file_manager::get_cache_root_dir(app)?.join("image-work");
    std::fs::create_dir_all(&cache).map_err(|e| e.to_string())?;
    let image = derive_image(
        app,
        path,
        &cache.join(uuid::Uuid::new_v4().to_string()),
        2048,
        4 * 1024 * 1024,
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
        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        Ok(format!("data:{};base64,{}", image.mime, b64))
    })();
    let _ = std::fs::remove_file(&image.path);
    read_result
}

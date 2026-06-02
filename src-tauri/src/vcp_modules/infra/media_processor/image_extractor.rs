use base64::Engine as _;
use std::path::Path;
use tauri::{AppHandle, Runtime};

/// 将本地图片转换为多模态 Base64 data URL
/// 优先使用 Android Kotlin 原生硬件/高保真缩放（Android）
/// 如果在 Android 平台压制失败或在非 Android 平台，
/// 当且仅当文件大小 < 5MB 时允许直接无处理读取原图字节转 Base64 传输，超过 5MB 则禁止回退直接报错。
pub fn convert_local_image_for_multimodal<R: Runtime>(
    app: &AppHandle<R>,
    path: &Path,
) -> Result<String, String> {
    let process_err: Option<String>;

    #[cfg(target_os = "android")]
    {
        use tauri::Manager;
        use tauri_plugin_vcp_mobile::VcpMobileState;

        let state = app.state::<VcpMobileState<R>>();
        match (|| -> Result<String, String> {
            let handle = state.plugin_handle.lock().map_err(|e| e.to_string())?;
            let plugin_handle = handle.as_ref().ok_or("VCP Mobile Plugin handle not initialized")?;

            #[derive(serde::Deserialize)]
            struct ProcessImageResult {
                path: String,
            }

            let input_str = path.to_str().ok_or("Invalid image path")?;
            log::info!("[ImageExtractor] Invoking Kotlin processImage for: {}", input_str);

            let res = plugin_handle
                .run_mobile_plugin::<ProcessImageResult>(
                    "processImage",
                    serde_json::json!({ "path": input_str }),
                )
                .map_err(|e| format!("Kotlin processImage failed: {}", e))?;

            log::info!("[ImageExtractor] Kotlin processImage success, output: {}", res.path);
            let output_path = Path::new(&res.path);

            let webp_bytes = std::fs::read(output_path)
                .map_err(|e| format!("Failed to read processed image: {}", e))?;

            let _ = std::fs::remove_file(output_path);

            let b64 = base64::engine::general_purpose::STANDARD.encode(&webp_bytes);
            Ok(format!("data:image/webp;base64,{}", b64))
        })() {
            Ok(data_url) => return Ok(data_url),
            Err(e) => {
                log::warn!("[ImageExtractor] Native processing failed: {}. Falling back if < 5MB.", e);
                process_err = Some(e);
            }
        }
    }

    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        process_err = Some("非 Android 物理端不支持原生硬件缩放".to_string());
    }

    // 共享的严格大小受限降级兜底逻辑
    let file_size = std::fs::metadata(path)
        .map(|m| m.len())
        .map_err(|e| format!("Failed to read raw image metadata: {}", e))?;

    const FIVE_MB: u64 = 5_000_000;
    if file_size >= FIVE_MB {
        return Err(format!(
            "Image processing failed ({:?}), and raw size ({} bytes) >= 5MB. Direct fallback prohibited.",
            process_err, file_size
        ));
    }

    log::info!(
        "[ImageExtractor] Processing failed ({:?}), falling back to direct byte read for small image: {} bytes",
        process_err, file_size
    );

    let bytes = std::fs::read(path).map_err(|e| format!("Failed to read raw image bytes: {}", e))?;
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

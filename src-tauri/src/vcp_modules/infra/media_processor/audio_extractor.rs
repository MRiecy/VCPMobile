use base64::Engine as _;
use std::path::Path;
use tauri::{AppHandle, Runtime};

/// 处理音频：优先在 Android 侧提取为 16kHz 单声道 (32kbps) AAC，返回 base64 data URL
/// 如果在 Android 平台转码失败或在非 Android 平台，
/// 当且仅当文件大小 < 5MB 时允许直接无处理读取原音频字节转 Base64 传输，超过 5MB 则禁止回退直接报错。
pub fn process_audio_for_multimodal<R: Runtime>(
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
            struct ProcessAudioResult {
                path: String,
            }

            let input_str = path.to_str().ok_or("Invalid audio path")?;
            log::info!("[AudioExtractor] Invoking Kotlin processAudio for: {}", input_str);

            let res = plugin_handle
                .run_mobile_plugin::<ProcessAudioResult>(
                    "processAudio",
                    serde_json::json!({ "path": input_str }),
                )
                .map_err(|e| format!("Kotlin processAudio failed: {}", e))?;

            log::info!("[AudioExtractor] Kotlin processAudio success, output: {}", res.path);
            let output_path = Path::new(&res.path);

            let aac_bytes = std::fs::read(output_path)
                .map_err(|e| format!("Failed to read processed AAC file: {}", e))?;

            let _ = std::fs::remove_file(output_path);

            let prefix = "data:audio/aac;base64,";
            let b64_len = (aac_bytes.len() * 4).div_ceil(3);
            let mut result = String::with_capacity(prefix.len() + b64_len);
            result.push_str(prefix);
            base64::engine::general_purpose::STANDARD.encode_string(&aac_bytes, &mut result);
            Ok(result)
        })() {
            Ok(data_url) => return Ok(data_url),
            Err(e) => {
                log::warn!("[AudioExtractor] Native processing failed: {}. Falling back if < 5MB.", e);
                process_err = Some(e);
            }
        }
    }

    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        process_err = Some("非 Android 物理端不支持原生硬件音频转码".to_string());
    }

    // 共享的严格大小受限降级兜底逻辑
    let file_size = std::fs::metadata(path)
        .map(|m| m.len())
        .map_err(|e| format!("Failed to read raw audio metadata: {}", e))?;

    const FIVE_MB: u64 = 5_000_000;
    if file_size >= FIVE_MB {
        return Err(format!(
            "Audio transcode failed ({:?}), and raw size ({} bytes) >= 5MB. Direct fallback prohibited.",
            process_err, file_size
        ));
    }

    log::info!(
        "[AudioExtractor] Processing failed ({:?}), falling back to direct byte read for small audio: {} bytes",
        process_err, file_size
    );

    let bytes = std::fs::read(path).map_err(|e| format!("Failed to read raw audio bytes: {}", e))?;
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("mp3")
        .to_lowercase();

    let mime = match ext.as_str() {
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "flac" => "audio/flac",
        "aac" => "audio/aac",
        "m4a" => "audio/mp4",
        "opus" => "audio/opus",
        _ => "audio/mpeg", // 兜底
    };

    let prefix = format!("data:{};base64,", mime);
    let b64_len = (bytes.len() * 4).div_ceil(3);
    let mut result = String::with_capacity(prefix.len() + b64_len);
    result.push_str(&prefix);
    base64::engine::general_purpose::STANDARD.encode_string(&bytes, &mut result);

    Ok(result)
}

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
    let hash = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();

    let cache_dir = crate::vcp_modules::infra::file_manager::get_multimodal_cache_dir(app)?;
    if !cache_dir.exists() {
        let _ = std::fs::create_dir_all(&cache_dir);
    }
    let cache_path = cache_dir.join(format!("{}.json", hash));

    // 1. 检查并命中缓存
    if !hash.is_empty() && cache_path.exists() {
        if let Ok(json_str) = std::fs::read_to_string(&cache_path) {
            if let Ok(cached_data) = serde_json::from_str::<Vec<String>>(&json_str) {
                if let Some(audio_url) = cached_data.first() {
                    log::info!(
                        "[AudioExtractor] Cache HIT for hash: {}, returning audio payload instantly",
                        hash
                    );
                    return Ok(audio_url.clone());
                }
            }
        }
    }

    // 2. 缓存未命中，执行内部提取逻辑
    let result = process_audio_for_multimodal_internal(app, path)?;

    // 3. 写入持久化缓存
    if !hash.is_empty() && !result.is_empty() {
        let wrapper = vec![result.clone()];
        if let Ok(json_str) = serde_json::to_string(&wrapper) {
            if let Err(e) = std::fs::write(&cache_path, json_str) {
                log::warn!("[AudioExtractor] Failed to write cache for {}: {}", hash, e);
            } else {
                log::info!(
                    "[AudioExtractor] Successfully cached audio payload for hash: {}",
                    hash
                );
            }
        }
    }

    Ok(result)
}

fn process_audio_for_multimodal_internal<R: Runtime>(
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
            let plugin_handle = handle
                .as_ref()
                .ok_or("VCP Mobile Plugin handle not initialized")?;

            #[derive(serde::Deserialize)]
            struct ProcessAudioResult {
                path: String,
            }

            let input_str = path.to_str().ok_or("Invalid audio path")?;
            log::info!(
                "[AudioExtractor] Cache MISS. Invoking Kotlin processAudio for: {}",
                input_str
            );

            let res = plugin_handle
                .run_mobile_plugin::<ProcessAudioResult>(
                    "processAudio",
                    serde_json::json!({ "path": input_str }),
                )
                .map_err(|e| format!("Kotlin processAudio failed: {}", e))?;

            log::info!(
                "[AudioExtractor] Kotlin processAudio success, output: {}",
                res.path
            );
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
                log::warn!(
                    "[AudioExtractor] Native processing failed: {}. Falling back if < 5MB.",
                    e
                );
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

    let bytes =
        std::fs::read(path).map_err(|e| format!("Failed to read raw audio bytes: {}", e))?;
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

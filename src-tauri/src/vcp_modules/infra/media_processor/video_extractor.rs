use std::path::Path;
use tauri::{AppHandle, Runtime};

/// 处理视频：返回帧序列（每张帧为 base64 data URL）
/// 优先使用 Android Kotlin 原生多媒体库进行高保真异步抽帧与 JPEG 压缩（Android）
/// 在非 Android 平台直接返回不支持错误。
/// 支持 18MB Base64 数据硬截断限额，并物理清理全部临时生成的 cache 帧文件。
pub fn process_video_for_multimodal<R: Runtime>(
    app: &AppHandle<R>,
    path: &Path,
) -> Result<Vec<String>, String> {
    #[cfg(target_os = "android")]
    {
        use base64::Engine as _;
        use tauri::Manager;
        use tauri_plugin_vcp_mobile::VcpMobileState;

        // 1. 获取内容 Hash (基于 attachments 下的内容寻址文件名)
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

        // 2. 检查并命中缓存
        if !hash.is_empty() && cache_path.exists() {
            if let Ok(json_str) = std::fs::read_to_string(&cache_path) {
                if let Ok(cached_frames) = serde_json::from_str::<Vec<String>>(&json_str) {
                    log::info!(
                        "[VideoExtractor] Cache HIT for hash: {}, returning {} frames instantly",
                        hash,
                        cached_frames.len()
                    );
                    return Ok(cached_frames);
                }
            }
        }

        let state = app.state::<VcpMobileState<R>>();
        let handle = state.plugin_handle.lock().map_err(|e| e.to_string())?;
        let plugin_handle = handle
            .as_ref()
            .ok_or("VCP Mobile Plugin handle not initialized")?;

        #[derive(serde::Deserialize)]
        struct ProcessVideoResult {
            paths: Vec<String>,
        }

        let input_str = path.to_str().ok_or("Invalid video path")?;
        log::info!(
            "[VideoExtractor] Cache MISS. Invoking Kotlin processVideo for: {}",
            input_str
        );

        // 调用 Kotlin 侧的高并发异步视频帧提取与 JPEG 压缩 (1280x720包络, 步长采样, 降采样截断 300)
        let res = plugin_handle
            .run_mobile_plugin::<ProcessVideoResult>(
                "processVideo",
                serde_json::json!({ "path": input_str }),
            )
            .map_err(|e| format!("Kotlin processVideo failed: {}", e))?;

        log::info!(
            "[VideoExtractor] Kotlin processVideo success, extracted {} frame paths",
            res.paths.len()
        );

        let mut results = Vec::new();
        let mut total_b64_size = 0;
        const SIZE_LIMIT: usize = 18_000_000; // 18MB Base64 字符硬限额

        for (idx, frame_path_str) in res.paths.iter().enumerate() {
            let frame_path = Path::new(frame_path_str);
            if frame_path.exists() {
                if total_b64_size < SIZE_LIMIT {
                    match std::fs::read(frame_path) {
                        Ok(jpeg_bytes) => {
                            let b64 = base64::engine::general_purpose::STANDARD.encode(&jpeg_bytes);
                            let data_url = format!("data:image/jpeg;base64,{}", b64);
                            total_b64_size += data_url.len();
                            results.push(data_url);
                        }
                        Err(e) => {
                            log::warn!(
                                "[VideoExtractor] Failed to read frame {}: {}",
                                frame_path_str,
                                e
                            );
                        }
                    }
                } else {
                    log::warn!(
                        "[VideoExtractor] Video multimodal Base64 payload reached 18MB limit at frame {}. Truncating remainder.",
                        idx
                    );
                }
                // 极速物理清理当前文件，防御存储垃圾残留
                let _ = std::fs::remove_file(frame_path);
            }
        }

        // 尝试清理临时父文件夹
        if let Some(first_path_str) = res.paths.first() {
            if let Some(parent_dir) = Path::new(first_path_str).parent() {
                if parent_dir.exists() && parent_dir.is_dir() {
                    let _ = std::fs::remove_dir_all(parent_dir);
                }
            }
        }

        // 3. 写入持久化缓存
        if !hash.is_empty() && !results.is_empty() {
            if let Ok(json_str) = serde_json::to_string(&results) {
                if let Err(e) = std::fs::write(&cache_path, json_str) {
                    log::warn!("[VideoExtractor] Failed to write cache for {}: {}", hash, e);
                } else {
                    log::info!(
                        "[VideoExtractor] Successfully cached {} frames for hash: {}",
                        results.len(),
                        hash
                    );
                    // 🌟 写入缓存后，主动运行一次大小收敛，限制在 300MB，清理至 150MB 🌟
                    crate::vcp_modules::infra::file_manager::evict_multimodal_cache_if_needed(
                        app,
                        300 * 1024 * 1024,
                        150 * 1024 * 1024,
                    );
                }
            }
        }

        return Ok(results);
    }

    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        let _ = path;
        Err("非 Android 物理端不支持视频多模态抽帧处理".to_string())
    }
}

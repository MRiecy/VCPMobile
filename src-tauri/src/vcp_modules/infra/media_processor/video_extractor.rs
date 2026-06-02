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
            "[VideoExtractor] Invoking Kotlin processVideo for: {}",
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

        return Ok(results);
    }

    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        let _ = path;
        Err("非 Android 物理端不支持视频多模态抽帧处理".to_string())
    }
}

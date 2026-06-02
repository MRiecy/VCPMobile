#[cfg(target_os = "android")]
use crate::VcpMobileState;
use serde::{Deserialize, Serialize};
#[cfg(target_os = "android")]
use tauri::Manager;
use tauri::{AppHandle, Runtime};

#[derive(Serialize, Deserialize)]
pub struct PermissionStatus {
    pub notification: bool,
    pub storage: bool,
    pub battery: bool,
    pub microphone: bool,
    pub camera: bool,
    pub overlay: bool,
}

#[tauri::command]
pub fn check_all_permissions<R: Runtime>(app: AppHandle<R>) -> Result<PermissionStatus, String> {
    #[cfg(target_os = "android")]
    {
        let state = app.state::<VcpMobileState<R>>();
        let handle = state.plugin_handle.lock().map_err(|e| e.to_string())?;
        let plugin_handle = handle.as_ref().ok_or("Plugin handle not initialized")?;

        let status = plugin_handle
            .run_mobile_plugin::<PermissionStatus>("checkAllPermissions", serde_json::json!({}))
            .map_err(|e| format!("run_mobile_plugin failed: {}", e))?;
        Ok(status)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Ok(PermissionStatus {
            notification: true,
            storage: true,
            battery: true,
            microphone: true,
            camera: true,
            overlay: true,
        })
    }
}

#[tauri::command]
pub fn request_android_permission<R: Runtime>(
    app: AppHandle<R>,
    p_type: String,
) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        let state = app.state::<VcpMobileState<R>>();
        let handle = state.plugin_handle.lock().map_err(|e| e.to_string())?;
        let plugin_handle = handle.as_ref().ok_or("Plugin handle not initialized")?;

        plugin_handle
            .run_mobile_plugin::<serde_json::Value>(
                "requestAndroidPermission",
                serde_json::json!({ "type": p_type }),
            )
            .map_err(|e| format!("run_mobile_plugin failed: {}", e))?;
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        let _ = p_type;
    }
    Ok(())
}

#[tauri::command]
pub fn move_task_to_back<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        let state = app.state::<VcpMobileState<R>>();
        let handle = state.plugin_handle.lock().map_err(|e| e.to_string())?;
        let plugin_handle = handle.as_ref().ok_or("Plugin handle not initialized")?;

        plugin_handle
            .run_mobile_plugin::<serde_json::Value>("moveTaskToBack", serde_json::json!({}))
            .map_err(|e| format!("run_mobile_plugin failed: {}", e))?;
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PickedFileInfo {
    pub path: String,
    pub name: String,
    pub mime: String,
    pub size: u64,
    pub hash: String,
    pub thumbnail_path: Option<String>,
}

#[tauri::command]
pub fn pick_file<R: Runtime>(
    app: AppHandle<R>,
    mode: Option<String>,
) -> Result<PickedFileInfo, String> {
    #[cfg(target_os = "android")]
    {
        let state = app.state::<VcpMobileState<R>>();
        let handle = state.plugin_handle.lock().map_err(|e| e.to_string())?;
        let plugin_handle = handle.as_ref().ok_or("Plugin handle not initialized")?;

        let file_info = plugin_handle
            .run_mobile_plugin::<PickedFileInfo>(
                "pickFile",
                serde_json::json!({
                    "mode": mode.unwrap_or_else(|| "file".to_string())
                }),
            )
            .map_err(|e| format!("run_mobile_plugin failed: {}", e))?;
        Ok(file_info)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        let _ = mode;
        Err("该接口仅在 Android 物理端可用".to_string())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatteryStatus {
    pub level: i32,
    pub is_power_save_mode: bool,
}

#[tauri::command]
pub fn get_battery_status<R: Runtime>(app: AppHandle<R>) -> Result<BatteryStatus, String> {
    #[cfg(target_os = "android")]
    {
        let state = app.state::<VcpMobileState<R>>();
        let handle = state.plugin_handle.lock().map_err(|e| e.to_string())?;
        let plugin_handle = handle.as_ref().ok_or("Plugin handle not initialized")?;

        let status = plugin_handle
            .run_mobile_plugin::<BatteryStatus>("getBatteryStatus", serde_json::json!({}))
            .map_err(|e| format!("run_mobile_plugin failed: {}", e))?;
        Ok(status)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Ok(BatteryStatus {
            level: 100,
            is_power_save_mode: false,
        })
    }
}

#[tauri::command]
pub fn open_file_native<R: Runtime>(app: AppHandle<R>, path: String) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        let state = app.state::<VcpMobileState<R>>();
        let handle = state.plugin_handle.lock().map_err(|e| e.to_string())?;
        let plugin_handle = handle.as_ref().ok_or("Plugin handle not initialized")?;

        plugin_handle
            .run_mobile_plugin::<serde_json::Value>("openFile", serde_json::json!({ "path": path }))
            .map_err(|e| format!("run_mobile_plugin failed: {}", e))?;
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        let _ = path;
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowSnapshot {
    pub data_url: String,
    pub width: i32,
    pub height: i32,
}

#[tauri::command]
pub fn capture_window_snapshot<R: Runtime>(
    app: AppHandle<R>,
    max_width: Option<i32>,
    quality: Option<i32>,
) -> Result<WindowSnapshot, String> {
    #[cfg(target_os = "android")]
    {
        let state = app.state::<VcpMobileState<R>>();
        let handle = state.plugin_handle.lock().map_err(|e| e.to_string())?;
        let plugin_handle = handle.as_ref().ok_or("Plugin handle not initialized")?;
        let max_width = max_width.unwrap_or(200).clamp(160, 420);
        let quality = quality.unwrap_or(64).clamp(45, 85);

        let snapshot = plugin_handle
            .run_mobile_plugin::<WindowSnapshot>(
                "captureWindowSnapshot",
                serde_json::json!({ "maxWidth": max_width, "quality": quality }),
            )
            .map_err(|e| format!("run_mobile_plugin failed: {}", e))?;
        Ok(snapshot)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        let _ = max_width;
        let _ = quality;
        Err("该接口仅在 Android 物理端可用".to_string())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GallerySaveResult {
    pub uri: String,
    pub display_name: String,
    pub mime_type: String,
    pub size: i32,
}

#[tauri::command]
pub fn save_image_to_gallery<R: Runtime>(
    app: AppHandle<R>,
    source_url: String,
    file_name: Option<String>,
) -> Result<GallerySaveResult, String> {
    #[cfg(target_os = "android")]
    {
        let state = app.state::<VcpMobileState<R>>();
        let handle = state.plugin_handle.lock().map_err(|e| e.to_string())?;
        let plugin_handle = handle.as_ref().ok_or("Plugin handle not initialized")?;

        let result = plugin_handle
            .run_mobile_plugin::<GallerySaveResult>(
                "saveImageToGallery",
                serde_json::json!({ "sourceUrl": source_url, "fileName": file_name }),
            )
            .map_err(|e| format!("run_mobile_plugin failed: {}", e))?;
        Ok(result)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        let _ = source_url;
        let _ = file_name;
        Err("该接口仅在 Android 物理端可用".to_string())
    }
}

#[tauri::command]
pub fn save_image_from_path<R: Runtime>(
    app: AppHandle<R>,
    image_path: String,
    file_name: Option<String>,
) -> Result<GallerySaveResult, String> {
    #[cfg(target_os = "android")]
    {
        let state = app.state::<VcpMobileState<R>>();
        let handle = state.plugin_handle.lock().map_err(|e| e.to_string())?;
        let plugin_handle = handle.as_ref().ok_or("Plugin handle not initialized")?;

        let result = plugin_handle
            .run_mobile_plugin::<GallerySaveResult>(
                "saveImageFromPath",
                serde_json::json!({ "imagePath": image_path, "fileName": file_name }),
            )
            .map_err(|e| format!("run_mobile_plugin failed: {}", e))?;
        Ok(result)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        let _ = image_path;
        let _ = file_name;
        Err("该接口仅在 Android 物理端可用".to_string())
    }
}

#[tauri::command]
pub fn write_temp_file<R: Runtime>(
    app: AppHandle<R>,
    bytes: Vec<u8>,
    file_name: String,
) -> Result<String, String> {
    #[cfg(target_os = "android")]
    {
        use tauri::Manager;
        let cache_dir = app.path().cache_dir().map_err(|e| e.to_string())?;
        let temp_path = cache_dir.join(&file_name);
        std::fs::write(&temp_path, bytes).map_err(|e| e.to_string())?;
        Ok(temp_path.to_string_lossy().to_string())
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        let _ = bytes;
        let _ = file_name;
        Err("该接口仅在 Android 物理端可用".to_string())
    }
}

#[tauri::command]
pub fn start_download_notification<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        let state = app.state::<VcpMobileState<R>>();
        let handle = state.plugin_handle.lock().map_err(|e| e.to_string())?;
        let plugin_handle = handle.as_ref().ok_or("Plugin handle not initialized")?;

        plugin_handle
            .run_mobile_plugin::<serde_json::Value>(
                "startDownloadNotification",
                serde_json::json!({}),
            )
            .map_err(|e| format!("run_mobile_plugin failed: {}", e))?;
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
    }
    Ok(())
}

#[tauri::command]
pub fn update_download_notification<R: Runtime>(
    app: AppHandle<R>,
    progress: i32,
    text: Option<String>,
) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        let state = app.state::<VcpMobileState<R>>();
        let handle = state.plugin_handle.lock().map_err(|e| e.to_string())?;
        let plugin_handle = handle.as_ref().ok_or("Plugin handle not initialized")?;

        plugin_handle
            .run_mobile_plugin::<serde_json::Value>(
                "updateDownloadNotification",
                serde_json::json!({ "progress": progress, "text": text }),
            )
            .map_err(|e| format!("run_mobile_plugin failed: {}", e))?;
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        let _ = progress;
        let _ = text;
    }
    Ok(())
}

#[tauri::command]
pub fn cancel_download_notification<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        let state = app.state::<VcpMobileState<R>>();
        let handle = state.plugin_handle.lock().map_err(|e| e.to_string())?;
        let plugin_handle = handle.as_ref().ok_or("Plugin handle not initialized")?;

        plugin_handle
            .run_mobile_plugin::<serde_json::Value>(
                "cancelDownloadNotification",
                serde_json::json!({}),
            )
            .map_err(|e| format!("run_mobile_plugin failed: {}", e))?;
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
    }
    Ok(())
}

#[tauri::command]
pub fn request_overlay_permission<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        let state = app.state::<VcpMobileState<R>>();
        let handle = state.plugin_handle.lock().map_err(|e| e.to_string())?;
        let plugin_handle = handle.as_ref().ok_or("Plugin handle not initialized")?;

        plugin_handle
            .run_mobile_plugin::<serde_json::Value>(
                "requestOverlayPermission",
                serde_json::json!({}),
            )
            .map_err(|e| format!("run_mobile_plugin failed: {}", e))?;
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedFileItem {
    pub cache_path: String,
    pub mime_type: String,
    pub file_name: String,
}

#[tauri::command]
pub fn register_shared_files<R: Runtime>(
    app: AppHandle<R>,
    files: Vec<SharedFileItem>,
) -> Result<Vec<PickedFileInfo>, String> {
    #[cfg(target_os = "android")]
    {
        let state = app.state::<VcpMobileState<R>>();
        let handle = state.plugin_handle.lock().map_err(|e| e.to_string())?;
        let plugin_handle = handle.as_ref().ok_or("Plugin handle not initialized")?;

        let mut results = Vec::new();
        for file in files {
            let file_info = plugin_handle
                .run_mobile_plugin::<PickedFileInfo>(
                    "processSharedFile",
                    serde_json::json!({
                        "cachePath": file.cache_path,
                        "mimeType": file.mime_type,
                        "fileName": file.file_name,
                    }),
                )
                .map_err(|e| format!("run_mobile_plugin processSharedFile failed: {}", e))?;
            results.push(file_info);
        }
        Ok(results)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        let _ = files;
        Err("该接口仅在 Android 物理端可用".to_string())
    }
}

#[tauri::command]
pub fn toggle_floating_ball<R: Runtime>(app: AppHandle<R>, show: bool) -> Result<bool, String> {
    #[cfg(target_os = "android")]
    {
        let state = app.state::<VcpMobileState<R>>();
        let handle = state.plugin_handle.lock().map_err(|e| e.to_string())?;
        let plugin_handle = handle.as_ref().ok_or("Plugin handle not initialized")?;

        #[derive(Deserialize)]
        struct ToggleResult {
            success: bool,
        }

        let res = plugin_handle
            .run_mobile_plugin::<ToggleResult>(
                "toggleFloatingBall",
                serde_json::json!({ "show": show }),
            )
            .map_err(|e| format!("run_mobile_plugin failed: {}", e))?;
        Ok(res.success)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        let _ = show;
        Ok(false)
    }
}

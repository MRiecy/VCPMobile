#[cfg(target_os = "android")]
use crate::VcpMobileState;
use serde::{Deserialize, Serialize};
#[cfg(target_os = "android")]
use tauri::Manager;
use tauri::{AppHandle, Runtime};

#[cfg(target_os = "android")]
const TEMP_FILE_MAX_BYTES: usize = 20 * 1024 * 1024;
#[cfg(target_os = "android")]
const MAX_SHARED_FILES: usize = 16;

#[cfg(any(target_os = "android", test))]
fn safe_leaf_name(file_name: &str) -> Result<&str, String> {
    let path = std::path::Path::new(file_name);
    if file_name.is_empty()
        || file_name == "."
        || file_name == ".."
        || file_name.contains(['/', '\\'])
        || file_name.len() > 180
        || path.is_absolute()
        || path.components().count() != 1
        || path.file_name().and_then(|name| name.to_str()) != Some(file_name)
    {
        return Err("文件名必须是不含路径分隔符的 basename".to_string());
    }
    Ok(file_name)
}

#[cfg(any(target_os = "android", test))]
fn canonical_file_in(root: &std::path::Path, path: &std::path::Path) -> Result<bool, String> {
    if !path.exists() {
        return Ok(false);
    }
    let canonical_root =
        std::fs::canonicalize(root).map_err(|error| format!("临时目录规范化失败: {}", error))?;
    let canonical_path =
        std::fs::canonicalize(path).map_err(|error| format!("临时文件规范化失败: {}", error))?;
    Ok(canonical_path.starts_with(canonical_root) && canonical_path.is_file())
}

#[derive(Serialize, Deserialize)]
pub struct PermissionStatus {
    pub notification: bool,
    pub storage: bool,
    pub battery: bool,
    pub microphone: bool,
    pub camera: bool,
    pub location: bool,
}

#[tauri::command]
pub fn check_all_permissions<R: Runtime>(app: AppHandle<R>) -> Result<PermissionStatus, String> {
    #[cfg(target_os = "android")]
    {
        let state = app.state::<VcpMobileState<R>>();
        let plugin_handle = state.mobile_plugin_handle()?;

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
            location: true,
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
        let plugin_handle = state.mobile_plugin_handle()?;

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
        let plugin_handle = state.mobile_plugin_handle()?;

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
pub struct ListenerPermissionResponse {
    pub enabled: bool,
}

#[tauri::command]
pub fn check_notification_listener_permission<R: Runtime>(
    app: AppHandle<R>,
) -> Result<ListenerPermissionResponse, String> {
    #[cfg(target_os = "android")]
    {
        let state = app.state::<VcpMobileState<R>>();
        let plugin_handle = state.mobile_plugin_handle()?;

        let res = plugin_handle
            .run_mobile_plugin::<ListenerPermissionResponse>(
                "check_notification_listener_permission",
                serde_json::json!({}),
            )
            .map_err(|e| format!("run_mobile_plugin failed: {}", e))?;
        Ok(res)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Ok(ListenerPermissionResponse { enabled: true })
    }
}

#[tauri::command]
pub fn request_notification_listener_permission<R: Runtime>(
    app: AppHandle<R>,
) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        let state = app.state::<VcpMobileState<R>>();
        let plugin_handle = state.mobile_plugin_handle()?;

        plugin_handle
            .run_mobile_plugin::<serde_json::Value>(
                "request_notification_listener_permission",
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
pub fn request_auto_start_permission<R: Runtime>(app: AppHandle<R>) -> Result<bool, String> {
    #[cfg(target_os = "android")]
    {
        let state = app.state::<VcpMobileState<R>>();
        let plugin_handle = state.mobile_plugin_handle()?;

        let res = plugin_handle
            .run_mobile_plugin::<serde_json::Value>(
                "requestAutoStartPermission",
                serde_json::json!({}),
            )
            .map_err(|e| format!("run_mobile_plugin failed: {}", e))?;

        let success = res
            .get("success")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        Ok(success)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Ok(true)
    }
}

#[tauri::command]
pub fn request_power_management_permission<R: Runtime>(app: AppHandle<R>) -> Result<bool, String> {
    #[cfg(target_os = "android")]
    {
        let state = app.state::<VcpMobileState<R>>();
        let plugin_handle = state.mobile_plugin_handle()?;

        let res = plugin_handle
            .run_mobile_plugin::<serde_json::Value>(
                "requestPowerManagementPermission",
                serde_json::json!({}),
            )
            .map_err(|e| format!("run_mobile_plugin failed: {}", e))?;

        let success = res
            .get("success")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        Ok(success)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Ok(true)
    }
}

#[tauri::command]
pub fn check_auto_start_permission<R: Runtime>(app: AppHandle<R>) -> Result<String, String> {
    #[cfg(target_os = "android")]
    {
        let state = app.state::<VcpMobileState<R>>();
        let plugin_handle = state.mobile_plugin_handle()?;

        let res = plugin_handle
            .run_mobile_plugin::<serde_json::Value>(
                "checkAutoStartPermission",
                serde_json::json!({}),
            )
            .map_err(|e| format!("run_mobile_plugin failed: {}", e))?;

        let status = res
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("unsupported")
            .to_string();
        Ok(status)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Ok("unsupported".to_string())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskSpaceInfo {
    pub free_bytes: u64,
    pub free_gb: f64,
    pub total_bytes: u64,
    pub total_gb: f64,
}

#[tauri::command]
pub fn get_free_disk_space<R: Runtime>(app: AppHandle<R>) -> Result<DiskSpaceInfo, String> {
    #[cfg(target_os = "android")]
    {
        let state = app.state::<VcpMobileState<R>>();
        let plugin_handle = state.mobile_plugin_handle()?;

        let info = plugin_handle
            .run_mobile_plugin::<DiskSpaceInfo>("getFreeDiskSpace", serde_json::json!({}))
            .map_err(|e| format!("run_mobile_plugin failed: {}", e))?;
        Ok(info)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Ok(DiskSpaceInfo {
            free_bytes: 10 * 1024 * 1024 * 1024,
            free_gb: 10.0,
            total_bytes: 100 * 1024 * 1024 * 1024,
            total_gb: 100.0,
        })
    }
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
        let plugin_handle = state.mobile_plugin_handle()?;

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
    pub status: Option<String>,
    pub temperature: Option<f64>,
}

#[tauri::command]
pub fn get_battery_status<R: Runtime>(app: AppHandle<R>) -> Result<BatteryStatus, String> {
    #[cfg(target_os = "android")]
    {
        let state = app.state::<VcpMobileState<R>>();
        let plugin_handle = state.mobile_plugin_handle()?;

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
            status: Some("未充电".to_string()),
            temperature: Some(25.0),
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkStatus {
    pub connected: bool,
    pub r#type: String,
    pub down_speed_kbps: i32,
    pub up_speed_kbps: i32,
    pub ip: String,
}

#[tauri::command]
pub fn get_network_status<R: Runtime>(app: AppHandle<R>) -> Result<NetworkStatus, String> {
    #[cfg(target_os = "android")]
    {
        let state = app.state::<VcpMobileState<R>>();
        let plugin_handle = state.mobile_plugin_handle()?;

        let status = plugin_handle
            .run_mobile_plugin::<NetworkStatus>("getNetworkStatus", serde_json::json!({}))
            .map_err(|e| format!("run_mobile_plugin failed: {}", e))?;
        Ok(status)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Ok(NetworkStatus {
            connected: true,
            r#type: "以太网".to_string(),
            down_speed_kbps: 100000,
            up_speed_kbps: 100000,
            ip: "127.0.0.1".to_string(),
        })
    }
}

#[tauri::command]
pub fn open_file_native<R: Runtime>(app: AppHandle<R>, path: String) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        let state = app.state::<VcpMobileState<R>>();
        let plugin_handle = state.mobile_plugin_handle()?;

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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApkSignatureVerification {
    pub apk_sha256: Option<String>,
    pub self_sha256: Option<String>,
    pub matched: bool,
}

/// OTA 安装前证书连续性校验：比对未安装 APK 与当前应用的签名证书 SHA-256。
#[tauri::command]
pub fn verify_apk_signature<R: Runtime>(
    app: AppHandle<R>,
    path: String,
) -> Result<ApkSignatureVerification, String> {
    #[cfg(target_os = "android")]
    {
        let state = app.state::<VcpMobileState<R>>();
        let plugin_handle = state.mobile_plugin_handle()?;

        return plugin_handle
            .run_mobile_plugin::<ApkSignatureVerification>(
                "verifyApkSignature",
                serde_json::json!({ "path": path }),
            )
            .map_err(|e| format!("run_mobile_plugin failed: {}", e));
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        let _ = path;
        Err("签名校验仅在 Android 上可用".to_string())
    }
}

/// 是否已授予"安装未知应用"权限。
#[tauri::command]
pub fn can_install_packages<R: Runtime>(app: AppHandle<R>) -> Result<bool, String> {
    #[cfg(target_os = "android")]
    {
        let state = app.state::<VcpMobileState<R>>();
        let plugin_handle = state.mobile_plugin_handle()?;

        let result = plugin_handle
            .run_mobile_plugin::<serde_json::Value>("canInstallPackages", serde_json::json!({}))
            .map_err(|e| format!("run_mobile_plugin failed: {}", e))?;
        return Ok(result
            .get("allowed")
            .and_then(|v| v.as_bool())
            .unwrap_or(false));
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Ok(true)
    }
}

/// 跳转系统"安装未知应用"授权页。
#[tauri::command]
pub fn open_unknown_sources_settings<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        let state = app.state::<VcpMobileState<R>>();
        let plugin_handle = state.mobile_plugin_handle()?;

        plugin_handle
            .run_mobile_plugin::<serde_json::Value>(
                "openUnknownSourcesSettings",
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

/// OTA 下载期间持有前台锁，防止切后台后进程被杀导致下载中断。
#[tauri::command]
pub fn acquire_ota_keepalive<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        let state = app.state::<VcpMobileState<R>>();
        let plugin_handle = state.mobile_plugin_handle()?;

        plugin_handle
            .run_mobile_plugin::<serde_json::Value>("acquireOtaKeepalive", serde_json::json!({}))
            .map_err(|e| format!("run_mobile_plugin failed: {}", e))?;
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
    }
    Ok(())
}

#[tauri::command]
pub fn release_ota_keepalive<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        let state = app.state::<VcpMobileState<R>>();
        let plugin_handle = state.mobile_plugin_handle()?;

        plugin_handle
            .run_mobile_plugin::<serde_json::Value>("releaseOtaKeepalive", serde_json::json!({}))
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
        let plugin_handle = state.mobile_plugin_handle()?;
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
        let plugin_handle = state.mobile_plugin_handle()?;

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
        let plugin_handle = state.mobile_plugin_handle()?;

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

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadsSaveResult {
    pub uri: String,
    pub display_name: String,
    pub mime_type: String,
    pub size: i32,
}

#[tauri::command]
pub fn save_to_downloads<R: Runtime>(
    app: AppHandle<R>,
    file_name: String,
    content_base64: String,
    mime_type: Option<String>,
) -> Result<DownloadsSaveResult, String> {
    #[cfg(target_os = "android")]
    {
        let state = app.state::<VcpMobileState<R>>();
        let plugin_handle = state.mobile_plugin_handle()?;

        let result = plugin_handle
            .run_mobile_plugin::<DownloadsSaveResult>(
                "saveToDownloads",
                serde_json::json!({
                    "fileName": file_name,
                    "contentBase64": content_base64,
                    "mimeType": mime_type,
                }),
            )
            .map_err(|e| format!("run_mobile_plugin failed: {}", e))?;
        Ok(result)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        let _ = file_name;
        let _ = content_base64;
        let _ = mime_type;
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
        if bytes.len() > TEMP_FILE_MAX_BYTES {
            return Err("临时文件过大 (Limit: 20MB)".to_string());
        }
        let file_name = safe_leaf_name(&file_name)?;
        let cache_dir = app.path().cache_dir().map_err(|e| e.to_string())?;
        let temp_dir = cache_dir.join("vcp_bridge_temp");
        std::fs::create_dir_all(&temp_dir).map_err(|e| e.to_string())?;
        let unique = format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|e| e.to_string())?
                .as_nanos()
        );
        let temp_path = temp_dir.join(format!("{}-{}", unique, file_name));
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
pub fn delete_temp_file<R: Runtime>(app: AppHandle<R>, file_path: String) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        use std::path::Path;
        let path = Path::new(&file_path);

        use tauri::Manager;
        let cache_dir = app.path().cache_dir().map_err(|e| e.to_string())?;
        let temp_dir = cache_dir.join("vcp_bridge_temp");
        std::fs::create_dir_all(&temp_dir).map_err(|e| e.to_string())?;
        if !path.exists() {
            return Ok(());
        }
        if !path.is_absolute() || !canonical_file_in(&temp_dir, path)? {
            return Err("拒绝删除非 bridge staging 文件".to_string());
        }
        std::fs::remove_file(path).map_err(|e| e.to_string())?;
        Ok(())
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        let _ = file_path;
        Ok(())
    }
}

#[tauri::command]
pub fn start_download_notification<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        let state = app.state::<VcpMobileState<R>>();
        let plugin_handle = state.mobile_plugin_handle()?;

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
        let plugin_handle = state.mobile_plugin_handle()?;

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
        let plugin_handle = state.mobile_plugin_handle()?;

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
        let plugin_handle = state.mobile_plugin_handle()?;

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
    pub staging_ticket: String,
}

#[tauri::command]
pub fn register_shared_files<R: Runtime>(
    app: AppHandle<R>,
    owner_id: String,
    files: Vec<SharedFileItem>,
) -> Result<Vec<PickedFileInfo>, String> {
    #[cfg(target_os = "android")]
    {
        if files.len() > MAX_SHARED_FILES {
            return Err(format!("单次最多处理 {} 个分享文件", MAX_SHARED_FILES));
        }
        if owner_id.is_empty()
            || owner_id.len() > 64
            || !owner_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err("非法的分享 intent owner".to_string());
        }
        let state = app.state::<VcpMobileState<R>>();
        let plugin_handle = state.mobile_plugin_handle()?;

        let mut results: Vec<PickedFileInfo> = Vec::new();
        for file in files {
            if file.staging_ticket.is_empty()
                || file.staging_ticket.len() > 64
                || !file
                    .staging_ticket
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            {
                return Err("非法的分享 staging ticket".to_string());
            }
            let file_info = match plugin_handle.run_mobile_plugin::<PickedFileInfo>(
                "processSharedFile",
                serde_json::json!({
                    "cachePath": file.cache_path,
                    "mimeType": file.mime_type,
                    "fileName": file.file_name,
                    "ownerId": owner_id,
                    "stagingTicket": file.staging_ticket,
                }),
            ) {
                Ok(info) => info,
                Err(error) => {
                    for completed in &results {
                        let _ = std::fs::remove_file(&completed.path);
                        if let Some(thumbnail) = &completed.thumbnail_path {
                            let _ = std::fs::remove_file(thumbnail);
                        }
                    }
                    return Err(format!(
                        "run_mobile_plugin processSharedFile failed: {}",
                        error
                    ));
                }
            };
            results.push(file_info);
        }
        Ok(results)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        let _ = owner_id;
        let _ = files;
        Err("该接口仅在 Android 物理端可用".to_string())
    }
}

#[cfg(test)]
mod file_boundary_tests {
    use super::{canonical_file_in, safe_leaf_name};
    use std::fs;

    #[test]
    fn temp_file_name_is_leaf_only() {
        assert_eq!(safe_leaf_name("preview.png"), Ok("preview.png"));
        assert!(safe_leaf_name("../preview.png").is_err());
        assert!(safe_leaf_name("nested/preview.png").is_err());
        assert!(safe_leaf_name("nested\\preview.png").is_err());
        assert!(safe_leaf_name("/tmp/preview.png").is_err());
    }

    #[test]
    fn temp_delete_gate_uses_canonical_containment() {
        let temp = std::env::temp_dir().join(format!(
            "vcp-mobile-file-boundary-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let root = temp.join("bridge");
        let outside = temp.join("outside");
        fs::create_dir_all(&root).expect("root");
        fs::create_dir_all(&outside).expect("outside");
        let inside_file = root.join("inside.bin");
        let outside_file = outside.join("outside.bin");
        fs::write(&inside_file, b"inside").expect("inside");
        fs::write(&outside_file, b"outside").expect("outside file");

        assert_eq!(canonical_file_in(&root, &inside_file), Ok(true));
        assert_eq!(canonical_file_in(&root, &outside_file), Ok(false));
        let _ = fs::remove_dir_all(temp);
    }
}

#[tauri::command]
pub fn toggle_floating_ball<R: Runtime>(app: AppHandle<R>, show: bool) -> Result<bool, String> {
    #[cfg(target_os = "android")]
    {
        let state = app.state::<VcpMobileState<R>>();
        let plugin_handle = state.mobile_plugin_handle()?;

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

#[tauri::command]
pub fn start_sensor_collection<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        let state = app.state::<VcpMobileState<R>>();
        let plugin_handle = state.mobile_plugin_handle()?;

        plugin_handle
            .run_mobile_plugin::<serde_json::Value>("startSensorCollection", serde_json::json!({}))
            .map_err(|e| format!("run_mobile_plugin failed: {}", e))?;
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
    }
    Ok(())
}

#[tauri::command]
pub fn stop_sensor_collection<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        let state = app.state::<VcpMobileState<R>>();
        let plugin_handle = state.mobile_plugin_handle()?;

        plugin_handle
            .run_mobile_plugin::<serde_json::Value>("stopSensorCollection", serde_json::json!({}))
            .map_err(|e| format!("run_mobile_plugin failed: {}", e))?;
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
    }
    Ok(())
}

#[tauri::command]
pub fn get_sensor_data<R: Runtime>(
    app: AppHandle<R>,
    sensor_type: String,
) -> Result<serde_json::Value, String> {
    #[cfg(target_os = "android")]
    {
        let state = app.state::<VcpMobileState<R>>();
        let plugin_handle = state.mobile_plugin_handle()?;

        let data = plugin_handle
            .run_mobile_plugin::<serde_json::Value>(
                "getSensorData",
                serde_json::json!({ "type": sensor_type }),
            )
            .map_err(|e| format!("run_mobile_plugin failed: {}", e))?;
        Ok(data)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        let dummy = match sensor_type.as_str() {
            "location" => "坐标: 39.9000°N, 116.4000°E | 精度: 15m | 海拔: 50m",
            "motion" => "状态: 静止 | 平均加速度: 9.80m/s² | 峰值: 9.80m/s²",
            "ambient" => "环境光: 150 lux (室内) | 气压: 1013 hPa",
            _ => "{}",
        };
        if sensor_type == "all" {
            Ok(serde_json::json!({
                "location": "坐标: 39.9000°N, 116.4000°E | 精度: 15m | 海拔: 50m",
                "motion": "状态: 静止 | 平均加速度: 9.80m/s² | 峰值: 9.80m/s²",
                "ambient": "环境光: 150 lux (室内) | 气压: 1013 hPa",
            }))
        } else {
            Ok(serde_json::json!({ "value": dummy }))
        }
    }
}

#[tauri::command]
pub fn get_cpu_thermal_status<R: Runtime>(app: AppHandle<R>) -> Result<String, String> {
    #[cfg(target_os = "android")]
    {
        let state = app.state::<VcpMobileState<R>>();
        let plugin_handle = state.mobile_plugin_handle()?;

        #[derive(Deserialize)]
        struct ThermalResponse {
            status: String,
        }
        let res = plugin_handle
            .run_mobile_plugin::<ThermalResponse>("getCpuThermalStatus", serde_json::json!({}))
            .map_err(|e| format!("run_mobile_plugin failed: {}", e))?;
        Ok(res.status)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Ok("正常".to_string())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuStatus {
    pub renderer: String,
    pub restricted: bool,
}

#[tauri::command]
pub fn get_gpu_status<R: Runtime>(app: AppHandle<R>) -> Result<GpuStatus, String> {
    #[cfg(target_os = "android")]
    {
        let state = app.state::<VcpMobileState<R>>();
        let plugin_handle = state.mobile_plugin_handle()?;

        let status = plugin_handle
            .run_mobile_plugin::<GpuStatus>("getGpuStatus", serde_json::json!({}))
            .map_err(|e| format!("run_mobile_plugin failed: {}", e))?;
        Ok(status)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Ok(GpuStatus {
            renderer: "PC Mock GPU".to_string(),
            restricted: true,
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RootAccessStatus {
    pub is_root: bool,
}

#[tauri::command]
pub fn check_root_access<R: Runtime>(app: AppHandle<R>) -> Result<RootAccessStatus, String> {
    #[cfg(target_os = "android")]
    {
        let state = app.state::<VcpMobileState<R>>();
        let plugin_handle = state.mobile_plugin_handle()?;

        let status = plugin_handle
            .run_mobile_plugin::<RootAccessStatus>("checkRootAccess", serde_json::json!({}))
            .map_err(|e| format!("run_mobile_plugin failed: {}", e))?;
        Ok(status)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Ok(RootAccessStatus { is_root: false })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RootCommandResult {
    pub success: bool,
    pub output: String,
}

#[tauri::command]
pub fn run_root_command<R: Runtime>(
    app: AppHandle<R>,
    command: String,
) -> Result<RootCommandResult, String> {
    #[cfg(target_os = "android")]
    {
        let state = app.state::<VcpMobileState<R>>();
        let plugin_handle = state.mobile_plugin_handle()?;

        let res = plugin_handle
            .run_mobile_plugin::<RootCommandResult>(
                "runRootCommand",
                serde_json::json!({ "command": command }),
            )
            .map_err(|e| format!("run_mobile_plugin failed: {}", e))?;
        Ok(res)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        let _ = command;
        Ok(RootCommandResult {
            success: false,
            output: "非Android物理端无法运行Root指令".to_string(),
        })
    }
}

#[derive(Deserialize)]
pub struct ClipboardReadResult {
    pub content: String,
}

pub fn write_clipboard_native<R: Runtime>(
    app: AppHandle<R>,
    content: String,
) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        let state = app.state::<VcpMobileState<R>>();
        let plugin_handle = state.mobile_plugin_handle()?;
        plugin_handle
            .run_mobile_plugin::<()>("writeClipboard", serde_json::json!({ "content": content }))
            .map_err(|e| format!("JNI writeClipboard failed: {}", e))?;
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        let _ = content;
    }
    Ok(())
}

pub fn read_clipboard_native<R: Runtime>(app: AppHandle<R>) -> Result<String, String> {
    #[cfg(target_os = "android")]
    {
        let state = app.state::<VcpMobileState<R>>();
        let plugin_handle = state.mobile_plugin_handle()?;
        let res = plugin_handle
            .run_mobile_plugin::<ClipboardReadResult>("readClipboard", serde_json::json!({}))
            .map_err(|e| format!("JNI readClipboard failed: {}", e))?;
        Ok(res.content)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Ok("Desktop Clipboard Placeholder".to_string())
    }
}

pub fn send_notification_native<R: Runtime>(
    app: AppHandle<R>,
    title: String,
    body: String,
) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        let state = app.state::<VcpMobileState<R>>();
        let plugin_handle = state.mobile_plugin_handle()?;
        plugin_handle
            .run_mobile_plugin::<()>(
                "sendLocalNotification",
                serde_json::json!({ "title": title, "body": body }),
            )
            .map_err(|e| format!("JNI sendLocalNotification failed: {}", e))?;
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        let _ = title;
        let _ = body;
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchRootManagerResult {
    pub success: bool,
    pub manager: Option<String>,
    pub message: Option<String>,
}

#[tauri::command]
pub fn launch_root_manager<R: Runtime>(
    app: AppHandle<R>,
) -> Result<LaunchRootManagerResult, String> {
    #[cfg(target_os = "android")]
    {
        let state = app.state::<VcpMobileState<R>>();
        let plugin_handle = state.mobile_plugin_handle()?;

        let res = plugin_handle
            .run_mobile_plugin::<LaunchRootManagerResult>(
                "launchRootManager",
                serde_json::json!({}),
            )
            .map_err(|e| format!("run_mobile_plugin failed: {}", e))?;
        Ok(res)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Ok(LaunchRootManagerResult {
            success: false,
            manager: None,
            message: Some("该接口仅在 Android 物理端可用".to_string()),
        })
    }
}

#[tauri::command]
pub fn acquire_wake_lock<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        let state = app.state::<VcpMobileState<R>>();
        let plugin_handle = state.mobile_plugin_handle()?;

        plugin_handle
            .run_mobile_plugin::<serde_json::Value>("acquireWakeLock", serde_json::json!({}))
            .map_err(|e| format!("run_mobile_plugin failed: {}", e))?;
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
    }
    Ok(())
}

#[tauri::command]
pub fn release_wake_lock<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        let state = app.state::<VcpMobileState<R>>();
        let plugin_handle = state.mobile_plugin_handle()?;

        plugin_handle
            .run_mobile_plugin::<serde_json::Value>("releaseWakeLock", serde_json::json!({}))
            .map_err(|e| format!("run_mobile_plugin failed: {}", e))?;
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
    }
    Ok(())
}

#[tauri::command]
pub fn start_network_monitoring<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        let state = app.state::<VcpMobileState<R>>();
        let plugin_handle = state.mobile_plugin_handle()?;

        plugin_handle
            .run_mobile_plugin::<serde_json::Value>("startNetworkMonitoring", serde_json::json!({}))
            .map_err(|e| format!("run_mobile_plugin failed: {}", e))?;
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
    }
    Ok(())
}

#[tauri::command]
pub fn get_pending_notification<R: Runtime>(
    app: AppHandle<R>,
) -> Result<serde_json::Value, String> {
    #[cfg(target_os = "android")]
    {
        let state = app.state::<VcpMobileState<R>>();
        let plugin_handle = state.mobile_plugin_handle()?;

        let notification_data = plugin_handle
            .run_mobile_plugin::<serde_json::Value>(
                "getPendingNotification",
                serde_json::Value::Null,
            )
            .map_err(|e| format!("run_mobile_plugin getPendingNotification failed: {}", e))?;
        Ok(notification_data)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Ok(serde_json::json!({}))
    }
}

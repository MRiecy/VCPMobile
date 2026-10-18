#[cfg(any(target_os = "android", target_os = "ios"))]
use crate::VcpMobileState;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
#[cfg(any(target_os = "android", target_os = "ios"))]
use tauri::Manager;
use tauri::{AppHandle, Runtime};

// The explicit host branches below are development fixtures. iOS must use
// its own adapter or return an unavailable error; it never consumes fixtures.

#[cfg(any(target_os = "android", target_os = "ios"))]
const TEMP_FILE_MAX_BYTES: usize = 20 * 1024 * 1024;
#[cfg(target_os = "android")]
const MAX_SHARED_FILES: usize = 16;

#[cfg(any(target_os = "android", target_os = "ios", test))]
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

#[cfg(any(target_os = "android", target_os = "ios", test))]
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

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NotificationAuthorization {
    NotDetermined,
    Denied,
    Authorized,
    Provisional,
    Ephemeral,
    Unknown,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NotificationPermission {
    pub authorization: NotificationAuthorization,
}

#[tauri::command]
pub async fn get_notification_permission<R: Runtime>(
    app: AppHandle<R>,
) -> Result<NotificationPermission, String> {
    #[cfg(target_os = "ios")]
    {
        crate::ios::run_async(app, "getNotificationPermission", serde_json::json!({})).await
    }
    #[cfg(not(target_os = "ios"))]
    {
        let _ = app;
        Err("unsupported: this notification authorization API requires iOS".into())
    }
}

#[tauri::command]
pub async fn request_notification_permission<R: Runtime>(
    app: AppHandle<R>,
) -> Result<NotificationPermission, String> {
    #[cfg(target_os = "ios")]
    {
        crate::ios::run_async(app, "requestNotificationPermission", serde_json::json!({})).await
    }
    #[cfg(not(target_os = "ios"))]
    {
        let _ = app;
        Err("unsupported: this notification authorization API requires iOS".into())
    }
}

#[tauri::command]
pub async fn open_notification_settings<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    #[cfg(target_os = "ios")]
    {
        crate::ios::run_async(app, "openNotificationSettings", serde_json::json!({})).await
    }
    #[cfg(not(target_os = "ios"))]
    {
        let _ = app;
        Err("unsupported: this notification settings API requires iOS".into())
    }
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
    #[cfg(target_os = "ios")]
    {
        let _ = app;
        crate::ios_unsupported("check_all_permissions")
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
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
    #[cfg(target_os = "ios")]
    {
        let _ = (app, p_type);
        crate::ios_unsupported::<()>("request_android_permission")?;
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
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
    #[cfg(target_os = "ios")]
    {
        let _ = app;
        crate::ios_unsupported::<()>("move_task_to_back")?;
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
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
    #[cfg(target_os = "ios")]
    {
        let _ = app;
        crate::ios_unsupported("check_notification_listener_permission")
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
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
    #[cfg(target_os = "ios")]
    {
        let _ = app;
        crate::ios_unsupported::<()>("request_notification_listener_permission")?;
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
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
    #[cfg(target_os = "ios")]
    {
        let _ = app;
        crate::ios_unsupported("request_auto_start_permission")
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
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
    #[cfg(target_os = "ios")]
    {
        let _ = app;
        crate::ios_unsupported("request_power_management_permission")
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
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
    #[cfg(target_os = "ios")]
    {
        let _ = app;
        crate::ios_unsupported("check_auto_start_permission")
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume: Option<String>,
}

#[tauri::command(async)]
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
    #[cfg(target_os = "ios")]
    {
        crate::ios::run(app, "getFreeDiskSpace", serde_json::json!({}))
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        let _ = app;
        Ok(DiskSpaceInfo {
            free_bytes: 10 * 1024 * 1024 * 1024,
            free_gb: 10.0,
            total_bytes: 100 * 1024 * 1024 * 1024,
            total_gb: 100.0,
            volume: None,
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

#[derive(Serialize, Deserialize)]
pub struct PickedFileFailure {
    pub name: String,
    pub message: String,
}

#[derive(Serialize, Deserialize)]
pub struct PickedFilesInfo {
    pub files: Vec<PickedFileInfo>,
    pub errors: Vec<PickedFileFailure>,
}

// Single-resource consumers (avatar/config/camera/CLI) and attachment batches
// share the same native acquisition owner, with explicit result shapes.
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
pub enum PickFileResult {
    Single(PickedFileInfo),
    Multiple(PickedFilesInfo),
}

#[tauri::command]
pub async fn prepare_file_operation<R: Runtime>(
    app: AppHandle<R>,
    webview: tauri::Webview<R>,
    operation_id: String,
) -> Result<serde_json::Value, String> {
    #[cfg(target_os = "ios")]
    {
        validate_operation_id(&operation_id)?;
        // Bind admission to the requesting WKWebView, not whichever replacement
        // happens to be attached when the queued native call is finally handled.
        let (sender, receiver) = tokio::sync::oneshot::channel();
        webview
            .with_webview(move |view| {
                let _ = sender.send(format!("{:x}", view.inner() as usize));
            })
            .map_err(|error| format!("cancelled: requesting WebView is unavailable: {error}"))?;
        let source_webview = receiver
            .await
            .map_err(|_| "cancelled: requesting WebView was detached".to_string())?;
        crate::ios::run_async(
            app,
            "prepareFileOperation",
            serde_json::json!({
                "operationId": operation_id, "sourceWebView": source_webview,
            }),
        )
        .await
    }
    #[cfg(not(target_os = "ios"))]
    {
        let _ = (app, webview, operation_id);
        Err("Picker admission is only used on iOS".into())
    }
}

#[tauri::command]
pub async fn pick_file<R: Runtime>(
    app: AppHandle<R>,
    mode: Option<String>,
    operation_id: Option<String>,
    max_bytes: Option<u64>,
    multiple: Option<bool>,
    reservation_token: Option<String>,
) -> Result<PickFileResult, String> {
    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        let mode = mode.unwrap_or_else(|| "file".into());
        if !matches!(
            mode.as_str(),
            "file" | "camera" | "gallery" | "avatar" | "config"
        ) {
            return Err("invalid_picker_mode".into());
        }
        let max_files = if multiple.unwrap_or(false) && matches!(mode.as_str(), "file" | "gallery")
        {
            16
        } else {
            1
        };
        let operation_id = operation_id.unwrap_or_else(native_operation_id);
        validate_operation_id(&operation_id)?;
        #[cfg(target_os = "ios")]
        if reservation_token.as_deref().is_none_or(str::is_empty) {
            return Err("cancelled: picker must be prepared before starting".into());
        }
        let max_bytes = max_bytes.unwrap_or(if mode == "config" {
            1024 * 1024
        } else {
            512 * 1024 * 1024
        });
        if max_bytes == 0 || max_bytes > 512 * 1024 * 1024 {
            return Err("invalid_file_budget".into());
        }
        let staging = app
            .path()
            .app_cache_dir()
            .map_err(|e| e.to_string())?
            .join("uploads");
        let state = app.state::<VcpMobileState<R>>();
        let plugin_handle = state.mobile_plugin_handle()?;
        tauri::async_runtime::spawn_blocking(move || {
            plugin_handle
                .run_mobile_plugin::<PickFileResult>(
                    "pickFile",
                    serde_json::json!({
                        "mode": mode, "operationId": operation_id,
                        "stagingDir": staging, "maxBytes": max_bytes, "maxFiles": max_files,
                        "reservationToken": reservation_token,
                    }),
                )
                .map_err(|e| format!("native_picker: {e}"))
        })
        .await
        .map_err(|e| format!("native_picker_worker: {e}"))?
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        let _ = (app, mode, operation_id, max_bytes, multiple, reservation_token);
        Err("文件选择仅在移动端可用".to_string())
    }
}

#[cfg(any(target_os = "android", target_os = "ios"))]
fn native_operation_id() -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    format!(
        "native-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

#[cfg(any(target_os = "android", target_os = "ios"))]
fn validate_operation_id(id: &str) -> Result<(), String> {
    if id.is_empty()
        || id.len() > 100
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err("invalid_operation_id".into());
    }
    Ok(())
}

#[tauri::command]
pub async fn cancel_file_operation<R: Runtime>(
    app: AppHandle<R>,
    operation_id: String,
) -> Result<(), String> {
    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        validate_operation_id(&operation_id)?;
        let handle = app.state::<VcpMobileState<R>>().mobile_plugin_handle()?;
        tauri::async_runtime::spawn_blocking(move || {
            handle
                .run_mobile_plugin::<serde_json::Value>(
                    "cancelFileOperation",
                    serde_json::json!({"operationId":operation_id}),
                )
                .map(|_| ())
                .map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| e.to_string())?
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        let _ = (app, operation_id);
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DerivedImageInfo {
    pub path: String,
    pub mime: String,
    pub size: u64,
    pub width: u32,
    pub height: u32,
}

#[tauri::command]
pub async fn get_webview_geometry<R: Runtime>(
    app: AppHandle<R>,
    sample_id: u64,
    viewport_revision: u64,
) -> Result<serde_json::Value, String> {
    #[cfg(target_os = "ios")]
    {
        crate::ios::run_async(
            app,
            "getWebViewGeometry",
            serde_json::json!({"sampleId":sample_id,"viewportRevision":viewport_revision}),
        )
        .await
    }
    #[cfg(not(target_os = "ios"))]
    {
        let _ = (app, sample_id, viewport_revision);
        Err("WebView geometry sampling requires iOS".into())
    }
}

pub fn clear_webview_cache_native<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    #[cfg(target_os = "ios")]
    {
        crate::ios::run::<R, serde_json::Value>(
            app.clone(),
            "clearWebViewCache",
            serde_json::json!({}),
        )
        .map(|_| ())
    }
    #[cfg(not(target_os = "ios"))]
    {
        let _ = app;
        Err("This WebView cache adapter requires iOS".into())
    }
}

/// Called from the file domain's serial blocking image worker.
#[allow(clippy::too_many_arguments)]
pub fn derive_image<R: Runtime>(
    app: &AppHandle<R>,
    source_path: &str,
    output_path: &str,
    max_dimension: u32,
    max_pixels: u64,
    max_bytes: u64,
    quality: f64,
    preserve_alpha: bool,
) -> Result<DerivedImageInfo, String> {
    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        app.state::<VcpMobileState<R>>().mobile_plugin_handle()?.run_mobile_plugin("deriveImage", serde_json::json!({
            "sourcePath":source_path,"outputPath":output_path,"maxDimension":max_dimension,"maxPixels":max_pixels,"maxBytes":max_bytes,"quality":quality,"preserveAlpha":preserve_alpha
        })).map_err(|e| e.to_string())
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        let _ = (
            app,
            source_path,
            output_path,
            max_dimension,
            max_pixels,
            max_bytes,
            quality,
            preserve_alpha,
        );
        Err("图像派生仅在移动端可用".into())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatteryStatus {
    pub level: Option<i32>,
    pub is_power_save_mode: bool,
    pub status: Option<String>,
    pub temperature: Option<f64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub unavailable: BTreeMap<String, String>,
}

#[tauri::command(async)]
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
    #[cfg(target_os = "ios")]
    {
        crate::ios::run(app, "getBatteryStatus", serde_json::json!({}))
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        let _ = app;
        Ok(BatteryStatus {
            level: Some(100),
            is_power_save_mode: false,
            status: Some("未充电".to_string()),
            temperature: Some(25.0),
            unavailable: BTreeMap::new(),
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkStatus {
    pub connected: bool,
    pub r#type: String,
    pub down_speed_kbps: Option<i32>,
    pub up_speed_kbps: Option<i32>,
    pub ip: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_expensive: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_constrained: Option<bool>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub unavailable: BTreeMap<String, String>,
}

#[tauri::command(async)]
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
    #[cfg(target_os = "ios")]
    {
        crate::ios::run(app, "getNetworkStatus", serde_json::json!({}))
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        let _ = app;
        Ok(NetworkStatus {
            connected: true,
            r#type: "以太网".to_string(),
            down_speed_kbps: Some(100000),
            up_speed_kbps: Some(100000),
            ip: Some("127.0.0.1".to_string()),
            is_expensive: None,
            is_constrained: None,
            unavailable: BTreeMap::new(),
        })
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSnapshot {
    pub model: String,
    pub system_name: String,
    pub system_version: String,
    pub architecture: String,
    pub hardware_identifier: Option<String>,
    pub processor_count: u32,
    pub active_processor_count: u32,
    pub cpu_usage_percent: Option<f64>,
    pub physical_memory_bytes: u64,
    pub process_resident_memory_bytes: Option<u64>,
    pub process_available_memory_bytes: u64,
    pub thermal_state: String,
    #[serde(default)]
    pub unavailable: BTreeMap<String, String>,
}

/// Device/CPU/memory tool input. This is an internal native read, not a second
/// platform capability snapshot or a frontend command.
pub fn get_device_snapshot<R: Runtime>(app: AppHandle<R>) -> Result<DeviceSnapshot, String> {
    #[cfg(target_os = "ios")]
    {
        crate::ios::run(app, "getDeviceSnapshot", serde_json::json!({}))
    }
    #[cfg(not(target_os = "ios"))]
    {
        let _ = app;
        Err("unsupported: this device snapshot API requires iOS".into())
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeFileActionResult {
    pub outcome: String,
}

#[tauri::command]
pub async fn open_file_native<R: Runtime>(
    app: AppHandle<R>,
    path: String,
    action: Option<String>,
    operation_id: Option<String>,
    file_name: Option<String>,
) -> Result<NativeFileActionResult, String> {
    let action = action.unwrap_or_else(|| "view".to_string());
    if action != "view" && action != "share" {
        return Err("Unsupported native file action".to_string());
    }

    #[cfg(target_os = "android")]
    {
        let state = app.state::<VcpMobileState<R>>();
        let plugin_handle = state.mobile_plugin_handle()?;

        tauri::async_runtime::spawn_blocking(move || {
            plugin_handle
                .run_mobile_plugin::<serde_json::Value>(
                    "openFile",
                    serde_json::json!({ "path": path, "action": action, "fileName":file_name }),
                )
                .map_err(|e| format!("run_mobile_plugin failed: {}", e))
        })
        .await
        .map_err(|e| e.to_string())??;
        let _ = operation_id;
        Ok(NativeFileActionResult {
            outcome: "opened".into(),
        })
    }
    #[cfg(target_os = "ios")]
    {
        let operation_id = operation_id.unwrap_or_else(native_operation_id);
        validate_operation_id(&operation_id)?;
        crate::ios::run_async(app, "openFile", serde_json::json!({"path":path,"action":action,"operationId":operation_id,"fileName":file_name})).await
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        let _ = (app, path, action, operation_id, file_name);
        Err("原生文件操作仅在移动端可用".into())
    }
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
    #[cfg(target_os = "ios")]
    {
        let _ = (app, path);
        crate::ios_unsupported("verify_apk_signature")
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
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
    #[cfg(target_os = "ios")]
    {
        let _ = app;
        crate::ios_unsupported("can_install_packages")
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
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
    #[cfg(target_os = "ios")]
    {
        let _ = app;
        crate::ios_unsupported::<()>("open_unknown_sources_settings")?;
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
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
    #[cfg(target_os = "ios")]
    {
        let _ = app;
        crate::ios_unsupported::<()>("acquire_ota_keepalive")?;
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
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
    #[cfg(target_os = "ios")]
    {
        let _ = app;
        crate::ios_unsupported::<()>("release_ota_keepalive")?;
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
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
    #[cfg(target_os = "ios")]
    {
        let _ = (app, max_width, quality);
        crate::ios_unsupported("capture_window_snapshot")
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
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

#[tauri::command(async)]
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
    #[cfg(target_os = "ios")]
    {
        crate::ios::run(
            app,
            "saveImageToGallery",
            serde_json::json!({"sourceUrl":source_url,"fileName":file_name}),
        )
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        let _ = app;
        let _ = source_url;
        let _ = file_name;
        Err("该接口仅在 Android 物理端可用".to_string())
    }
}

#[tauri::command(async)]
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
    #[cfg(target_os = "ios")]
    {
        crate::ios::run(
            app,
            "saveImageFromPath",
            serde_json::json!({"imagePath":image_path,"fileName":file_name}),
        )
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
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

/// Export an operation-owned file without routing its bytes through the WebView.
/// The caller keeps the source alive until this completes and owns its cleanup.
pub async fn save_file_to_downloads<R: Runtime>(
    app: AppHandle<R>,
    file_path: String,
    file_name: String,
    mime_type: Option<String>,
    max_bytes: u64,
) -> Result<DownloadsSaveResult, String> {
    #[cfg(target_os = "android")]
    {
        if max_bytes == 0 || max_bytes > i32::MAX as u64 {
            return Err("下载导出大小限制无效".to_string());
        }
        let state = app.state::<VcpMobileState<R>>();
        let plugin_handle = state.mobile_plugin_handle()?;
        tauri::async_runtime::spawn_blocking(move || {
            plugin_handle
                .run_mobile_plugin::<DownloadsSaveResult>(
                    "saveFileToDownloads",
                    serde_json::json!({
                        "filePath": file_path,
                        "fileName": file_name,
                        "mimeType": mime_type,
                        "maxBytes": max_bytes,
                    }),
                )
                .map_err(|error| format!("run_mobile_plugin failed: {error}"))
        })
        .await
        .map_err(|error| format!("downloads export worker failed: {error}"))?
    }
    #[cfg(target_os = "ios")]
    {
        crate::ios::run_async(app, "saveFileToDownloads", serde_json::json!({
            "filePath":file_path,"fileName":file_name,"mimeType":mime_type,"maxBytes":max_bytes,"operationId":native_operation_id()
        })).await
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        let _ = (app, file_path, file_name, mime_type, max_bytes);
        Err("该接口仅在 Android 物理端可用".to_string())
    }
}

#[tauri::command(async)]
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
    #[cfg(target_os = "ios")]
    {
        crate::ios::run(
            app,
            "saveToDownloads",
            serde_json::json!({
                "fileName":file_name,"contentBase64":content_base64,"mimeType":mime_type,"operationId":native_operation_id()
            }),
        )
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        let _ = app;
        let _ = file_name;
        let _ = content_base64;
        let _ = mime_type;
        Err("该接口仅在 Android 物理端可用".to_string())
    }
}

#[tauri::command(async)]
pub fn write_temp_file<R: Runtime>(
    app: AppHandle<R>,
    bytes: Vec<u8>,
    file_name: String,
) -> Result<String, String> {
    #[cfg(any(target_os = "android", target_os = "ios"))]
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
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        let _ = app;
        let _ = bytes;
        let _ = file_name;
        Err("该接口仅在 Android 物理端可用".to_string())
    }
}

#[tauri::command(async)]
pub fn delete_temp_file<R: Runtime>(app: AppHandle<R>, file_path: String) -> Result<(), String> {
    #[cfg(any(target_os = "android", target_os = "ios"))]
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
        let uploads = app
            .path()
            .app_cache_dir()
            .map_err(|e| e.to_string())?
            .join("uploads");
        let in_uploads = uploads.exists() && canonical_file_in(&uploads, path)?;
        if !path.is_absolute() || (!canonical_file_in(&temp_dir, path)? && !in_uploads) {
            return Err("拒绝删除非 bridge staging 文件".to_string());
        }
        std::fs::remove_file(path).map_err(|e| e.to_string())?;
        Ok(())
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
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
    #[cfg(target_os = "ios")]
    {
        let _ = app;
        crate::ios_unsupported::<()>("start_download_notification")?;
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
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
    #[cfg(target_os = "ios")]
    {
        let _ = (app, progress, text);
        crate::ios_unsupported::<()>("update_download_notification")?;
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
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
    #[cfg(target_os = "ios")]
    {
        let _ = app;
        crate::ios_unsupported::<()>("cancel_download_notification")?;
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
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
    #[cfg(target_os = "ios")]
    {
        let _ = app;
        crate::ios_unsupported::<()>("request_overlay_permission")?;
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
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
    #[cfg(target_os = "ios")]
    {
        let _ = (app, owner_id, files);
        crate::ios_not_implemented("register_shared_files")
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
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
    #[cfg(target_os = "ios")]
    {
        let _ = (app, show);
        crate::ios_unsupported("toggle_floating_ball")
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        let _ = app;
        let _ = show;
        Ok(false)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SensorSample {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub motion: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ambient: Option<String>,
}

/// 按本批 streaming tools 的真实需求执行一次有界传感器采样。
/// 原生回调只持有本次采样/授权窗口；同步 PluginHandle 调用放入 blocking worker。
/// 调用方取消只分离 worker，原生最终回调之前不会丢弃 Tauri receiver。
pub async fn sample_sensor_data<R: Runtime>(
    app: AppHandle<R>,
    location: bool,
    motion: bool,
    ambient: bool,
) -> Result<SensorSample, String> {
    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        let state = app.state::<VcpMobileState<R>>();
        let plugin_handle = state.mobile_plugin_handle()?;
        tauri::async_runtime::spawn_blocking(move || {
            plugin_handle
                .run_mobile_plugin::<SensorSample>(
                    "sampleSensorData",
                    serde_json::json!({
                        "location": location,
                        "motion": motion,
                        "ambient": ambient,
                    }),
                )
                .map_err(|error| format!("run_mobile_plugin failed: {error}"))
        })
        .await
        .map_err(|error| format!("sensor sample worker failed: {error}"))?
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        let _ = app;
        Ok(SensorSample {
            location: location
                .then(|| "坐标: 39.9000°N, 116.4000°E | 精度: 15m | 海拔: 50m (模拟)".to_string()),
            motion: motion
                .then(|| "状态: 静止 | 平均加速度: 9.80m/s² | 峰值: 9.80m/s² (模拟)".to_string()),
            ambient: ambient.then(|| "环境光: 150 lux (室内) | 气压: 1013 hPa (模拟)".to_string()),
        })
    }
}

#[tauri::command(async)]
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
    #[cfg(target_os = "ios")]
    {
        #[derive(Deserialize)]
        struct ThermalResponse {
            status: String,
        }
        let response: ThermalResponse =
            crate::ios::run(app, "getCpuThermalStatus", serde_json::json!({}))?;
        Ok(response.status)
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
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
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub unavailable: BTreeMap<String, String>,
}

#[tauri::command(async)]
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
    #[cfg(target_os = "ios")]
    {
        crate::ios::run(app, "getGpuStatus", serde_json::json!({}))
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        let _ = app;
        Ok(GpuStatus {
            renderer: "PC Mock GPU".to_string(),
            restricted: true,
            unavailable: BTreeMap::new(),
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
    #[cfg(target_os = "ios")]
    {
        let _ = app;
        crate::ios_unsupported("check_root_access")
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
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
    #[cfg(target_os = "ios")]
    {
        let _ = (app, command);
        crate::ios_unsupported("run_root_command")
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
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
    #[cfg(target_os = "ios")]
    {
        crate::ios::run::<R, ()>(
            app,
            "writeClipboard",
            serde_json::json!({ "content": content }),
        )?;
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
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
    #[cfg(target_os = "ios")]
    {
        let response: ClipboardReadResult =
            crate::ios::run(app, "readClipboard", serde_json::json!({}))?;
        Ok(response.content)
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
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
    #[cfg(target_os = "ios")]
    {
        crate::ios::run::<R, ()>(
            app,
            "sendLocalNotification",
            serde_json::json!({ "title": title, "body": body }),
        )?;
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
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
    #[cfg(target_os = "ios")]
    {
        let _ = app;
        crate::ios_unsupported("launch_root_manager")
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
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
    #[cfg(target_os = "ios")]
    {
        let _ = app;
        crate::ios_unsupported::<()>("acquire_wake_lock")?;
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
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
    #[cfg(target_os = "ios")]
    {
        let _ = app;
        crate::ios_unsupported::<()>("release_wake_lock")?;
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
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
    #[cfg(target_os = "ios")]
    {
        let _ = app;
        crate::ios_unsupported::<()>("start_network_monitoring")?;
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        let _ = app;
    }
    Ok(())
}

#[tauri::command(async)]
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
    #[cfg(target_os = "ios")]
    {
        crate::ios::run(app, "getPendingNotification", serde_json::json!({}))
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        let _ = app;
        Ok(serde_json::json!({}))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HapticFeedbackArgs {
    pub r#type: String,
    pub effect: Option<String>,
    pub duration_ms: Option<u64>,
    pub amplitude: Option<i32>,
}

#[tauri::command(async)]
pub fn trigger_haptic_feedback<R: Runtime>(
    app: AppHandle<R>,
    payload: HapticFeedbackArgs,
) -> Result<serde_json::Value, String> {
    #[cfg(target_os = "android")]
    {
        let state = app.state::<VcpMobileState<R>>();
        let plugin_handle = state.mobile_plugin_handle()?;

        plugin_handle
            .run_mobile_plugin::<serde_json::Value>(
                "triggerHapticFeedback",
                serde_json::to_value(&payload).map_err(|e| e.to_string())?,
            )
            .map_err(|e| format!("run_mobile_plugin triggerHapticFeedback failed: {}", e))
    }
    #[cfg(target_os = "ios")]
    {
        let _ = (app, payload);
        crate::ios_not_implemented("trigger_haptic_feedback")
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        let _ = (app, payload);
        Ok(serde_json::json!({
            "performed": false,
            "reason": "host_mock"
        }))
    }
}

#[cfg(test)]
mod device_result_tests {
    use super::{BatteryStatus, NetworkStatus};
    use serde_json::json;

    #[test]
    fn battery_keeps_a_real_zero_distinct_from_an_unavailable_native_reading() {
        let android: BatteryStatus = serde_json::from_value(json!({
            "level": 0, "isPowerSaveMode": true,
            "status": "未充电", "temperature": 23.5
        }))
        .unwrap();
        assert_eq!(android.level, Some(0));
        assert!(android.unavailable.is_empty());

        // Swift synthesized Encodable omits nil fields. Rust's frontend result
        // must still express an unknown reading as null, rather than 0 or 100.
        let ios: BatteryStatus = serde_json::from_value(json!({
            "isPowerSaveMode": false,
            "unavailable": {"level": "系统尚未提供电量读数", "temperature": "iOS 未公开电池温度"}
        }))
        .unwrap();
        assert_eq!(ios.level, None);
        assert_eq!(ios.temperature, None);
        let result = serde_json::to_value(ios).unwrap();
        assert!(result["level"].is_null());
        assert_eq!(result["unavailable"]["level"], "系统尚未提供电量读数");
    }

    #[test]
    fn network_path_without_speed_does_not_become_a_zero_speed_measurement() {
        let android: NetworkStatus = serde_json::from_value(json!({
            "connected": true, "type": "wifi", "downSpeedKbps": 0,
            "upSpeedKbps": 1200, "ip": "192.168.1.2"
        }))
        .unwrap();
        assert_eq!(android.down_speed_kbps, Some(0));
        assert_eq!(android.up_speed_kbps, Some(1200));
        assert_eq!(android.ip.as_deref(), Some("192.168.1.2"));

        let ios: NetworkStatus = serde_json::from_value(json!({
            "connected": true, "type": "Wi-Fi", "isExpensive": false,
            "isConstrained": true, "unavailable": {
                "downSpeedKbps": "NWPath 不提供链路下行速率",
                "ip": "未取得活动接口地址"
            }
        }))
        .unwrap();
        assert!(ios.connected);
        assert_eq!(ios.down_speed_kbps, None);
        assert_eq!(ios.ip, None);
        assert_eq!(ios.is_constrained, Some(true));
        let result = serde_json::to_value(ios).unwrap();
        assert!(result["downSpeedKbps"].is_null());
        assert_eq!(result["unavailable"]["ip"], "未取得活动接口地址");
    }
}

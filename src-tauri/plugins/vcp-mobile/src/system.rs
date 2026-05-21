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

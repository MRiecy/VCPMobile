#[allow(unused_imports)]
use tauri::{AppHandle, Manager, Runtime};

#[allow(unused_imports)]
use crate::VcpMobileState;

/// 申请持有进程级前台锁 (语义化接口)
pub fn acquire_foreground_inner<R: Runtime>(
    _app: &AppHandle<R>,
    tag: &str,
    priority: i32,
    label: &str,
    screen_keep_on: bool,
) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        let state = _app.state::<VcpMobileState<R>>();
        let handle = state.plugin_handle.lock().map_err(|e| e.to_string())?;
        let plugin_handle = handle.as_ref().ok_or("Plugin handle not initialized")?;
        plugin_handle
            .run_mobile_plugin::<serde_json::Value>(
                "acquireForeground",
                serde_json::json!({
                    "tag": tag,
                    "priority": priority,
                    "label": label,
                    "screenKeepOn": screen_keep_on
                }),
            )
            .map_err(|e| format!("run_mobile_plugin failed: {}", e))?;
    }

    log::info!(
        "[VcpMobilePlugin] acquire_foreground_inner: tag={}, priority={}, label={}, screenKeepOn={}",
        tag, priority, label, screen_keep_on
    );

    Ok(())
}

/// 释放进程级前台锁 (语义化接口)
pub fn release_foreground_inner<R: Runtime>(_app: &AppHandle<R>, tag: &str) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        let state = _app.state::<VcpMobileState<R>>();
        let handle = state.plugin_handle.lock().map_err(|e| e.to_string())?;
        let plugin_handle = handle.as_ref().ok_or("Plugin handle not initialized")?;
        plugin_handle
            .run_mobile_plugin::<serde_json::Value>(
                "releaseForeground",
                serde_json::json!({ "tag": tag }),
            )
            .map_err(|e| format!("run_mobile_plugin failed: {}", e))?;
    }

    log::info!("[VcpMobilePlugin] release_foreground_inner: tag={}", tag);

    Ok(())
}

/// Start the stream keepalive service (兼容老版本接口)
pub fn start_stream_service_inner<R: Runtime>(
    app: &AppHandle<R>,
    agent_name: &str,
) -> Result<(), String> {
    let tag = format!("stream:{}", agent_name);
    let priority = if agent_name.contains("[数据同步]") {
        40
    } else if agent_name.contains("[预渲染重建]") {
        30
    } else {
        20
    };
    let screen_keep_on = agent_name.contains("[数据同步]") || agent_name.contains("[预渲染重建]");
    acquire_foreground_inner(app, &tag, priority, agent_name, screen_keep_on)
}

/// Stop the stream keepalive service (兼容老版本接口)
pub fn stop_stream_service_inner<R: Runtime>(
    app: &AppHandle<R>,
    agent_name: &str,
) -> Result<(), String> {
    let tag = format!("stream:{}", agent_name);
    release_foreground_inner(app, &tag)
}

/// 设置分布式保活模式 (兼容老版本接口)
pub fn set_keepalive_mode_inner<R: Runtime>(
    app: &AppHandle<R>,
    is_keepalive: bool,
) -> Result<(), String> {
    if is_keepalive {
        acquire_foreground_inner(app, "distributed", 10, "distributed", false)
    } else {
        release_foreground_inner(app, "distributed")
    }
}

#[tauri::command]
pub fn acquire_foreground<R: Runtime>(
    app: AppHandle<R>,
    tag: String,
    priority: i32,
    label: String,
    screen_keep_on: bool,
) -> Result<(), String> {
    acquire_foreground_inner(&app, &tag, priority, &label, screen_keep_on)
}

#[tauri::command]
pub fn release_foreground<R: Runtime>(app: AppHandle<R>, tag: String) -> Result<(), String> {
    release_foreground_inner(&app, &tag)
}

#[tauri::command]
pub fn start_streaming_service<R: Runtime>(
    app: AppHandle<R>,
    agent_name: String,
) -> Result<(), String> {
    start_stream_service_inner(&app, &agent_name)
}

#[tauri::command]
pub fn stop_streaming_service<R: Runtime>(
    app: AppHandle<R>,
    agent_name: String,
) -> Result<(), String> {
    stop_stream_service_inner(&app, &agent_name)
}
#[tauri::command]
pub fn set_keepalive_mode<R: Runtime>(app: AppHandle<R>, is_keepalive: bool) -> Result<(), String> {
    set_keepalive_mode_inner(&app, is_keepalive)
}

#[tauri::command]
#[allow(unused_variables)]
pub fn start_helper_service<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        let state = app.state::<VcpMobileState<R>>();
        let handle = state.plugin_handle.lock().map_err(|e| e.to_string())?;
        let plugin_handle = handle.as_ref().ok_or("Plugin handle not initialized")?;
        plugin_handle
            .run_mobile_plugin::<serde_json::Value>("startHelperService", serde_json::json!({}))
            .map_err(|e| format!("startHelperService failed: {}", e))?;
    }
    log::info!("[VcpMobilePlugin] start_helper_service called");
    Ok(())
}

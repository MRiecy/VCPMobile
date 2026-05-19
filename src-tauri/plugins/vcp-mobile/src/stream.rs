use std::sync::atomic::Ordering;
use tauri::{AppHandle, Manager, Runtime};

use crate::VcpMobileState;

/// Start the stream keepalive service.
/// Counter 0→1 triggers actual Android foreground service start.
pub fn start_stream_service_inner<R: Runtime>(
    app: &AppHandle<R>,
    agent_name: &str,
) -> Result<(), String> {
    let state = app.state::<VcpMobileState<R>>();
    let count = state.streaming_count.fetch_add(1, Ordering::SeqCst);

    if count == 0 {
        #[cfg(target_os = "android")]
        {
            let handle = state
                .plugin_handle
                .lock()
                .map_err(|e| e.to_string())?;
            let plugin_handle = handle
                .as_ref()
                .ok_or("Plugin handle not initialized")?;
            plugin_handle
                .run_mobile_plugin::<serde_json::Value>(
                    "startStreamingService",
                    serde_json::json!({ "agentName": agent_name }),
                )
                .map_err(|e| format!("run_mobile_plugin failed: {}", e))?;
        }
    }

    log::info!(
        "[VcpMobilePlugin] Stream started for '{}'. Active count: {}",
        agent_name,
        count + 1
    );

    Ok(())
}

/// Stop the stream keepalive service.
/// Counter reaches 0 triggers actual Android service stop.
pub fn stop_stream_service_inner<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let state = app.state::<VcpMobileState<R>>();
    let count = state.streaming_count.fetch_sub(1, Ordering::SeqCst);

    if count <= 1 {
        state.streaming_count.store(0, Ordering::SeqCst);
        #[cfg(target_os = "android")]
        {
            let handle = state
                .plugin_handle
                .lock()
                .map_err(|e| e.to_string())?;
            let plugin_handle = handle
                .as_ref()
                .ok_or("Plugin handle not initialized")?;
            plugin_handle
                .run_mobile_plugin::<serde_json::Value>(
                    "stopStreamingService",
                    serde_json::json!({}),
                )
                .map_err(|e| format!("run_mobile_plugin failed: {}", e))?;
        }
    }

    log::info!(
        "[VcpMobilePlugin] Stream stopped. Active count: {}",
        state.streaming_count.load(Ordering::SeqCst)
    );

    Ok(())
}

#[tauri::command]
pub fn start_stream_service<R: Runtime>(
    app: AppHandle<R>,
    agent_name: String,
) -> Result<(), String> {
    start_stream_service_inner(&app, &agent_name)
}

#[tauri::command]
pub fn stop_stream_service<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    stop_stream_service_inner(&app)
}

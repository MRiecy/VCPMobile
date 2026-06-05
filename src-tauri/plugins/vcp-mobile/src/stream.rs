use tauri::{AppHandle, Manager, Runtime};

use crate::VcpMobileState;

/// Start the stream keepalive service.
pub fn start_stream_service_inner<R: Runtime>(
    app: &AppHandle<R>,
    agent_name: &str,
) -> Result<(), String> {
    let state = app.state::<VcpMobileState<R>>();

    let active_names = {
        let mut streams = state.active_streams.lock().map_err(|e| e.to_string())?;

        // 更新计数或添加新 Agent
        if let Some(pos) = streams.iter().position(|(name, _)| name == agent_name) {
            streams[pos].1 += 1;
        } else {
            streams.push((agent_name.to_string(), 1));
        }

        // 格式化名字列表：A、B、C...
        let names: Vec<&str> = streams.iter().map(|(name, _)| name.as_str()).collect();
        if names.len() > 3 {
            format!("{}...", names[..3].join("、"))
        } else {
            names.join("、")
        }
    };

    #[cfg(target_os = "android")]
    {
        let handle = state.plugin_handle.lock().map_err(|e| e.to_string())?;
        let plugin_handle = handle.as_ref().ok_or("Plugin handle not initialized")?;
        plugin_handle
            .run_mobile_plugin::<serde_json::Value>(
                "startStreamingService",
                serde_json::json!({ "agentName": active_names }),
            )
            .map_err(|e| format!("run_mobile_plugin failed: {}", e))?;
    }

    log::info!(
        "[VcpMobilePlugin] Stream started for '{}'. Current notification: {}",
        agent_name,
        active_names
    );

    Ok(())
}

/// Stop the stream keepalive service.
pub fn stop_stream_service_inner<R: Runtime>(
    app: &AppHandle<R>,
    agent_name: &str,
) -> Result<(), String> {
    let state = app.state::<VcpMobileState<R>>();

    let (_should_stop, active_names) = {
        let mut streams = state.active_streams.lock().map_err(|e| e.to_string())?;

        if let Some(pos) = streams.iter().position(|(name, _)| name == agent_name) {
            if streams[pos].1 > 1 {
                streams[pos].1 -= 1;
            } else {
                streams.remove(pos);
            }
        }

        let names: Vec<&str> = streams.iter().map(|(name, _)| name.as_str()).collect();
        let formatted = if names.len() > 3 {
            format!("{}...", names[..3].join("、"))
        } else {
            names.join("、")
        };

        (streams.is_empty(), formatted)
    };

    #[cfg(target_os = "android")]
    {
        let handle = state.plugin_handle.lock().map_err(|e| e.to_string())?;
        let plugin_handle = handle.as_ref().ok_or("Plugin handle not initialized")?;

        if _should_stop {
            plugin_handle
                .run_mobile_plugin::<serde_json::Value>(
                    "startStreamingService",
                    serde_json::json!({ "agentName": "" }),
                )
                .map_err(|e| format!("run_mobile_plugin failed: {}", e))?;
        } else {
            // 更新通知内容为剩余的 Agent
            plugin_handle
                .run_mobile_plugin::<serde_json::Value>(
                    "startStreamingService",
                    serde_json::json!({ "agentName": active_names }),
                )
                .map_err(|e| format!("run_mobile_plugin failed: {}", e))?;
        }
    }

    log::info!(
        "[VcpMobilePlugin] Stream stopped for '{}'. Current notification: {}",
        agent_name,
        active_names
    );

    Ok(())
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

/// 设置分布式保活模式
pub fn set_keepalive_mode<R: Runtime>(
    _app: &AppHandle<R>,
    is_keepalive: bool,
) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        let state = _app.state::<VcpMobileState<R>>();
        let handle = state.plugin_handle.lock().map_err(|e| e.to_string())?;
        let plugin_handle = handle.as_ref().ok_or("Plugin handle not initialized")?;
        plugin_handle
            .run_mobile_plugin::<serde_json::Value>(
                "startStreamingService",
                serde_json::json!({
                    "agentName": "",
                    "isKeepaliveMode": is_keepalive
                }),
            )
            .map_err(|e| format!("run_mobile_plugin failed: {}", e))?;
    }

    log::info!("[VcpMobilePlugin] Keepalive mode set to {}", is_keepalive);

    Ok(())
}

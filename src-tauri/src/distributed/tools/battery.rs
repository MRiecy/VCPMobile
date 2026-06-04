// distributed/tools/battery.rs
// [Streaming] MobileBatteryInfo — battery level, charging status, temperature.
// Reads /sys/class/power_supply/battery/{capacity, status, temp}


use crate::distributed::tool_registry::StreamingTool;
use crate::distributed::types::ToolManifest;


pub struct BatteryInfoTool;


impl StreamingTool for BatteryInfoTool {
    fn manifest(&self) -> ToolManifest {
        ToolManifest {
            name: "MobileBatteryInfo".to_string(),
            description: "监控实时电量、充电状态、电池健康度及是否处于省电模式。".to_string(),
            display_name: "电池状态".to_string(),
            placeholder: Some("{{MobileBattery}}".to_string()),
            invocation_commands: vec![],
        }
    }

    fn placeholder_key(&self) -> &str {
        "{{MobileBattery}}"
    }

    fn poll_interval_secs(&self) -> u64 {
        60
    }

    fn read_current(&self, app: &tauri::AppHandle) -> Result<String, String> {
        #[cfg(target_os = "android")]
        {
            use tauri::Manager;
            let state = app.state::<tauri_plugin_vcp_mobile::VcpMobileState<tauri::Wry>>();
            let handle_guard = state.plugin_handle.lock().map_err(|e| e.to_string())?;
            let plugin_handle = handle_guard.as_ref().ok_or("VcpMobile plugin not initialized")?;
            
            #[derive(serde::Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct BatteryResponse {
                level: i32,
                is_power_save_mode: bool,
                status: Option<String>,
                temperature: Option<f64>,
            }
            
            let res = plugin_handle
                .run_mobile_plugin::<BatteryResponse>(
                    "getBatteryStatus",
                    serde_json::json!({}),
                )
                .map_err(|e| format!("JNI call failed: {}", e))?;

            let status_str = res.status.unwrap_or_else(|| "未知".to_string());
            let temp_str = match res.temperature {
                Some(t) if t >= 0.0 => format!("{:.1}°C", t),
                _ => "N/A".to_string(),
            };
            
            let pwr_save = if res.is_power_save_mode { " (低功耗模式)" } else { "" };
            
            Ok(format!(
                "电量: {}%{} | 状态: {} | 温度: {}",
                res.level, pwr_save, status_str, temp_str
            ))
        }
        #[cfg(not(target_os = "android"))]
        {
            let _ = app;
            Ok("电池信息不可用".to_string())
        }
    }
}

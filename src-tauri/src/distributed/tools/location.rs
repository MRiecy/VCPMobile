// distributed/tools/location.rs
// [Streaming] MobileLocation — GPS position from Android native LocationManager.

use tauri::AppHandle;

use crate::distributed::tool_registry::StreamingTool;
use crate::distributed::types::ToolManifest;


pub struct LocationTool;

impl StreamingTool for LocationTool {
    fn manifest(&self) -> ToolManifest {
        ToolManifest {
            name: "MobileLocation".to_string(),
            description: "获取当前的经纬度高精度坐标、移动速度、海拔高度及定位源精度。".to_string(),
            display_name: "GPS 地理定位".to_string(),
            placeholder: Some("{{MobileLocation}}".to_string()),
            invocation_commands: vec![],
        }
    }

    fn placeholder_key(&self) -> &str {
        "{{MobileLocation}}"
    }

    fn poll_interval_secs(&self) -> u64 {
        120
    }

    fn read_current(&self, app: &AppHandle) -> Result<String, String> {
        #[cfg(target_os = "android")]
        {
            use tauri::Manager;
            let state = app.state::<tauri_plugin_vcp_mobile::VcpMobileState<tauri::Wry>>();
            let handle_guard = state.plugin_handle.lock().map_err(|e| e.to_string())?;
            let plugin_handle = handle_guard.as_ref().ok_or("VcpMobile plugin not initialized")?;
            
            #[derive(serde::Deserialize)]
            struct SensorResponse {
                value: String,
            }
            
            let res = plugin_handle
                .run_mobile_plugin::<SensorResponse>(
                    "getSensorData",
                    serde_json::json!({ "type": "location" }),
                )
                .map_err(|e| format!("JNI call failed: {}", e))?;
            Ok(res.value)
        }
        #[cfg(not(target_os = "android"))]
        {
            let _ = app;
            Ok("坐标: 39.9000°N, 116.4000°E | 精度: 15m | 海拔: 50m (模拟)".to_string())
        }
    }
}

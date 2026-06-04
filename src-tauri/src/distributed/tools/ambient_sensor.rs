// distributed/tools/ambient_sensor.rs
// [Streaming] MobileAmbient — ambient light and barometer from Android native sensors.

use serde_json::json;
use tauri::AppHandle;

use crate::distributed::tool_registry::StreamingTool;
use crate::distributed::types::{ToolManifest, CommType};

pub struct AmbientSensorTool;

impl StreamingTool for AmbientSensorTool {
    fn manifest(&self) -> ToolManifest {
        ToolManifest {
            name: "MobileAmbient".to_string(),
            description: "读取设备所处的物理环境光照度 (Lux) 与气压值 (hPa)，推算环境场景。".to_string(),
            parameters: json!({}),
            tool_type: "mobile".to_string(),
            display_name: "物理环境传感器".to_string(),
            icon: "i-lucide-sun".to_string(),
            placeholder: Some("{{MobileAmbient}}".to_string()),
            communication: CommType::Ipc {
                command: "plugin:vcp-mobile|get_sensor_data".to_string(),
                args: Some(json!({ "sensorType": "ambient" })),
            },
        }
    }

    fn placeholder_key(&self) -> &str {
        "{{MobileAmbient}}"
    }

    fn poll_interval_secs(&self) -> u64 {
        60
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
                    serde_json::json!({ "type": "ambient" }),
                )
                .map_err(|e| format!("JNI call failed: {}", e))?;
            Ok(res.value)
        }
        #[cfg(not(target_os = "android"))]
        {
            let _ = app;
            Ok("环境光: 150 lux (室内) | 气压: 1013 hPa (模拟)".to_string())
        }
    }
}

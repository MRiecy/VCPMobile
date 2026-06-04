// distributed/tools/motion_sensor.rs
// [Streaming] MobileMotion — device motion state from Android native accelerometer sensors.

use tauri::AppHandle;

use crate::distributed::tool_registry::StreamingTool;
use crate::distributed::types::ToolManifest;


pub struct MotionSensorTool;

impl StreamingTool for MotionSensorTool {
    fn manifest(&self) -> ToolManifest {
        ToolManifest {
            name: "MobileMotion".to_string(),
            description: "采集设备的三轴加速度、陀螺仪旋转向量与磁力计取向，识别物理运动姿态。".to_string(),
            display_name: "九轴运动传感器".to_string(),
            placeholder: Some("{{MobileMotion}}".to_string()),
            invocation_commands: vec![],
        }
    }

    fn placeholder_key(&self) -> &str {
        "{{MobileMotion}}"
    }

    fn poll_interval_secs(&self) -> u64 {
        30
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
                    serde_json::json!({ "type": "motion" }),
                )
                .map_err(|e| format!("JNI call failed: {}", e))?;
            Ok(res.value)
        }
        #[cfg(not(target_os = "android"))]
        {
            let _ = app;
            let brief = "状态: 静止 (模拟)";
            let detail = "状态: 静止 | 平均加速度: 9.80m/s² (峰值: 9.80m/s²) (模拟)";
            let folded = format!(
                "[===vcp_fold: 0.0 ::desc: 物理运动姿态粗略状态(静止、步行、步行中或剧烈移动)===]\n{}\n\n[===vcp_fold: 0.50 ::desc: 九轴高频遥测指标、旋转角速度、加速度峰值、三轴磁敏度物理强度===]\n{}",
                brief, detail
            );
            Ok(folded)
        }
    }
}

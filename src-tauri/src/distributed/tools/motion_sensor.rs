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
            description: "采集设备的三轴加速度、陀螺仪旋转向量与磁力计取向，识别物理运动姿态。"
                .to_string(),
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
        use tauri::Manager;
        let dist_state = app.state::<crate::distributed::DistributedState>();
        let motion = dist_state.telemetry.get_motion_info(app);
        Ok(motion)
    }
}

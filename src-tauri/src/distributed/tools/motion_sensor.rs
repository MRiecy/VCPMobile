// distributed/tools/motion_sensor.rs
// [Streaming] MobileMotion — device motion state from frontend DeviceMotion API.
// Frontend pushes via Tauri command → read_current() returns cached value.

use serde_json::json;

use crate::distributed::tool_registry::StreamingTool;
use crate::distributed::types::ToolManifest;

use super::frontend_bridge;

pub struct MotionSensorTool;

impl StreamingTool for MotionSensorTool {
    fn manifest(&self) -> ToolManifest {
        ToolManifest {
            name: "MobileMotion".to_string(),
            description: "移动设备运动状态(加速度/步数/运动类型)".to_string(),
            parameters: json!({}),
            tool_type: "mobile".to_string(),
        }
    }

    fn placeholder_key(&self) -> &str {
        "{{MobileMotion}}"
    }

    fn poll_interval_secs(&self) -> u64 {
        30
    }

    fn read_current(&self) -> Result<String, String> {
        // Max staleness: 2 minutes
        match frontend_bridge::read_sensor("motion", 120) {
            Some(val) => Ok(val),
            None => Ok("运动状态: 等待前端采集...".to_string()),
        }
    }
}

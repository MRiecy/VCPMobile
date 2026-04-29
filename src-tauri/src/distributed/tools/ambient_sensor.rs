// distributed/tools/ambient_sensor.rs
// [Streaming] MobileAmbient — ambient light and barometer from frontend sensor APIs.
// Frontend pushes via Tauri command → read_current() returns cached value.

use serde_json::json;

use crate::distributed::tool_registry::StreamingTool;
use crate::distributed::types::ToolManifest;

use super::frontend_bridge;

pub struct AmbientSensorTool;

impl StreamingTool for AmbientSensorTool {
    fn manifest(&self) -> ToolManifest {
        ToolManifest {
            name: "MobileAmbient".to_string(),
            description: "移动设备环境传感器(环境光/气压)".to_string(),
            parameters: json!({}),
            tool_type: "mobile".to_string(),
        }
    }

    fn placeholder_key(&self) -> &str {
        "{{MobileAmbient}}"
    }

    fn poll_interval_secs(&self) -> u64 {
        60
    }

    fn read_current(&self) -> Result<String, String> {
        // Max staleness: 3 minutes
        match frontend_bridge::read_sensor("ambient", 180) {
            Some(val) => Ok(val),
            None => Ok("环境传感器: 等待前端采集...".to_string()),
        }
    }
}

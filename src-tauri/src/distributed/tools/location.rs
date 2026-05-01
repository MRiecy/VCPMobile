// distributed/tools/location.rs
// [Streaming] MobileLocation — GPS position from frontend geolocation API.
// Frontend pushes via Tauri command → read_current() returns cached value.

use serde_json::json;

use crate::distributed::tool_registry::StreamingTool;
use crate::distributed::types::ToolManifest;

use super::frontend_bridge;

pub struct LocationTool;

impl StreamingTool for LocationTool {
    fn manifest(&self) -> ToolManifest {
        ToolManifest {
            name: "MobileLocation".to_string(),
            description: "移动设备GPS位置信息(坐标/地址/精度)".to_string(),
            parameters: json!({}),
            tool_type: "mobile".to_string(),
        }
    }

    fn placeholder_key(&self) -> &str {
        "{{MobileLocation}}"
    }

    fn poll_interval_secs(&self) -> u64 {
        120
    }

    fn read_current(&self) -> Result<String, String> {
        // Max staleness: 5 minutes (GPS may update slowly or be unavailable indoors)
        match frontend_bridge::read_sensor("location", 300) {
            Some(val) => Ok(val),
            None => Ok("位置信息: 等待前端采集...".to_string()),
        }
    }
}

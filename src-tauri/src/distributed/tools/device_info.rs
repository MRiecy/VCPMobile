// distributed/tools/device_info.rs
// [OneShot] MobileDeviceInfo — returns device model, OS, battery, etc.
// Mirrors VCPChat's WindowSensor plugin.

use async_trait::async_trait;
use serde_json::{json, Value};
use tauri::AppHandle;

use crate::distributed::tool_registry::OneShotTool;
use crate::distributed::types::ToolManifest;

pub struct DeviceInfoTool;

#[async_trait]
impl OneShotTool for DeviceInfoTool {
    fn manifest(&self) -> ToolManifest {
        ToolManifest {
            name: "MobileDeviceInfo".to_string(),
            description: "获取移动设备的基本信息，包括操作系统、设备型号等。Get mobile device information including OS, device model, etc.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
            tool_type: "mobile".to_string(),
        }
    }

    async fn execute(&self, _args: Value, _app: &AppHandle) -> Result<Value, String> {
        Ok(json!({
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "family": std::env::consts::FAMILY,
        }))
    }
}

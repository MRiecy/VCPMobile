// distributed/tools/device_info.rs
// [OneShot] MobileDeviceInfo — returns device model, OS, battery, etc.
// Mirrors VCPChat's WindowSensor plugin.

use async_trait::async_trait;
use serde_json::{json, Value};
use tauri::AppHandle;

use crate::distributed::tool_registry::OneShotTool;
use crate::distributed::types::ToolManifest;

use crate::distributed::types::CommType;

pub struct DeviceInfoTool;

#[async_trait]
impl OneShotTool for DeviceInfoTool {
    fn manifest(&self) -> ToolManifest {
        ToolManifest {
            name: "MobileDeviceInfo".to_string(),
            description: "提供物理设备的基础硬件信息，包括品牌、型号、系统版本及 ABI 架构。".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
            tool_type: "mobile".to_string(),
            display_name: "设备信息".to_string(),
            icon: "i-lucide-smartphone".to_string(),
            placeholder: None,
            communication: CommType::Mock,
            requires_root: false,
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

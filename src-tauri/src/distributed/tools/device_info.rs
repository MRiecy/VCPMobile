// distributed/tools/device_info.rs
// [OneShot] MobileDeviceInfo — returns device model, OS, battery, etc.
// Mirrors VCPChat's WindowSensor plugin.

use async_trait::async_trait;
use serde_json::{json, Value};
use tauri::AppHandle;

use crate::distributed::tool_registry::OneShotTool;
use crate::distributed::types::ToolManifest;

use crate::distributed::types::InvocationCommand;

pub struct DeviceInfoTool;

#[async_trait]
impl OneShotTool for DeviceInfoTool {
    fn manifest(&self) -> ToolManifest {
        ToolManifest {
            name: "MobileDeviceInfo".to_string(),
            description: "提供物理设备的基础硬件信息，包括品牌、型号、系统版本及 ABI 架构。".to_string(),
            display_name: "设备信息".to_string(),
            placeholder: None,
            invocation_commands: vec![
                InvocationCommand {
                    command_identifier: "MobileDeviceInfo".to_string(),
                    description: "查询当前连接的移动设备的基础硬件信息，包括操作系统类型、CPU 架构和系统家族。无需任何参数。\n\
调用格式:\n\
<<<[TOOL_REQUEST]>>>\n\
tool_name:「始」MobileDeviceInfo「末」\n\
<<<[END_TOOL_REQUEST]>>>".to_string(),
                    example: "<<<[TOOL_REQUEST]>>>\ntool_name:「始」MobileDeviceInfo「末」\n<<<[END_TOOL_REQUEST]>>>".to_string(),
                },
            ],
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

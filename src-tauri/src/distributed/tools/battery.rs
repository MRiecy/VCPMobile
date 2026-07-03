// distributed/tools/battery.rs
// [Streaming] MobileBatteryInfo — battery level, charging status, temperature.
// Reads /sys/class/power_supply/battery/{capacity, status, temp}

use crate::distributed::tool_registry::StreamingTool;
use crate::distributed::types::ToolManifest;

pub struct BatteryInfoTool;

impl StreamingTool for BatteryInfoTool {
    fn manifest(&self) -> ToolManifest {
        ToolManifest {
            name: "MobileBatteryInfo".to_string(),
            description: "监控实时电量、充电状态、电池健康度及是否处于省电模式。".to_string(),
            display_name: "电池状态".to_string(),
            placeholder: Some("{{MobileBattery}}".to_string()),
            invocation_commands: vec![],
        }
    }

    fn placeholder_key(&self) -> &str {
        "{{MobileBattery}}"
    }

    fn poll_interval_secs(&self) -> u64 {
        60
    }

    fn read_current(&self, app: &tauri::AppHandle) -> Result<String, String> {
        use tauri::Manager;
        let dist_state = app.state::<crate::distributed::DistributedState>();
        let battery_info = dist_state.telemetry.get_battery_info(app);
        Ok(battery_info)
    }
}

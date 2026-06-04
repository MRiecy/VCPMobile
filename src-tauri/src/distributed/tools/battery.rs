// distributed/tools/battery.rs
// [Streaming] MobileBatteryInfo — battery level, charging status, temperature.
// Reads /sys/class/power_supply/battery/{capacity, status, temp}

use serde_json::json;

use crate::distributed::tool_registry::StreamingTool;
use crate::distributed::types::ToolManifest;

use super::sysfs_utils::read_sysfs;

const BATTERY_BASE: &str = "/sys/class/power_supply/battery";

pub struct BatteryInfoTool;

impl BatteryInfoTool {
    fn read_capacity(&self) -> String {
        let raw = read_sysfs(&format!("{}/capacity", BATTERY_BASE));
        if raw.is_empty() {
            return "N/A".to_string();
        }
        format!("{}%", raw)
    }

    fn read_status(&self) -> String {
        let raw = read_sysfs(&format!("{}/status", BATTERY_BASE));
        if raw.is_empty() {
            return "未知".to_string();
        }
        match raw.as_str() {
            "Charging" => "充电中".to_string(),
            "Discharging" => "放电中".to_string(),
            "Full" => "已充满".to_string(),
            "Not charging" => "未充电".to_string(),
            other => other.to_string(),
        }
    }

    fn read_temp(&self) -> String {
        // Battery temp is in units of 0.1°C (e.g. 320 = 32.0°C)
        let raw = read_sysfs(&format!("{}/temp", BATTERY_BASE));
        match raw.parse::<i64>() {
            Ok(t) => {
                let deg = t as f64 / 10.0;
                format!("{:.0}°C", deg)
            }
            Err(_) => "N/A".to_string(),
        }
    }
}

use crate::distributed::types::CommType;

impl StreamingTool for BatteryInfoTool {
    fn manifest(&self) -> ToolManifest {
        ToolManifest {
            name: "MobileBatteryInfo".to_string(),
            description: "监控实时电量、充电状态、电池健康度及是否处于省电模式。".to_string(),
            parameters: json!({}),
            tool_type: "mobile".to_string(),
            display_name: "电池状态".to_string(),
            icon: "i-lucide-battery".to_string(),
            placeholder: Some("{{MobileBattery}}".to_string()),
            communication: CommType::Ipc {
                command: "plugin:vcp-mobile|get_battery_status".to_string(),
                args: None,
            },
        }
    }

    fn placeholder_key(&self) -> &str {
        "{{MobileBattery}}"
    }

    fn poll_interval_secs(&self) -> u64 {
        60
    }

    fn read_current(&self, app: &tauri::AppHandle) -> Result<String, String> {
        let _ = app;
        let capacity = self.read_capacity();
        let status = self.read_status();
        let temp = self.read_temp();

        // If all values are N/A, the sysfs path likely doesn't exist
        if capacity == "N/A" && status == "未知" && temp == "N/A" {
            return Ok("电池信息不可用".to_string());
        }

        Ok(format!(
            "电量: {} | 状态: {} | 温度: {}",
            capacity, status, temp
        ))
    }
}

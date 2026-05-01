// distributed/tools/device_status_summary.rs
// [Streaming] MobileStatus — aggregates all Phase 1 & 2 sensor data into a one-line summary.
// Reads from other StreamingTools' data sources directly for minimal overhead.

use serde_json::json;

use crate::distributed::tool_registry::StreamingTool;
use crate::distributed::types::ToolManifest;

use super::frontend_bridge;
use super::sysfs_utils::{find_thermal_zone, read_sysfs};

pub struct DeviceStatusSummaryTool;

impl DeviceStatusSummaryTool {
    /// CPU: brief usage + temp
    fn cpu_brief(&self) -> String {
        // Read /proc/stat for a rough idle% snapshot (single sample, not delta)
        let stat = read_sysfs("/proc/stat");
        let cpu_str = if let Some(line) = stat.lines().next() {
            let vals: Vec<u64> = line
                .split_whitespace()
                .skip(1) // skip "cpu"
                .filter_map(|v| v.parse().ok())
                .collect();
            if vals.len() >= 4 {
                let total: u64 = vals.iter().sum();
                let idle = vals[3];
                if let Some(div_result) = (idle * 100).checked_div(total) {
                    let usage = 100 - div_result;
                    format!("{}%", usage)
                } else {
                    "N/A".to_string()
                }
            } else {
                "N/A".to_string()
            }
        } else {
            "N/A".to_string()
        };

        let temp = find_thermal_zone("cpu")
            .and_then(|zone| read_sysfs(&format!("{}/temp", zone)).parse::<i64>().ok())
            .map(|t| format!("{}°C", t / 1000))
            .unwrap_or_default();

        if temp.is_empty() {
            format!("CPU:{}", cpu_str)
        } else {
            format!("CPU:{}/{}", cpu_str, temp)
        }
    }

    /// GPU: brief usage + temp
    fn gpu_brief(&self) -> Option<String> {
        // Try Adreno busy%
        let busy = read_sysfs("/sys/class/kgsl/kgsl-3d0/gpu_busy_percentage");
        if !busy.is_empty() {
            let temp = read_sysfs("/sys/class/kgsl/kgsl-3d0/temp")
                .parse::<i64>()
                .ok()
                .map(|t| format!("/{}°C", t / 1000))
                .unwrap_or_default();
            return Some(format!("GPU:{}{}", busy.trim_end_matches('%'), temp));
        }
        // Try Mali
        let util = read_sysfs("/sys/class/misc/mali0/device/utilization");
        if !util.is_empty() {
            return Some(format!("GPU:{}%", util));
        }
        None
    }

    /// Memory: used%
    fn mem_brief(&self) -> String {
        let info = read_sysfs("/proc/meminfo");
        let mut total = 0u64;
        let mut avail = 0u64;
        for line in info.lines() {
            if line.starts_with("MemTotal:") {
                total = line
                    .split_whitespace()
                    .nth(1)
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0);
            } else if line.starts_with("MemAvailable:") {
                avail = line
                    .split_whitespace()
                    .nth(1)
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0);
            }
        }
        if let Some(pct) = ((total - avail) * 100).checked_div(total) {
            format!("内存:{}%", pct)
        } else {
            "内存:N/A".to_string()
        }
    }

    /// Battery: level + status
    fn battery_brief(&self) -> String {
        let cap = read_sysfs("/sys/class/power_supply/battery/capacity");
        let status = read_sysfs("/sys/class/power_supply/battery/status");
        if cap.is_empty() {
            return "电量:N/A".to_string();
        }
        let suffix = match status.as_str() {
            "Charging" => "充电中",
            "Full" => "已满",
            _ => "",
        };
        if suffix.is_empty() {
            format!("电量:{}%", cap)
        } else {
            format!("电量:{}%{}", cap, suffix)
        }
    }

    /// Network: type
    fn net_brief(&self) -> String {
        let route = read_sysfs("/proc/net/route");
        for line in route.lines().skip(1) {
            let cols: Vec<&str> = line.split('\t').collect();
            if cols.len() >= 2 && cols[1] == "00000000" {
                let iface = cols[0];
                let net_type = if iface.starts_with("wlan") {
                    "WiFi"
                } else if iface.starts_with("rmnet") || iface.starts_with("ccmni") {
                    "移动数据"
                } else {
                    iface
                };
                return net_type.to_string();
            }
        }
        "离线".to_string()
    }

    /// Location: brief from frontend bridge
    fn location_brief(&self) -> Option<String> {
        frontend_bridge::read_sensor("location", 300).map(|val| {
            // Extract just the coordinate portion if available
            if val.starts_with("坐标:") {
                // Take up to first " | " for brevity
                val.split(" | ").next().unwrap_or(&val).to_string()
            } else {
                val
            }
        })
    }

    /// Motion: brief state from frontend bridge
    fn motion_brief(&self) -> Option<String> {
        frontend_bridge::read_sensor("motion", 120).and_then(|val| {
            // Extract just the state, e.g. "步行中"
            if val.starts_with("状态: ") {
                val.strip_prefix("状态: ")
                    .and_then(|s| s.split(" | ").next())
                    .map(|s| s.to_string())
            } else {
                Some(val)
            }
        })
    }
}

impl StreamingTool for DeviceStatusSummaryTool {
    fn manifest(&self) -> ToolManifest {
        ToolManifest {
            name: "MobileStatusSummary".to_string(),
            description: "移动设备状态聚合摘要(CPU/GPU/内存/电池/网络/位置/运动)".to_string(),
            parameters: json!({}),
            tool_type: "mobile".to_string(),
        }
    }

    fn placeholder_key(&self) -> &str {
        "{{MobileStatus}}"
    }

    fn poll_interval_secs(&self) -> u64 {
        60
    }

    fn read_current(&self) -> Result<String, String> {
        let mut parts = Vec::with_capacity(8);

        parts.push(self.cpu_brief());
        if let Some(gpu) = self.gpu_brief() {
            parts.push(gpu);
        }
        parts.push(self.mem_brief());
        parts.push(self.battery_brief());
        parts.push(self.net_brief());
        if let Some(loc) = self.location_brief() {
            parts.push(loc);
        }
        if let Some(motion) = self.motion_brief() {
            parts.push(motion);
        }

        Ok(format!("[手机状态] {}", parts.join(" | ")))
    }
}

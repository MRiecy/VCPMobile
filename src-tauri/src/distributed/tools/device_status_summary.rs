// distributed/tools/device_status_summary.rs
// [Streaming] MobileStatus — aggregates all Phase 1 & 2 sensor data into a one-line summary.
// Reads from other StreamingTools' data sources directly for minimal overhead.

use serde_json::json;

use crate::distributed::tool_registry::StreamingTool;
use crate::distributed::types::ToolManifest;

use super::sysfs_utils::{find_thermal_zone, read_sysfs};

pub struct DeviceStatusSummaryTool;

#[allow(dead_code)]
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
}

use crate::distributed::types::CommType;

impl StreamingTool for DeviceStatusSummaryTool {
    fn manifest(&self) -> ToolManifest {
        ToolManifest {
            name: "MobileStatusSummary".to_string(),
            description: "分布式节点专属大图，整合电池、CPU、内存及核心遥测状态，向外部提供一键摘要。".to_string(),
            parameters: json!({}),
            tool_type: "mobile".to_string(),
            display_name: "整机状态摘要".to_string(),
            icon: "i-lucide-gauge".to_string(),
            placeholder: Some("{{MobileStatus}}".to_string()),
            communication: CommType::Mock,
            requires_root: false,
        }
    }

    fn placeholder_key(&self) -> &str {
        "{{MobileStatus}}"
    }

    fn poll_interval_secs(&self) -> u64 {
        60
    }

    fn read_current(&self, app: &tauri::AppHandle) -> Result<String, String> {
        #[cfg(target_os = "android")]
        {
            // 1. 获取 CPU 使用率与温度快照
            let cpu_usage = if let Some(out) = super::sysfs_utils::execute_root_command_safe(app, "cat /proc/stat") {
                if let Some(line) = out.lines().next() {
                    let vals: Vec<u64> = line
                        .split_whitespace()
                        .skip(1)
                        .filter_map(|v| v.parse().ok())
                        .collect();
                    if vals.len() >= 4 {
                        let total: u64 = vals.iter().sum();
                        let idle = vals[3];
                        if let Some(div_result) = (idle * 100).checked_div(total) {
                            format!("{}%", 100 - div_result)
                        } else {
                            "受系统安全限制".to_string()
                        }
                    } else {
                        "受系统安全限制".to_string()
                    }
                } else {
                    "受系统安全限制".to_string()
                }
            } else {
                "受系统安全限制".to_string()
            };

            let cpu_temp = if let Some(raw_temp) = super::sysfs_utils::execute_root_command_safe(app, "cat /sys/class/thermal/thermal_zone0/temp") {
                if let Ok(t) = raw_temp.trim().parse::<i64>() {
                    format!("{}°C", t / 1000)
                } else {
                    "".to_string()
                }
            } else {
                match tauri_plugin_vcp_mobile::system::get_cpu_thermal_status(app.clone()) {
                    Ok(status) => status, // 由 Kotlin 转译好的中文发热状态
                    Err(_) => "".to_string(),
                }
            };

            // 2. 获取 GPU 型号与负载快照
            let mut gpu_loaded = false;
            let mut gpu_load_str = "受系统安全限制".to_string();
            let gpu_renderer = match tauri_plugin_vcp_mobile::system::get_gpu_status(app.clone()) {
                Ok(gpu_status) => gpu_status.renderer,
                Err(_) => "Unknown GPU".to_string(),
            };

            if let Some(raw_busy) = super::sysfs_utils::execute_root_command_safe(app, "cat /sys/class/kgsl/kgsl-3d0/gpubusy") {
                let raw_parts: Vec<&str> = raw_busy.split_whitespace().collect();
                if raw_parts.len() >= 2 {
                    if let (Ok(busy), Ok(total)) = (raw_parts[0].parse::<u64>(), raw_parts[1].parse::<u64>()) {
                        if total > 0 {
                            let pct = (busy as f64 / total as f64) * 100.0;
                            gpu_load_str = format!("{}%", pct.round() as u64);
                            gpu_loaded = true;
                        }
                    }
                }
            }

            if !gpu_loaded {
                let mali_paths = [
                    "cat /sys/devices/platform/14ac0000.mali/utilization",
                    "cat /sys/devices/platform/gpu/utilization",
                    "cat /sys/devices/platform/mali/utilization",
                ];
                for path in &mali_paths {
                    if let Some(raw_busy) = super::sysfs_utils::execute_root_command_safe(app, path) {
                        let clean = raw_busy.trim().trim_end_matches('%');
                        if let Ok(load) = clean.parse::<u64>() {
                            gpu_load_str = format!("{}%", load);
                            break;
                        }
                    }
                }
            }

            // 3. 内存已用占比 (无需 Root)
            let mem_str = self.mem_brief();

            // 4. 电量及充电状态 (利用 JNI API)
            let battery_str = match tauri_plugin_vcp_mobile::system::get_battery_status(app.clone()) {
                Ok(bat) => {
                    let suffix = match bat.status.as_deref() {
                        Some("充电中") => "充电中",
                        Some("已充满") => "已满",
                        _ => "",
                    };
                    format!("电量:{}%{}", bat.level, suffix)
                }
                Err(_) => "电量:N/A".to_string(),
            };

            // 5. 网络连接状态 (利用 JNI API)
            let net_str = match tauri_plugin_vcp_mobile::system::get_network_status(app.clone()) {
                Ok(net) => {
                    if net.connected {
                        net.r#type
                    } else {
                        "离线".to_string()
                    }
                }
                Err(_) => "离线".to_string(),
            };

            // 6. 物理传感器数据 (位置、九轴运动状态)
            let mut coords = "位置信息: 等待数据采集...".to_string();
            let mut motion_state = "静止".to_string();
            
            use tauri::Manager;
            if let Some(state) = app.try_state::<tauri_plugin_vcp_mobile::VcpMobileState<tauri::Wry>>() {
                if let Ok(handle_guard) = state.plugin_handle.lock() {
                    if let Some(plugin_handle) = handle_guard.as_ref() {
                        #[derive(serde::Deserialize)]
                        struct AllSensorResponse {
                            location: String,
                            motion: String,
                        }
                        if let Ok(res) = plugin_handle.run_mobile_plugin::<AllSensorResponse>(
                            "getSensorData",
                            serde_json::json!({ "type": "all" }),
                        ) {
                            if res.location.starts_with("坐标:") {
                                if let Some(c) = res.location.split(" | ").next() {
                                    coords = c.to_string();
                                }
                            }
                            if res.motion.starts_with("状态: ") {
                                if let Some(m) = res.motion.strip_prefix("状态: ").and_then(|s| s.split(" | ").next()) {
                                    motion_state = m.to_string();
                                }
                            }
                        }
                    }
                }
            }

            // 7. 组装折叠块文本协议 (高包含低累加逻辑)
            let brief_str = format!(
                "{} | {} | 网络:{} | CPU温度:{}", 
                battery_str, mem_str, net_str, if cpu_temp.is_empty() { "正常".to_string() } else { cpu_temp.clone() }
            );

            let cpu_block = if cpu_temp.is_empty() {
                format!("CPU:{}", cpu_usage)
            } else {
                format!("CPU:{} | 温度:{}", cpu_usage, cpu_temp)
            };
            let perf_str = format!(
                "{} | {} | 网络:{} | {} | GPU:{} | 负载:{}",
                battery_str, mem_str, net_str, cpu_block, gpu_renderer, gpu_load_str
            );

            let full_str = format!(
                "{} | {} | 运动:{}",
                perf_str, coords, motion_state
            );

            let folded = format!(
                "[===vcp_fold: 0.0 ::desc: 设备基本电量网络和内存占用概要===]\n[手机状态] {}\n\n[===vcp_fold: 0.35 ::desc: 手机性能规格与运行负载===]\n[手机状态] {}\n\n[===vcp_fold: 0.45 ::desc: 手机物理传感器和详细定位状态===]\n[手机状态] {}",
                brief_str, perf_str, full_str
            );

            Ok(folded)
        }

        #[cfg(not(target_os = "android"))]
        {
            let _ = app;
            let cpu_b = self.cpu_brief();
            let gpu_b = self.gpu_brief().unwrap_or_else(|| "GPU:信息不可用".to_string());
            let mem_b = self.mem_brief();
            let battery_b = self.battery_brief();
            let net_b = self.net_brief();
            let coords_b = "坐标: 39.9000°N, 116.4000°E (模拟)";
            let motion_b = "静止 (模拟)";

            let brief_str = format!("{} | {} | 网络:{} | CPU温度:正常", battery_b, mem_b, net_b);
            let perf_str = format!("{} | {} | 网络:{} | {} | {}", battery_b, mem_b, net_b, cpu_b, gpu_b);
            let full_str = format!("{} | {} | 运动:{}", perf_str, coords_b, motion_b);

            let folded = format!(
                "[===vcp_fold: 0.0 ::desc: 设备基本电量网络和内存占用概要===]\n[手机状态] {}\n\n[===vcp_fold: 0.35 ::desc: 手机性能规格与运行负载===]\n[手机状态] {}\n\n[===vcp_fold: 0.45 ::desc: 手机物理传感器和详细定位状态===]\n[手机状态] {}",
                brief_str, perf_str, full_str
            );

            Ok(folded)
        }
    }
}

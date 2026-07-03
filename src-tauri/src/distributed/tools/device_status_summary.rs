// distributed/tools/device_status_summary.rs
// [Streaming] MobileStatus — aggregates all Phase 1 & 2 sensor data into a one-line summary.
// Reads from other StreamingTools' data sources directly for minimal overhead.

use crate::distributed::tool_registry::StreamingTool;
use crate::distributed::types::ToolManifest;

use crate::distributed::DistributedState;
pub struct DeviceStatusSummaryTool;

impl StreamingTool for DeviceStatusSummaryTool {
    fn manifest(&self) -> ToolManifest {
        ToolManifest {
            name: "MobileStatusSummary".to_string(),
            description:
                "分布式节点专属大图，整合电池、CPU、内存及核心遥测状态，向外部提供一键摘要。"
                    .to_string(),
            display_name: "整机状态摘要".to_string(),
            placeholder: Some("{{MobileStatus}}".to_string()),
            invocation_commands: vec![],
        }
    }

    fn placeholder_key(&self) -> &str {
        "{{MobileStatus}}"
    }

    fn poll_interval_secs(&self) -> u64 {
        60
    }

    fn read_current(&self, app: &tauri::AppHandle) -> Result<String, String> {
        use tauri::Manager;
        let dist_state = app.state::<DistributedState>();
        let telemetry = &dist_state.telemetry;

        let battery_str = telemetry.get_battery_brief(app);
        let mem_str = telemetry.get_memory_brief();
        let net_str = telemetry.get_network_brief(app);
        let cpu_temp = telemetry.get_cpu_temp(app);
        let cpu_usage = telemetry.get_cpu_usage(app);
        let gpu_info = telemetry.get_gpu_info(app);
        let coords = telemetry.get_coords_brief(app);
        let motion_state = telemetry.get_motion_brief(app);

        let brief_str = format!(
            "{} | {} | 网络:{} | CPU温度:{}",
            battery_str,
            mem_str,
            net_str,
            if cpu_temp.is_empty() || cpu_temp == "N/A" {
                "正常".to_string()
            } else {
                cpu_temp.clone()
            }
        );

        let cpu_block = if cpu_temp.is_empty() || cpu_temp == "N/A" {
            format!("CPU:{}", cpu_usage)
        } else {
            format!("CPU:{} | 温度:{}", cpu_usage, cpu_temp)
        };

        let perf_str = format!(
            "{} | {} | 网络:{} | {} | GPU:{}",
            battery_str, mem_str, net_str, cpu_block, gpu_info
        );

        let full_str = format!("{} | {} | 运动:{}", perf_str, coords, motion_state);

        let folded = format!(
            "[===vcp_fold: 0.0 ::desc: 设备基本电量网络和内存占用概要===]\n[手机状态] {}\n\n[===vcp_fold: 0.35 ::desc: 手机性能规格与运行负载===]\n[手机状态] {}\n\n[===vcp_fold: 0.45 ::desc: 手机物理传感器和详细定位状态===]\n[手机状态] {}",
            brief_str, perf_str, full_str
        );

        Ok(folded)
    }
}

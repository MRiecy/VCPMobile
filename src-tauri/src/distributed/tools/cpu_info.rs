// distributed/tools/cpu_info.rs
// [Streaming] MobileCPUInfo — CPU usage, frequency, temperature.
// Usage: delta sampling from /proc/stat (requires two reads to compute %).
// Frequency: /sys/devices/system/cpu/cpu*/cpufreq/scaling_cur_freq
// Temperature: PowerManager thermal status level via JNI, or exact millideg via Root.

use crate::distributed::tool_registry::StreamingTool;
use crate::distributed::types::ToolManifest;



pub struct CpuInfoTool;

impl CpuInfoTool {
    pub fn new() -> Self {
        Self
    }


}

impl StreamingTool for CpuInfoTool {
    fn manifest(&self) -> ToolManifest {
        ToolManifest {
            name: "MobileCPUInfo".to_string(),
            description: "显示多核 CPU 拓扑、当前主频、核心温度及整机 CPU 占用率。".to_string(),
            display_name: "CPU核心监控".to_string(),
            placeholder: Some("{{MobileCPU}}".to_string()),
            invocation_commands: vec![],
        }
    }

    fn placeholder_key(&self) -> &str {
        "{{MobileCPU}}"
    }

    fn poll_interval_secs(&self) -> u64 {
        15
    }

    fn read_current(&self, app: &tauri::AppHandle) -> Result<String, String> {
        use tauri::Manager;
        let dist_state = app.state::<crate::distributed::DistributedState>();
        let usage = dist_state.telemetry.get_cpu_usage(app);
        let freq = dist_state.telemetry.get_cpu_freq();
        let temp = dist_state.telemetry.get_cpu_temp(app);

        // 0.0 级：热度温度块
        let brief_str = format!("CPU温度: {}", temp);

        // 0.40 级：使用率及规格累加块 (包含 0.0 级数据)
        let detail_str = format!("CPU 使用率: {} | 频率: {} | 温度: {}", usage, freq, temp);

        let folded = format!(
            "[===vcp_fold: 0.0 ::desc: CPU芯片当前温度与发热情况===]\n{}\n\n[===vcp_fold: 0.40 ::desc: CPU核心当前运行主频、核心温控热敏感知、硬件拓扑与主频规格===]\n{}",
            brief_str, detail_str
        );

        Ok(folded)
    }
}

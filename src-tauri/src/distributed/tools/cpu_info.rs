// distributed/tools/cpu_info.rs
// [Streaming] MobileCPUInfo — CPU usage, frequency, temperature.
// Usage: delta sampling from /proc/stat (requires two reads to compute %).
// Frequency: /sys/devices/system/cpu/cpu*/cpufreq/scaling_cur_freq
// Temperature: PowerManager thermal status level via JNI, or exact millideg via Root.

use std::sync::Mutex;

use crate::distributed::tool_registry::StreamingTool;
use crate::distributed::types::ToolManifest;

use super::sysfs_utils::read_sysfs;
use super::sysfs_utils::read_sysfs_u64;

/// Snapshot of aggregate CPU counters from /proc/stat first line.
#[derive(Clone)]
struct CpuSample {
    total: u64,
    idle: u64,
}

pub struct CpuInfoTool {
    prev_sample: Mutex<Option<CpuSample>>,
}

impl CpuInfoTool {
    pub fn new() -> Self {
        Self {
            prev_sample: Mutex::new(None),
        }
    }

    /// Parse the first "cpu " line from /proc/stat into a CpuSample.
    fn read_stat_sample(&self, app: &tauri::AppHandle) -> Option<CpuSample> {
        // 优先使用 Root 管道获取，若失败则降级为直接读取 (非 Root 会返回空)
        let content = match super::sysfs_utils::execute_root_command_safe(app, "cat /proc/stat") {
            Some(out) => out,
            None => read_sysfs("/proc/stat"),
        };

        for line in content.lines() {
            if !line.starts_with("cpu ") {
                continue;
            }
            // cpu  user nice system idle iowait irq softirq steal guest guest_nice
            let fields: Vec<u64> = line
                .split_whitespace()
                .skip(1) // skip "cpu"
                .filter_map(|s| s.parse::<u64>().ok())
                .collect();
            if fields.len() < 4 {
                return None;
            }
            let total: u64 = fields.iter().sum();
            // idle = fields[3], iowait = fields[4] (if present)
            let idle = fields[3] + fields.get(4).unwrap_or(&0);
            return Some(CpuSample { total, idle });
        }
        None
    }

    /// Compute CPU usage % from delta between previous and current sample.
    fn read_usage(&self, app: &tauri::AppHandle) -> String {
        let current = match self.read_stat_sample(app) {
            Some(s) => s,
            None => return "受系统安全限制".to_string(),
        };

        let mut prev = self.prev_sample.lock().unwrap_or_else(|e| e.into_inner());
        let result = match prev.as_ref() {
            Some(prev_sample) => {
                let total_diff = current.total.saturating_sub(prev_sample.total);
                let idle_diff = current.idle.saturating_sub(prev_sample.idle);
                if total_diff == 0 {
                    "0%".to_string()
                } else {
                    let usage = ((total_diff - idle_diff) as f64 / total_diff as f64) * 100.0;
                    format!("{:.0}%", usage)
                }
            }
            None => "采集中...".to_string(),
        };

        *prev = Some(current);
        result
    }

    /// Read CPU frequencies, group by big/little core, return summary string.
    fn read_freq(&self) -> String {
        let cpu_base = "/sys/devices/system/cpu";
        let mut freqs_khz: Vec<u64> = Vec::new();

        // Try cpu0..cpu15
        for i in 0..16 {
            let path = format!("{}/cpu{}/cpufreq/scaling_cur_freq", cpu_base, i);
            if let Some(freq) = read_sysfs_u64(&path) {
                freqs_khz.push(freq);
            }
        }

        if freqs_khz.is_empty() {
            return "N/A".to_string();
        }

        let max_freq = *freqs_khz.iter().max().unwrap();
        let min_freq = *freqs_khz.iter().min().unwrap();

        // If big.LITTLE, show both; otherwise just max
        if max_freq > min_freq * 12 / 10 {
            // >20% difference → likely big.LITTLE
            format!(
                "{:.1}/{:.1}GHz",
                min_freq as f64 / 1_000_000.0,
                max_freq as f64 / 1_000_000.0,
            )
        } else {
            format!("{:.1}GHz", max_freq as f64 / 1_000_000.0)
        }
    }

    /// Read CPU thermal status.
    /// Try reading exact millideg node under Root first; fall back to PowerManager thermal level JNI.
    fn read_temp(&self, app: &tauri::AppHandle) -> String {
        // 1. 优先尝试使用 Root 管道极速读取精确热敏芯片数值
        if let Some(raw_temp) = super::sysfs_utils::execute_root_command_safe(
            app,
            "cat /sys/class/thermal/thermal_zone0/temp",
        ) {
            if let Ok(temp_millideg) = raw_temp.trim().parse::<i64>() {
                if temp_millideg > 0 {
                    return format!("{}°C", temp_millideg / 1000);
                }
            }
        }

        // 2. 无 Root 或读取失败，降级回退到 JNI 的 PowerManager 热分级 API
        #[cfg(target_os = "android")]
        {
            use tauri::Manager;
            let result: Result<String, String> = (|| {
                let state = app.state::<tauri_plugin_vcp_mobile::VcpMobileState<tauri::Wry>>();
                let handle_guard = state.plugin_handle.lock().map_err(|e| e.to_string())?;
                let plugin_handle = handle_guard
                    .as_ref()
                    .ok_or("VcpMobile plugin not initialized")?;

                #[derive(serde::Deserialize)]
                struct ThermalResponse {
                    status: String,
                }

                let res = plugin_handle
                    .run_mobile_plugin::<ThermalResponse>(
                        "getCpuThermalStatus",
                        serde_json::json!({}),
                    )
                    .map_err(|e| format!("JNI call failed: {}", e))?;

                Ok(res.status)
            })();

            match result {
                Ok(status) => status,
                Err(_) => "N/A".to_string(),
            }
        }
        #[cfg(not(target_os = "android"))]
        {
            let _ = app;
            "正常".to_string()
        }
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
            web_socket_push: None,
        }
    }

    fn placeholder_key(&self) -> &str {
        "{{MobileCPU}}"
    }

    fn poll_interval_secs(&self) -> u64 {
        15
    }

    fn read_current(&self, app: &tauri::AppHandle) -> Result<String, String> {
        let usage = self.read_usage(app);
        let freq = self.read_freq();
        let temp = self.read_temp(app);

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

// distributed/tools/cpu_info.rs
// [Streaming] MobileCPUInfo — CPU usage, frequency, temperature.
// Usage: delta sampling from /proc/stat (requires two reads to compute %).
// Frequency: /sys/devices/system/cpu/cpu*/cpufreq/scaling_cur_freq
// Temperature: /sys/class/thermal/thermal_zone*/type matching "cpu"

use std::sync::Mutex;

use serde_json::json;

use crate::distributed::tool_registry::StreamingTool;
use crate::distributed::types::ToolManifest;

use super::sysfs_utils::{find_thermal_zone, read_sysfs, read_sysfs_u64, read_thermal_temp};

/// Snapshot of aggregate CPU counters from /proc/stat first line.
#[derive(Clone)]
struct CpuSample {
    total: u64,
    idle: u64,
}

pub struct CpuInfoTool {
    prev_sample: Mutex<Option<CpuSample>>,
    /// Cached thermal zone path for CPU (probed once).
    cpu_thermal_zone: Mutex<Option<Option<String>>>,
}

impl CpuInfoTool {
    pub fn new() -> Self {
        Self {
            prev_sample: Mutex::new(None),
            cpu_thermal_zone: Mutex::new(None),
        }
    }

    /// Parse the first "cpu " line from /proc/stat into a CpuSample.
    fn read_stat_sample(&self) -> Option<CpuSample> {
        let content = read_sysfs("/proc/stat");
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
    fn read_usage(&self) -> String {
        let current = match self.read_stat_sample() {
            Some(s) => s,
            None => return "N/A".to_string(),
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

    /// Read CPU temperature from thermal zone.
    fn read_temp(&self) -> String {
        let mut zone = self.cpu_thermal_zone.lock().unwrap_or_else(|e| e.into_inner());
        if zone.is_none() {
            // Probe once: try "cpu" keyword in thermal zone types
            *zone = Some(find_thermal_zone("cpu"));
        }

        match zone.as_ref().unwrap() {
            Some(path) => read_thermal_temp(path),
            None => "N/A".to_string(),
        }
    }
}

impl StreamingTool for CpuInfoTool {
    fn manifest(&self) -> ToolManifest {
        ToolManifest {
            name: "MobileCPUInfo".to_string(),
            description: "移动设备CPU状态(使用率/频率/温度)".to_string(),
            parameters: json!({}),
            tool_type: "mobile".to_string(),
        }
    }

    fn placeholder_key(&self) -> &str {
        "{{MobileCPU}}"
    }

    fn poll_interval_secs(&self) -> u64 {
        15
    }

    fn read_current(&self) -> Result<String, String> {
        let usage = self.read_usage();
        let freq = self.read_freq();
        let temp = self.read_temp();

        Ok(format!(
            "CPU 使用率: {} | 频率: {} | 温度: {}",
            usage, freq, temp
        ))
    }
}

// distributed/tools/gpu_info.rs
// [Streaming] MobileGPUInfo — GPU usage, frequency, temperature.
// Supports Adreno (Qualcomm) and Mali (ARM/Samsung/MediaTek) via sysfs probing.
// Graceful fallback when GPU info is unavailable.

use std::sync::Mutex;

use serde_json::json;

use crate::distributed::tool_registry::StreamingTool;
use crate::distributed::types::ToolManifest;

use super::sysfs_utils::{
    find_thermal_zone, probe_glob, probe_path, read_sysfs, read_thermal_temp,
};

/// Detected GPU vendor and sysfs paths.
struct GpuPaths {
    vendor: String,
    usage_path: Option<String>,
    freq_path: Option<String>,
}

pub struct GpuInfoTool {
    /// Probed GPU paths, initialized on first read.
    paths: Mutex<Option<Option<GpuPaths>>>,
    /// Cached thermal zone path for GPU.
    gpu_thermal_zone: Mutex<Option<Option<String>>>,
}

impl GpuInfoTool {
    pub fn new() -> Self {
        Self {
            paths: Mutex::new(None),
            gpu_thermal_zone: Mutex::new(None),
        }
    }

    /// Probe GPU sysfs paths. Called once, result cached.
    fn probe_gpu(&self) -> Option<GpuPaths> {
        // === Adreno (Qualcomm) ===
        if std::path::Path::new("/sys/class/kgsl/kgsl-3d0").exists() {
            return Some(GpuPaths {
                vendor: "Adreno".to_string(),
                usage_path: probe_path(&["/sys/class/kgsl/kgsl-3d0/gpubusy"]),
                freq_path: probe_path(&[
                    "/sys/class/kgsl/kgsl-3d0/clock_mhz",
                    "/sys/class/kgsl/kgsl-3d0/gpuclk",
                ]),
            });
        }

        // === Mali (ARM — used by Samsung Exynos, MediaTek, etc.) ===
        // Try common Mali paths
        let mali_util = probe_glob("/sys/devices/platform/*/gpu/utilization")
            .or_else(|| probe_glob("/sys/devices/platform/*/mali/utilization"));
        let mali_freq = probe_glob("/sys/devices/platform/*/gpu/cur_freq")
            .or_else(|| probe_glob("/sys/devices/platform/*/mali/cur_freq"));

        if mali_util.is_some() || mali_freq.is_some() {
            return Some(GpuPaths {
                vendor: "Mali".to_string(),
                usage_path: mali_util,
                freq_path: mali_freq,
            });
        }

        // === Immortalis / other ARM GPU ===
        let alt_util = probe_glob("/sys/devices/platform/*/gpufreq/gpu_utilization");
        let alt_freq = probe_glob("/sys/devices/platform/*/gpufreq/gpufreq_cur_freq");
        if alt_util.is_some() || alt_freq.is_some() {
            return Some(GpuPaths {
                vendor: "GPU".to_string(),
                usage_path: alt_util,
                freq_path: alt_freq,
            });
        }

        None
    }

    fn get_paths(&self) -> Option<GpuPaths> {
        // This is a bit awkward because we can't return a reference to Mutex content.
        // We re-probe if needed, but cache the result.
        let mut cache = self.paths.lock().unwrap_or_else(|e| e.into_inner());
        if cache.is_none() {
            *cache = Some(self.probe_gpu());
        }
        // Clone the inner data to return
        cache.as_ref().unwrap().as_ref().map(|p| GpuPaths {
            vendor: p.vendor.clone(),
            usage_path: p.usage_path.clone(),
            freq_path: p.freq_path.clone(),
        })
    }

    fn read_usage(&self, paths: &GpuPaths) -> String {
        let path = match &paths.usage_path {
            Some(p) => p,
            None => return "N/A".to_string(),
        };

        let raw = read_sysfs(path);
        if raw.is_empty() {
            return "N/A".to_string();
        }

        // Adreno gpubusy format: "busy_time total_time" (both in microseconds)
        if raw.contains(' ') {
            let parts: Vec<&str> = raw.split_whitespace().collect();
            if parts.len() >= 2 {
                if let (Ok(busy), Ok(total)) = (parts[0].parse::<u64>(), parts[1].parse::<u64>()) {
                    if total > 0 {
                        let pct = (busy as f64 / total as f64) * 100.0;
                        return format!("{:.0}%", pct);
                    }
                }
            }
        }

        // Mali utilization format: "XX%" or just a number
        if let Ok(val) = raw.trim_end_matches('%').parse::<u64>() {
            return format!("{}%", val);
        }

        raw
    }

    fn read_freq(&self, paths: &GpuPaths) -> String {
        let path = match &paths.freq_path {
            Some(p) => p,
            None => return "N/A".to_string(),
        };

        let raw = read_sysfs(path);

        // clock_mhz is already in MHz
        if path.contains("clock_mhz") {
            if let Ok(mhz) = raw.parse::<u64>() {
                return format!("{}MHz", mhz);
            }
        }

        // gpuclk / cur_freq is typically in Hz or kHz
        if let Ok(freq) = raw.parse::<u64>() {
            if freq > 1_000_000 {
                // Hz → MHz
                return format!("{}MHz", freq / 1_000_000);
            } else if freq > 1_000 {
                // kHz → MHz
                return format!("{}MHz", freq / 1_000);
            } else {
                return format!("{}MHz", freq);
            }
        }

        "N/A".to_string()
    }

    fn read_temp(&self) -> String {
        let mut zone = self
            .gpu_thermal_zone
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if zone.is_none() {
            *zone = Some(find_thermal_zone("gpu"));
        }

        match zone.as_ref().unwrap() {
            Some(path) => read_thermal_temp(path),
            None => "N/A".to_string(),
        }
    }
}

impl StreamingTool for GpuInfoTool {
    fn manifest(&self) -> ToolManifest {
        ToolManifest {
            name: "MobileGPUInfo".to_string(),
            description: "移动设备GPU状态(使用率/频率/温度)".to_string(),
            parameters: json!({}),
            tool_type: "mobile".to_string(),
        }
    }

    fn placeholder_key(&self) -> &str {
        "{{MobileGPU}}"
    }

    fn poll_interval_secs(&self) -> u64 {
        15
    }

    fn read_current(&self) -> Result<String, String> {
        let paths = match self.get_paths() {
            Some(p) => p,
            None => {
                return Ok("GPU: 信息不可用(不支持此SoC或需root)".to_string());
            }
        };

        let usage = self.read_usage(&paths);
        let freq = self.read_freq(&paths);
        let temp = self.read_temp();

        Ok(format!(
            "GPU 使用率: {} | 频率: {} | 温度: {} | {}",
            usage, freq, temp, paths.vendor
        ))
    }
}

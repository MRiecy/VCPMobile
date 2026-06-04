// distributed/tools/sysfs_utils.rs
// Shared utilities for reading Android sysfs/procfs safely.
// All functions are non-panicking — return empty/default on failure.

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Safely read a sysfs/procfs file, returning trimmed content or empty string.
pub fn read_sysfs(path: &str) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// Read a sysfs file and parse as u64. Returns None on any failure.
pub fn read_sysfs_u64(path: &str) -> Option<u64> {
    read_sysfs(path).parse::<u64>().ok()
}

/// Scan /sys/class/thermal/thermal_zone*/type to find a zone whose type
/// contains the given keyword (case-insensitive). Returns the zone path
/// (e.g. "/sys/class/thermal/thermal_zone0") or None.
#[allow(dead_code)]
pub fn find_thermal_zone(keyword: &str) -> Option<String> {
    let keyword_lower = keyword.to_lowercase();
    let thermal_base = "/sys/class/thermal";

    let entries = match std::fs::read_dir(thermal_base) {
        Ok(e) => e,
        Err(_) => return None,
    };

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("thermal_zone") {
            continue;
        }
        let zone_path = format!("{}/{}", thermal_base, name);
        let type_path = format!("{}/type", zone_path);
        let zone_type = read_sysfs(&type_path).to_lowercase();
        if zone_type.contains(&keyword_lower) {
            return Some(zone_path);
        }
    }
    None
}

/// Format bytes into human-readable string (e.g. "5.2GB").
pub fn format_bytes(bytes: u64) -> String {
    const GB: u64 = 1024 * 1024 * 1024;
    const MB: u64 = 1024 * 1024;
    if bytes >= GB {
        format!("{:.1}GB", bytes as f64 / GB as f64)
    } else {
        format!("{:.0}MB", bytes as f64 / MB as f64)
    }
}

/// Format kB (from /proc/meminfo) into human-readable string.
pub fn format_kb(kb: u64) -> String {
    format_bytes(kb * 1024)
}

/// Self-throttling cache for low-frequency sensors.
/// Stores a cached value and only refreshes when the interval has elapsed.
pub struct ThrottledCache {
    interval: Duration,
    last_read: Mutex<Option<Instant>>,
    cached_value: Mutex<String>,
}

impl ThrottledCache {
    pub fn new(interval_secs: u64) -> Self {
        Self {
            interval: Duration::from_secs(interval_secs),
            last_read: Mutex::new(None),
            cached_value: Mutex::new(String::new()),
        }
    }

    /// Returns true if the cache is stale and needs refresh.
    pub fn needs_refresh(&self) -> bool {
        let last = self.last_read.lock().unwrap_or_else(|e| e.into_inner());
        match *last {
            None => true,
            Some(t) => t.elapsed() >= self.interval,
        }
    }

    /// Update the cached value and reset the timer.
    pub fn update(&self, value: String) {
        *self.cached_value.lock().unwrap_or_else(|e| e.into_inner()) = value;
        *self.last_read.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
    }

    /// Get the cached value (may be empty if never updated).
    pub fn get(&self) -> String {
        self.cached_value
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Get cached value, or refresh using the provided closure if stale.
    pub fn get_or_refresh<F: FnOnce() -> String>(&self, refresh_fn: F) -> String {
        if self.needs_refresh() {
            let value = refresh_fn();
            self.update(value);
        }
        self.get()
    }
}

/// 辅助方法：通过 JNI 接口尝试以 Root 执行命令并获取输出。
/// 如果获取失败、未授权或不是 Android 平台，返回 None
pub fn execute_root_command_safe(app: &tauri::AppHandle, command: &str) -> Option<String> {
    #[cfg(target_os = "android")]
    {
        use tauri::Manager;
        let result: Result<String, String> = (|| {
            let state = app.state::<tauri_plugin_vcp_mobile::VcpMobileState<tauri::Wry>>();
            let handle_guard = state.plugin_handle.lock().map_err(|e| e.to_string())?;
            let plugin_handle = handle_guard.as_ref().ok_or("VcpMobile plugin not initialized")?;
            
            #[derive(serde::Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct RootResult {
                success: bool,
                output: String,
            }
            
            let res = plugin_handle
                .run_mobile_plugin::<RootResult>(
                    "runRootCommand",
                    serde_json::json!({ "command": command }),
                )
                .map_err(|e| format!("JNI call failed: {}", e))?;

            if res.success && !res.output.trim().is_empty() {
                Ok(res.output)
            } else {
                Err("Root command returned failure or empty output".to_string())
            }
        })();

        result.ok()
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        let _ = command;
        None
    }
}

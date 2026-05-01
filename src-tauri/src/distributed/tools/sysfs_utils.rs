// distributed/tools/sysfs_utils.rs
// Shared utilities for reading Android sysfs/procfs safely.
// All functions are non-panicking — return empty/default on failure.

use std::path::Path;
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

/// Read a sysfs file and parse as i64. Returns None on any failure.
pub fn read_sysfs_i64(path: &str) -> Option<i64> {
    read_sysfs(path).parse::<i64>().ok()
}

/// Scan /sys/class/thermal/thermal_zone*/type to find a zone whose type
/// contains the given keyword (case-insensitive). Returns the zone path
/// (e.g. "/sys/class/thermal/thermal_zone0") or None.
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

/// Read temperature from a thermal zone path (milli-degrees Celsius → °C string).
/// Returns "N/A" on failure.
pub fn read_thermal_temp(zone_path: &str) -> String {
    match read_sysfs_i64(&format!("{}/temp", zone_path)) {
        Some(millideg) => format!("{}°C", millideg / 1000),
        None => "N/A".to_string(),
    }
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

/// Probe: try multiple paths, return the first one that exists.
pub fn probe_path(candidates: &[&str]) -> Option<String> {
    for path in candidates {
        if Path::new(path).exists() {
            return Some(path.to_string());
        }
    }
    None
}

/// Probe using glob-like patterns with a single `*` wildcard.
/// Returns the first existing path that matches.
pub fn probe_glob(pattern: &str) -> Option<String> {
    // Split pattern at '*'
    let parts: Vec<&str> = pattern.splitn(2, '*').collect();
    if parts.len() != 2 {
        // No wildcard, just check existence
        return if Path::new(pattern).exists() {
            Some(pattern.to_string())
        } else {
            None
        };
    }

    let (dir_prefix, suffix) = (parts[0], parts[1]);
    // Find the parent directory to scan
    let parent = Path::new(dir_prefix).parent()?;
    let prefix_name = Path::new(dir_prefix)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    let entries = match std::fs::read_dir(parent) {
        Ok(e) => e,
        Err(_) => return None,
    };

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with(&prefix_name) {
            let candidate = format!(
                "{}{}{}",
                dir_prefix.trim_end_matches(&prefix_name),
                name,
                suffix
            );
            if Path::new(&candidate).exists() {
                return Some(candidate);
            }
        }
    }
    None
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

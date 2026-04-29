// distributed/tools/memory_info.rs
// [Streaming] MobileMemoryInfo — RAM and Swap usage from /proc/meminfo

use serde_json::json;

use crate::distributed::tool_registry::StreamingTool;
use crate::distributed::types::ToolManifest;

use super::sysfs_utils::{format_kb, read_sysfs};

pub struct MemoryInfoTool;

/// Parsed fields from /proc/meminfo (values in kB)
struct MemInfo {
    mem_total: u64,
    mem_available: u64,
    swap_total: u64,
    swap_free: u64,
}

impl MemoryInfoTool {
    fn parse_meminfo(&self) -> Option<MemInfo> {
        let content = read_sysfs("/proc/meminfo");
        if content.is_empty() {
            return None;
        }

        let mut mem_total: Option<u64> = None;
        let mut mem_available: Option<u64> = None;
        let mut swap_total: Option<u64> = None;
        let mut swap_free: Option<u64> = None;

        for line in content.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 2 {
                continue;
            }
            let key = parts[0].trim_end_matches(':');
            let val: u64 = match parts[1].parse() {
                Ok(v) => v,
                Err(_) => continue,
            };

            match key {
                "MemTotal" => mem_total = Some(val),
                "MemAvailable" => mem_available = Some(val),
                "SwapTotal" => swap_total = Some(val),
                "SwapFree" => swap_free = Some(val),
                _ => {}
            }

            // Early exit once we have all fields
            if mem_total.is_some()
                && mem_available.is_some()
                && swap_total.is_some()
                && swap_free.is_some()
            {
                break;
            }
        }

        Some(MemInfo {
            mem_total: mem_total.unwrap_or(0),
            mem_available: mem_available.unwrap_or(0),
            swap_total: swap_total.unwrap_or(0),
            swap_free: swap_free.unwrap_or(0),
        })
    }
}

impl StreamingTool for MemoryInfoTool {
    fn manifest(&self) -> ToolManifest {
        ToolManifest {
            name: "MobileMemoryInfo".to_string(),
            description: "移动设备内存使用状态(总量/可用/Swap)".to_string(),
            parameters: json!({}),
            tool_type: "mobile".to_string(),
        }
    }

    fn placeholder_key(&self) -> &str {
        "{{MobileMemory}}"
    }

    fn poll_interval_secs(&self) -> u64 {
        15
    }

    fn read_current(&self) -> Result<String, String> {
        let info = match self.parse_meminfo() {
            Some(i) => i,
            None => return Ok("内存信息不可用".to_string()),
        };

        let used_kb = info.mem_total.saturating_sub(info.mem_available);
        let usage_pct = if info.mem_total > 0 {
            (used_kb as f64 / info.mem_total as f64 * 100.0) as u64
        } else {
            0
        };

        let mut result = format!(
            "内存: {} / {} ({}%已用) | 可用: {}",
            format_kb(used_kb),
            format_kb(info.mem_total),
            usage_pct,
            format_kb(info.mem_available),
        );

        if info.swap_total > 0 {
            let swap_used = info.swap_total.saturating_sub(info.swap_free);
            result.push_str(&format!(
                " | Swap: {}/{}",
                format_kb(swap_used),
                format_kb(info.swap_total),
            ));
        }

        Ok(result)
    }
}

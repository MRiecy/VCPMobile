// distributed/tools/storage_info.rs
// [Streaming] MobileStorageInfo — internal storage space via statvfs.
// Uses ThrottledCache (C2) to avoid redundant disk queries every 30s.

use serde_json::json;

use crate::distributed::tool_registry::StreamingTool;
use crate::distributed::types::ToolManifest;

#[cfg(unix)]
use super::sysfs_utils::format_bytes;
use super::sysfs_utils::ThrottledCache;

pub struct StorageInfoTool {
    cache: ThrottledCache,
}

impl StorageInfoTool {
    pub fn new() -> Self {
        Self {
            cache: ThrottledCache::new(300), // Refresh every 5 minutes
        }
    }

    /// Read storage info using libc::statvfs (Unix/Android only).
    #[cfg(unix)]
    fn read_storage(&self) -> String {
        let path = std::ffi::CString::new("/data")
            .unwrap_or_else(|_| std::ffi::CString::new("/").unwrap());

        unsafe {
            let mut stat: libc::statvfs = std::mem::zeroed();
            let ret = libc::statvfs(path.as_ptr(), &mut stat);
            if ret != 0 {
                return "存储信息不可用".to_string();
            }

            let block_size = stat.f_frsize as u64;
            let total = stat.f_blocks as u64 * block_size;
            let available = stat.f_bavail as u64 * block_size;
            let used = total.saturating_sub(available);

            let usage_pct = if total > 0 {
                (used as f64 / total as f64 * 100.0) as u64
            } else {
                0
            };

            format!(
                "内部存储: {} / {} ({}%已用)",
                format_bytes(used),
                format_bytes(total),
                usage_pct,
            )
        }
    }

    #[cfg(not(unix))]
    fn read_storage(&self) -> String {
        "存储信息不可用(非Unix平台)".to_string()
    }
}

impl StreamingTool for StorageInfoTool {
    fn manifest(&self) -> ToolManifest {
        ToolManifest {
            name: "MobileStorageInfo".to_string(),
            description: "移动设备存储空间使用状态".to_string(),
            parameters: json!({}),
            tool_type: "mobile".to_string(),
        }
    }

    fn placeholder_key(&self) -> &str {
        "{{MobileStorage}}"
    }

    fn poll_interval_secs(&self) -> u64 {
        300
    }

    fn read_current(&self) -> Result<String, String> {
        // C2: Self-throttle — only refresh every 300s even though called every 30s
        Ok(self.cache.get_or_refresh(|| self.read_storage()))
    }
}

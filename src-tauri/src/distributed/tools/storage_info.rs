// distributed/tools/storage_info.rs
// [Streaming] MobileStorageInfo — internal storage space via statvfs.
// Uses ThrottledCache (C2) to avoid redundant disk queries every 30s.

use crate::distributed::tool_registry::StreamingTool;
use crate::distributed::types::ToolManifest;



pub struct StorageInfoTool;

impl StorageInfoTool {
    pub fn new() -> Self {
        Self
    }
}

impl StreamingTool for StorageInfoTool {
    fn manifest(&self) -> ToolManifest {
        ToolManifest {
            name: "MobileStorageInfo".to_string(),
            description: "监控系统分区与用户分区的空闲/已用空间大小，预估磁盘健康度。".to_string(),
            display_name: "磁盘存储监控".to_string(),
            placeholder: Some("{{MobileStorage}}".to_string()),
            invocation_commands: vec![],
        }
    }

    fn placeholder_key(&self) -> &str {
        "{{MobileStorage}}"
    }

    fn poll_interval_secs(&self) -> u64 {
        300
    }

    fn read_current(&self, app: &tauri::AppHandle) -> Result<String, String> {
        use tauri::Manager;
        let dist_state = app.state::<crate::distributed::DistributedState>();
        let storage_info = dist_state.telemetry.get_storage_info();
        Ok(storage_info)
    }
}

// distributed/tools/memory_info.rs
// [Streaming] MobileMemoryInfo — RAM and Swap usage from /proc/meminfo

use crate::distributed::tool_registry::StreamingTool;
use crate::distributed::types::ToolManifest;

pub struct MemoryInfoTool;

impl StreamingTool for MemoryInfoTool {
    fn manifest(&self) -> ToolManifest {
        ToolManifest {
            name: "MobileMemoryInfo".to_string(),
            description: "监控总内存容量、当前可用空间及系统的虚拟内存/Swap 缓存分布。".to_string(),
            display_name: "内存监控".to_string(),
            placeholder: Some("{{MobileMemory}}".to_string()),
            invocation_commands: vec![],
        }
    }

    fn placeholder_key(&self) -> &str {
        "{{MobileMemory}}"
    }

    fn poll_interval_secs(&self) -> u64 {
        15
    }

    fn read_current(&self, app: &tauri::AppHandle) -> Result<String, String> {
        use tauri::Manager;
        let dist_state = app.state::<crate::distributed::DistributedState>();
        let memory_info = dist_state.telemetry.get_memory_info();
        Ok(memory_info)
    }
}

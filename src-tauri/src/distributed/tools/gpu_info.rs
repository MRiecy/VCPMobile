// distributed/tools/gpu_info.rs
// [Streaming] MobileGPUInfo — GPU chip information and status.
// Uses OpenGL ES renderer info via native Android JNI and Root for real-time load.

use crate::distributed::tool_registry::StreamingTool;
use crate::distributed::types::ToolManifest;

pub struct GpuInfoTool;

impl GpuInfoTool {
    pub fn new() -> Self {
        Self
    }

    /// Parse Adreno gpubusy: "busy_time total_time" (both in microseconds) into load percentage.
    #[allow(dead_code)]
    fn parse_adreno_gpubusy(&self, raw: &str) -> Option<u64> {
        let parts: Vec<&str> = raw.split_whitespace().collect();
        if parts.len() >= 2 {
            if let (Ok(busy), Ok(total)) = (parts[0].parse::<u64>(), parts[1].parse::<u64>()) {
                if total > 0 {
                    let pct = (busy as f64 / total as f64) * 100.0;
                    return Some(pct.round() as u64);
                }
            }
        }
        None
    }
}

impl StreamingTool for GpuInfoTool {
    fn manifest(&self) -> ToolManifest {
        ToolManifest {
            name: "MobileGPUInfo".to_string(),
            description: "显示 GPU 核心渲染器厂商、显存使用率及 API 性能指标。".to_string(),
            display_name: "GPU算力监控".to_string(),
            placeholder: Some("{{MobileGPU}}".to_string()),
            invocation_commands: vec![],
        }
    }

    fn placeholder_key(&self) -> &str {
        "{{MobileGPU}}"
    }

    fn poll_interval_secs(&self) -> u64 {
        15
    }

    fn read_current(&self, app: &tauri::AppHandle) -> Result<String, String> {
        use tauri::Manager;
        let dist_state = app.state::<crate::distributed::DistributedState>();
        let gpu_info = dist_state.telemetry.get_gpu_info(app);
        Ok(gpu_info)
    }
}

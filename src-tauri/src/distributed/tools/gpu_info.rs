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
        #[cfg(target_os = "android")]
        {
            use tauri::Manager;
            // 1. 静态获取 GPU 型号 (OpenGL ES glGetString)
            let gpu_renderer = (|| -> Result<String, String> {
                let state = app.state::<tauri_plugin_vcp_mobile::VcpMobileState<tauri::Wry>>();
                let handle_guard = state.plugin_handle.lock().map_err(|e| e.to_string())?;
                let plugin_handle = handle_guard.as_ref().ok_or("VcpMobile plugin not initialized")?;

                #[derive(serde::Deserialize)]
                struct GpuResponse {
                    renderer: String,
                }
                
                let res = plugin_handle
                    .run_mobile_plugin::<GpuResponse>(
                        "getGpuStatus",
                        serde_json::json!({}),
                    )
                    .map_err(|e| format!("JNI call failed: {}", e))?;
                
                Ok(res.renderer)
            })().unwrap_or_else(|_| "Unknown GPU".to_string());

            // 2. 尝试利用 Root 提权获取实时 GPU 负载
            // === Adreno gpubusy ===
            if let Some(raw_busy) = super::sysfs_utils::execute_root_command_safe(app, "cat /sys/class/kgsl/kgsl-3d0/gpubusy") {
                if let Some(load) = self.parse_adreno_gpubusy(&raw_busy) {
                    return Ok(format!("GPU: {} | 使用率: {}%", gpu_renderer, load));
                }
            }

            // === Mali utilization fallback (best effort root) ===
            // Try common paths
            let mali_paths = [
                "cat /sys/devices/platform/14ac0000.mali/utilization",
                "cat /sys/devices/platform/14ac0000.mali/mali/utilization",
                "cat /sys/devices/platform/gpu/utilization",
                "cat /sys/devices/platform/mali/utilization",
            ];
            for path in &mali_paths {
                if let Some(raw_busy) = super::sysfs_utils::execute_root_command_safe(app, path) {
                    let clean = raw_busy.trim().trim_end_matches('%');
                    if let Ok(load) = clean.parse::<u64>() {
                        return Ok(format!("GPU: {} | 使用率: {}%", gpu_renderer, load));
                    }
                }
            }

            // 3. 无 Root 权限，降级退回显示“受系统安全限制”
            Ok(format!("GPU: {} | 实时负载: 受系统安全限制", gpu_renderer))
        }
        #[cfg(not(target_os = "android"))]
        {
            let _ = app;
            Ok("GPU: 信息不可用".to_string())
        }
    }
}

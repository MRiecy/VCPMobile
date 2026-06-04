// distributed/tools/network_info.rs
// [Streaming] MobileNetworkInfo — network type, IP, traffic stats.


use crate::distributed::tool_registry::StreamingTool;
use crate::distributed::types::ToolManifest;

pub struct NetworkInfoTool;



impl StreamingTool for NetworkInfoTool {
    fn manifest(&self) -> ToolManifest {
        ToolManifest {
            name: "MobileNetworkInfo".to_string(),
            description: "检测当前连接网络介质（WIFI/蜂窝）、局域网 IP、延迟及当前吞吐速度。".to_string(),
            display_name: "网络带宽监控".to_string(),
            placeholder: Some("{{MobileNetwork}}".to_string()),
            invocation_commands: vec![],
        }
    }

    fn placeholder_key(&self) -> &str {
        "{{MobileNetwork}}"
    }

    fn poll_interval_secs(&self) -> u64 {
        30
    }

    fn read_current(&self, app: &tauri::AppHandle) -> Result<String, String> {
        #[cfg(target_os = "android")]
        {
            use tauri::Manager;
            let state = app.state::<tauri_plugin_vcp_mobile::VcpMobileState<tauri::Wry>>();
            let handle_guard = state.plugin_handle.lock().map_err(|e| e.to_string())?;
            let plugin_handle = handle_guard.as_ref().ok_or("VcpMobile plugin not initialized")?;
            
            #[derive(serde::Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct NetworkResponse {
                connected: bool,
                r#type: String,
                down_speed_kbps: i32,
                up_speed_kbps: i32,
                ip: String,
            }
            
            let res = plugin_handle
                .run_mobile_plugin::<NetworkResponse>(
                    "getNetworkStatus",
                    serde_json::json!({}),
                )
                .map_err(|e| format!("JNI call failed: {}", e))?;

            if !res.connected {
                return Ok("网络: 未连接".to_string());
            }

            // 对带宽速度做人性化换算
            let format_speed = |kbps: i32| -> String {
                if kbps >= 1000 {
                    format!("{:.1}Mbps", kbps as f64 / 1000.0)
                } else {
                    format!("{}Kbps", kbps)
                }
            };
            let down_str = format_speed(res.down_speed_kbps);
            let up_str = format_speed(res.up_speed_kbps);

            Ok(format!(
                "类型: {} | IP: {} | 下行: {} | 上行: {}",
                res.r#type, res.ip, down_str, up_str
            ))
        }
        #[cfg(not(target_os = "android"))]
        {
            let _ = app;
            Ok("网络: 未连接".to_string())
        }
    }
}

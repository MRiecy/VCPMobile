// distributed/tools/network_info.rs
// [Streaming] MobileNetworkInfo — network type, IP, traffic stats.

use crate::distributed::tool_registry::StreamingTool;
use crate::distributed::types::ToolManifest;

pub struct NetworkInfoTool;

impl StreamingTool for NetworkInfoTool {
    fn manifest(&self) -> ToolManifest {
        ToolManifest {
            name: "MobileNetworkInfo".to_string(),
            description: "检测当前连接网络介质（WIFI/蜂窝）、局域网 IP、延迟及当前吞吐速度。"
                .to_string(),
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
        use tauri::Manager;
        let dist_state = app.state::<crate::distributed::DistributedState>();
        let network_info = dist_state.telemetry.get_network_info(app);
        Ok(network_info)
    }
}

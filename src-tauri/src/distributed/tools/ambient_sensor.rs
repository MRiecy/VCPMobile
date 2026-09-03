// distributed/tools/ambient_sensor.rs
// [Streaming] MobileAmbient — ambient light and barometer from Android native sensors.

use tauri::AppHandle;

use crate::distributed::streaming_scheduler::SensorDemand;
use crate::distributed::tool_registry::StreamingTool;
use crate::distributed::types::ToolManifest;

pub struct AmbientSensorTool;

impl StreamingTool for AmbientSensorTool {
    fn manifest(&self) -> ToolManifest {
        ToolManifest {
            name: "MobileAmbient".to_string(),
            description: "读取设备所处的物理环境光照度 (Lux) 与气压值 (hPa)，推算环境场景。"
                .to_string(),
            display_name: "物理环境传感器".to_string(),
            placeholder: Some("{{MobileAmbient}}".to_string()),
            invocation_commands: vec![],
        }
    }

    fn placeholder_key(&self) -> &str {
        "{{MobileAmbient}}"
    }

    fn poll_interval_secs(&self) -> u64 {
        60
    }

    fn sensor_demand(&self) -> SensorDemand {
        SensorDemand::AMBIENT
    }

    fn read_current(&self, app: &AppHandle) -> Result<String, String> {
        use tauri::Manager;
        let dist_state = app.state::<crate::distributed::DistributedState>();
        let ambient = dist_state.telemetry.get_ambient_info(app);
        Ok(ambient)
    }
}

// distributed/tools/mod.rs
// Tool registration. Add new tools here.
// To add a tool: 1) create the .rs file, 2) add `mod` + `use`, 3) register in build_registry().

mod clipboard;
mod device_info;
mod notification;

mod sysfs_utils;
mod battery;
mod cpu_info;
mod gpu_info;
mod memory_info;
mod network_info;
mod storage_info;

pub(crate) mod frontend_bridge;
mod location;
mod motion_sensor;
mod ambient_sensor;
mod device_status_summary;

use super::tool_registry::ToolRegistry;

/// Build the tool registry with all mobile-native tools.
/// Called once on distributed node startup.
pub fn build_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();

    // OneShot tools
    registry.register_oneshot(device_info::DeviceInfoTool);
    registry.register_oneshot(notification::NotificationTool);
    registry.register_oneshot(clipboard::ClipboardTool);

    // Streaming tools — hardware monitoring (Phase 1)
    registry.register_streaming(battery::BatteryInfoTool);
    registry.register_streaming(memory_info::MemoryInfoTool);
    registry.register_streaming(cpu_info::CpuInfoTool::new());
    registry.register_streaming(gpu_info::GpuInfoTool::new());
    registry.register_streaming(network_info::NetworkInfoTool);
    registry.register_streaming(storage_info::StorageInfoTool::new());

    // Streaming tools — frontend-cooperative sensors (Phase 2)
    registry.register_streaming(location::LocationTool);
    registry.register_streaming(motion_sensor::MotionSensorTool);
    registry.register_streaming(ambient_sensor::AmbientSensorTool);

    // Streaming tools — aggregation (Phase 3)
    registry.register_streaming(device_status_summary::DeviceStatusSummaryTool);

    // Interactive tools — future phases:
    // registry.register_interactive(photo_capture::PhotoCaptureTool);

    log::info!(
        "[Distributed] Tool registry built: {} tools registered.",
        registry.tool_count()
    );

    registry
}

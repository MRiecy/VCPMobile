// distributed/tools/mod.rs
// Tool registration. Add new tools here.
// To add a tool: 1) create the .rs file, 2) add `mod` + `use`, 3) register in build_registry().

mod clipboard;
mod device_info;
mod notification;

use super::tool_registry::ToolRegistry;

/// Build the tool registry with all mobile-native tools.
/// Called once on distributed node startup.
pub fn build_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();

    // OneShot tools
    registry.register_oneshot(device_info::DeviceInfoTool);
    registry.register_oneshot(notification::NotificationTool);
    registry.register_oneshot(clipboard::ClipboardTool);

    // Interactive tools — Phase 3 will add:
    // registry.register_interactive(photo_capture::PhotoCaptureTool);
    // registry.register_interactive(biometric_auth::BiometricAuthTool);
    // registry.register_interactive(system_notes::SystemNotesTool);

    // Streaming tools — Phase 3 will add:
    // registry.register_streaming(sensor_stream::GyroscopeTool);
    // registry.register_streaming(sensor_stream::AccelerometerTool);

    log::info!(
        "[Distributed] Tool registry built: {} tools registered.",
        registry.tool_count()
    );

    registry
}

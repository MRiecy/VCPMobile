use tauri::{
    plugin::{Builder, TauriPlugin},
    Manager, Runtime,
};

mod screen;
pub mod stream;

/// Plugin state shared across commands
pub struct VcpMobileState {
    pub streaming_count: std::sync::atomic::AtomicU32,
}

impl Default for VcpMobileState {
    fn default() -> Self {
        Self {
            streaming_count: std::sync::atomic::AtomicU32::new(0),
        }
    }
}

/// Initializes the VCP Mobile plugin.
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("vcp-mobile")
        .invoke_handler(tauri::generate_handler![
            screen::set_keep_screen_on,
            screen::clear_keep_screen_on,
            stream::start_stream_service,
            stream::stop_stream_service,
        ])
        .setup(|app, _api| {
            app.manage(VcpMobileState::default());

            #[cfg(target_os = "android")]
            {
                _api.register_android_plugin("com.vcp.mobile", "VcpMobilePlugin")?;
            }

            Ok(())
        })
        .build()
}

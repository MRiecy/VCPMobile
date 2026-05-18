use tauri::{
    plugin::{Builder, TauriPlugin},
    Manager, Runtime,
};

mod screen;
pub mod stream;

/// Plugin state shared across commands
pub struct VcpMobileState<R: Runtime> {
    pub streaming_count: std::sync::atomic::AtomicU32,
    #[cfg(target_os = "android")]
    pub plugin_handle: std::sync::Mutex<Option<tauri::plugin::PluginHandle<R>>>,
    #[cfg(not(target_os = "android"))]
    _marker: std::marker::PhantomData<fn() -> R>,
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
            #[cfg(target_os = "android")]
            let plugin_handle = _api.register_android_plugin("com.vcp.mobile", "VcpMobilePlugin")?;

            app.manage(VcpMobileState::<R> {
                streaming_count: std::sync::atomic::AtomicU32::new(0),
                #[cfg(target_os = "android")]
                plugin_handle: std::sync::Mutex::new(Some(plugin_handle)),
                #[cfg(not(target_os = "android"))]
                _marker: std::marker::PhantomData,
            });

            Ok(())
        })
        .build()
}

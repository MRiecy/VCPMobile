use tauri::{
    plugin::{Builder, TauriPlugin},
    Manager, Runtime,
};

mod screen;
pub mod stream;
pub mod system;

/// Plugin state shared across commands
pub struct VcpMobileState<R: Runtime> {
    pub active_streams: std::sync::Mutex<Vec<(String, u32)>>,
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
            stream::start_streaming_service,
            stream::stop_streaming_service,
            system::check_all_permissions,
            system::request_android_permission,
            system::move_task_to_back,
            system::pick_file,
            system::get_battery_status,
            system::get_network_status,
            system::open_file_native,
            system::capture_window_snapshot,
            system::save_image_to_gallery,
            system::save_image_from_path,
            system::write_temp_file,
            system::start_download_notification,
            system::update_download_notification,
            system::cancel_download_notification,
            system::request_overlay_permission,
            system::register_shared_files,
            system::toggle_floating_ball,
            system::start_sensor_collection,
            system::stop_sensor_collection,
            system::get_sensor_data,
            system::get_cpu_thermal_status,
            system::get_gpu_status,
            system::check_root_access,
            system::run_root_command,
            system::launch_root_manager,
        ])
        .setup(|app, _api| {
            #[cfg(target_os = "android")]
            let plugin_handle =
                _api.register_android_plugin("com.vcp.mobile", "VcpMobilePlugin")?;

            app.manage(VcpMobileState::<R> {
                active_streams: std::sync::Mutex::new(Vec::new()),
                #[cfg(target_os = "android")]
                plugin_handle: std::sync::Mutex::new(Some(plugin_handle)),
                #[cfg(not(target_os = "android"))]
                _marker: std::marker::PhantomData,
            });

            Ok(())
        })
        .build()
}

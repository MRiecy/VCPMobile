use tauri::{
    plugin::{Builder, TauriPlugin},
    Manager, Runtime,
};

pub mod cli;
pub mod events;
#[cfg(any(target_os = "ios", test))]
mod ios;
mod screen;
pub mod stream;
pub mod system;
pub mod voice;
mod webview;

#[cfg(target_os = "ios")]
tauri::ios_plugin_binding!(init_plugin_vcp_mobile);

// iOS adapters are introduced phase by phase. A successful host fixture must
// never be interpreted as a completed native operation on an iPhone or iPad.
#[cfg(target_os = "ios")]
fn ios_not_implemented<T>(operation: &str) -> Result<T, String> {
    Err(format!(
        "not_implemented: {operation} has no iOS adapter yet"
    ))
}

#[cfg(target_os = "ios")]
fn ios_unsupported<T>(operation: &str) -> Result<T, String> {
    Err(format!("unsupported: {operation} is not supported on iOS"))
}

/// Plugin state shared across commands
pub struct VcpMobileState<R: Runtime> {
    #[cfg(any(target_os = "android", target_os = "ios"))]
    plugin_handle: std::sync::Mutex<Option<tauri::plugin::PluginHandle<R>>>,
    #[cfg(target_os = "ios")]
    ios_event_registration: std::sync::Arc<std::sync::atomic::AtomicBool>,
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    _marker: std::marker::PhantomData<fn() -> R>,
}

impl<R: Runtime> VcpMobileState<R> {
    #[cfg(any(target_os = "android", target_os = "ios"))]
    pub fn mobile_plugin_handle(&self) -> Result<tauri::plugin::PluginHandle<R>, String> {
        self.plugin_handle
            .lock()
            .map_err(|e| e.to_string())?
            .as_ref()
            .cloned()
            .ok_or_else(|| "Plugin handle not initialized".to_string())
    }
}

/// Initializes the VCP Mobile plugin.
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("vcp-mobile")
        .invoke_handler(tauri::generate_handler![
            screen::set_keep_screen_on,
            screen::clear_keep_screen_on,
            stream::acquire_foreground,
            stream::release_foreground,
            stream::start_helper_service,
            system::get_notification_permission,
            system::request_notification_permission,
            system::open_notification_settings,
            system::check_all_permissions,
            system::request_android_permission,
            system::move_task_to_back,
            system::check_notification_listener_permission,
            system::request_notification_listener_permission,
            system::request_auto_start_permission,
            system::request_power_management_permission,
            system::check_auto_start_permission,
            system::get_free_disk_space,
            system::pick_file,
            system::prepare_file_operation,
            system::cancel_file_operation,
            system::get_webview_geometry,
            webview::connect_webview_resume,
            webview::complete_webview_resume,
            system::get_battery_status,
            system::get_network_status,
            system::open_file_native,
            system::capture_window_snapshot,
            system::save_image_to_gallery,
            system::save_image_from_path,
            system::save_to_downloads,
            system::write_temp_file,
            system::delete_temp_file,
            system::start_download_notification,
            system::update_download_notification,
            system::cancel_download_notification,
            system::get_pending_notification,
            system::register_shared_files,
            system::get_cpu_thermal_status,
            system::get_gpu_status,
            system::check_root_access,
            system::run_root_command,
            system::launch_root_manager,
            system::acquire_wake_lock,
            system::release_wake_lock,
            system::start_network_monitoring,
            system::verify_apk_signature,
            system::can_install_packages,
            system::open_unknown_sources_settings,
            system::acquire_ota_keepalive,
            system::release_ota_keepalive,
            system::trigger_haptic_feedback,
            voice::start_voice_audio,
            voice::stop_voice_audio,
            voice::set_voice_probe,
        ])
        .setup(|app, _api| {
            app.manage(events::ServiceFailureNotices::default());
            app.manage(voice::VoiceAudioState::default());
            #[cfg(target_os = "android")]
            let plugin_handle =
                _api.register_android_plugin("com.vcp.mobile", "VcpMobilePlugin")?;
            #[cfg(target_os = "ios")]
            let plugin_handle = _api.register_ios_plugin(init_plugin_vcp_mobile)?;

            app.manage(VcpMobileState::<R> {
                #[cfg(any(target_os = "android", target_os = "ios"))]
                plugin_handle: std::sync::Mutex::new(Some(plugin_handle)),
                #[cfg(target_os = "ios")]
                ios_event_registration: Default::default(),
                #[cfg(not(any(target_os = "android", target_os = "ios")))]
                _marker: std::marker::PhantomData,
            });

            Ok(())
        })
        .build()
}

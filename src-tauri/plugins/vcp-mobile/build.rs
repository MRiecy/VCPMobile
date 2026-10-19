const COMMANDS: &[&str] = &[
    "set_keep_screen_on",
    "clear_keep_screen_on",
    "acquire_foreground",
    "release_foreground",
    "start_helper_service",
    "get_notification_permission",
    "request_notification_permission",
    "open_notification_settings",
    "check_all_permissions",
    "request_android_permission",
    "move_task_to_back",
    "check_notification_listener_permission",
    "request_notification_listener_permission",
    "request_auto_start_permission",
    "request_power_management_permission",
    "check_auto_start_permission",
    "get_free_disk_space",
    "pick_file",
    "prepare_file_operation",
    "cancel_file_operation",
    "get_webview_geometry",
    "connect_webview_resume",
    "complete_webview_resume",
    "get_battery_status",
    "get_network_status",
    "open_file_native",
    "capture_window_snapshot",
    "save_image_to_gallery",
    "save_image_from_path",
    "save_to_downloads",
    "write_temp_file",
    "delete_temp_file",
    "start_download_notification",
    "update_download_notification",
    "cancel_download_notification",
    "get_pending_notification",
    "register_shared_files",
    "get_cpu_thermal_status",
    "get_gpu_status",
    "check_root_access",
    "run_root_command",
    "launch_root_manager",
    "acquire_wake_lock",
    "release_wake_lock",
    "start_network_monitoring",
    "verify_apk_signature",
    "can_install_packages",
    "open_unknown_sources_settings",
    "acquire_ota_keepalive",
    "release_ota_keepalive",
    "trigger_haptic_feedback",
    "begin_voice_call",
    "end_voice_call",
    "start_voice_audio",
    "stop_voice_audio",
    "set_voice_probe",
];

fn main() {
    tauri_plugin::Builder::new(COMMANDS)
        .android_path("android")
        .ios_path("ios")
        .build();

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("ios") {
        // swift-rs links the plugin's SwiftPM static product, which does not
        // absorb binaryTarget archives. Link the custom ORT core explicitly.
        let runtime = std::path::PathBuf::from(
            std::env::var_os("CARGO_MANIFEST_DIR").expect("Cargo sets CARGO_MANIFEST_DIR"),
        )
        .join("ios/Runtime");
        let archive = runtime.join("libonnxruntime.a");
        assert!(
            archive.is_file(),
            "Build the custom iOS voice runtime with runtime-assets/voice/build_runtime.py ios before Cargo"
        );
        println!("cargo:rerun-if-changed={}", archive.display());
        println!("cargo:rustc-link-search=native={}", runtime.display());
        println!("cargo:rustc-link-lib=static=onnxruntime");
        // CPU MLAS uses its own kernels. The upstream CPU build needs
        // Foundation and libc++; no Accelerate/CoreML provider is enabled.
        println!("cargo:rustc-link-lib=c++");
        println!("cargo:rustc-link-lib=objc");
        println!("cargo:rustc-link-lib=framework=Foundation");
    }
}

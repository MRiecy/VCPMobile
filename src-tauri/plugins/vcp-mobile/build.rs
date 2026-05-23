const COMMANDS: &[&str] = &[
    "set_keep_screen_on",
    "clear_keep_screen_on",
    "start_stream_service",
    "stop_stream_service",
    "check_all_permissions",
    "request_android_permission",
    "move_task_to_back",
    "pick_file",
];

fn main() {
    tauri_plugin::Builder::new(COMMANDS)
        .android_path("android")
        .build();
}

const COMMANDS: &[&str] = &[
    "set_keep_screen_on",
    "clear_keep_screen_on",
    "start_stream_service",
    "stop_stream_service",
];

fn main() {
    tauri_plugin::Builder::new(COMMANDS)
        .android_path("android")
        .build();
}

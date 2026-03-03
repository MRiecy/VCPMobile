mod vcp_modules;

use vcp_modules::vcp_client::{ActiveRequests, sendToVCP, interruptRequest};
use vcp_modules::context_sanitizer::SanitizerState;

// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(ActiveRequests::default())
        .manage(SanitizerState::new(100))
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            greet,
            sendToVCP,
            interruptRequest
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

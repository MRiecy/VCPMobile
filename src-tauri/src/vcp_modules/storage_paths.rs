use std::path::PathBuf;
use tauri::{AppHandle, Manager, Runtime};

/// 获取 AppData/AgentGroups 目录
#[allow(dead_code)]
pub fn get_groups_base_path<R: Runtime>(app: &AppHandle<R>) -> PathBuf {
    let mut path = app
        .path()
        .app_config_dir()
        .unwrap_or_else(|_| PathBuf::from("AppData"));
    path.push("AgentGroups");
    path
}

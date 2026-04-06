use std::path::PathBuf;
use tauri::{AppHandle, Manager, Runtime};

/// 获取 AppData/AgentGroups 目录
pub fn get_groups_base_path<R: Runtime>(app: &AppHandle<R>) -> PathBuf {
    let mut path = app
        .path()
        .app_config_dir()
        .unwrap_or_else(|_| PathBuf::from("AppData"));
    path.push("AgentGroups");
    path
}

/// 物理探测: 判定 ID 是否属于群组 (不推荐使用，应通过数据库 owner_type 判断)
pub fn is_group_item<R: Runtime>(_app: &AppHandle<R>, item_id: &str) -> bool {
    item_id.starts_with("____")
}

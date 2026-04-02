use log::info;
use std::path::PathBuf;
use tauri::{AppHandle, Manager, Runtime};

/// 获取 AppData/Agents 目录 (注意: A 大写以对齐桌面端)
pub fn get_agents_base_path<R: Runtime>(app: &AppHandle<R>) -> PathBuf {
    let mut path = app
        .path()
        .app_config_dir()
        .unwrap_or_else(|_| PathBuf::from("AppData"));
    path.push("Agents");
    path
}

/// 获取 AppData/AgentGroups 目录
pub fn get_groups_base_path<R: Runtime>(app: &AppHandle<R>) -> PathBuf {
    let mut path = app
        .path()
        .app_config_dir()
        .unwrap_or_else(|_| PathBuf::from("AppData"));
    path.push("AgentGroups");
    path
}

/// 物理探测: 判定 ID 是否属于群组
pub fn is_group_item<R: Runtime>(app: &AppHandle<R>, item_id: &str) -> bool {
    get_groups_base_path(app).join(item_id).exists()
}

/// 获取话题目录路径 (支持双轨结构与 UserData/data 兼容)
pub fn resolve_topic_dir<R: Runtime>(app: &AppHandle<R>, item_id: &str, topic_id: &str) -> PathBuf {
    let config_dir = app
        .path()
        .app_config_dir()
        .unwrap_or_else(|_| PathBuf::from("AppData"));

    // 兼容性探测：优先 data (移动端同步标准)，次选 UserData (桌面端标准)
    let mut path = config_dir.join("data");
    if !path.exists() {
        let alt_path = config_dir.join("UserData");
        if alt_path.exists() {
            path = alt_path;
        }
    }

    path.push(item_id);
    path.push("topics");

    // 优化前缀处理逻辑：
    // 1. 如果 topic_id 已经包含 "group_" 前缀（新版逻辑），直接使用
    // 2. 如果不包含前缀，则通过 is_group_item 进行物理探测（兼容旧版逻辑）
    if !topic_id.starts_with("group_") && is_group_item(app, item_id) {
        path.push(format!("group_{}", topic_id));
    } else {
        path.push(topic_id);
    }
    path
}

/// 修改后的历史记录路径获取逻辑 (支持双轨结构)
pub fn resolve_history_path<R: Runtime>(
    app: &AppHandle<R>,
    item_id: &str,
    topic_id: &str,
) -> PathBuf {
    let mut path = resolve_topic_dir(app, item_id, topic_id);
    path.push("history.json");
    info!(
        "[PathTopology] Resolved history path for {}/{}: {:?}",
        item_id, topic_id, path
    );
    path
}

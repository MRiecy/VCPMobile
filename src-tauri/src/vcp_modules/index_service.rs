use log::{error, info};
use sqlx::{Pool, Sqlite};
use std::path::Path;
use tauri::{AppHandle, Manager};
use walkdir::WalkDir;

pub async fn full_scan(app_handle: &AppHandle, pool: &Pool<Sqlite>) -> Result<(), String> {
    let config_dir = app_handle
        .path()
        .app_config_dir()
        .map_err(|e: tauri::Error| e.to_string())?;

    // 兼容性扫描：同时支持 UserData (桌面端) 和 data (移动端同步)
    let search_dirs = [config_dir.join("UserData"), config_dir.join("data")];

    for data_dir in search_dirs {
        if !data_dir.exists() {
            continue;
        }

        info!("[IndexService] Starting background scan: {:?}", data_dir);

        for entry in WalkDir::new(&data_dir)
            .max_depth(4)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            if path.file_name().and_then(|n| n.to_str()) == Some("history.json") {
                if let Err(e) = index_history_file(app_handle, &config_dir, path, pool).await {
                    error!("[IndexService] Failed to index {:?}: {}", path, e);
                }
            }
        }
    }

    info!("[IndexService] Background scan completed.");
    Ok(())
}

pub async fn index_history_file(
    _app_handle: &AppHandle,
    _app_config_dir: &Path,
    path: &Path,
    _pool: &Pool<Sqlite>,
) -> Result<(), String> {
    info!("[IndexService] History file found at {:?}, but history-to-DB sync is now handled by SyncDaemon or manual migration.", path);
    Ok(())
}

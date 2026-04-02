use std::fs;
use tauri::{AppHandle, Runtime};

use crate::vcp_modules::group_cache_coordinator::GroupManagerState;
use crate::vcp_modules::group_config_repository_fs::read_group_config as read_group_config_fs;
use crate::vcp_modules::path_topology_service::get_groups_base_path;

/// 加载所有群组配置到缓存，并同步话题索引到数据库
pub async fn load_all_groups<R: Runtime>(
    app: &AppHandle<R>,
    state: &GroupManagerState,
    db: &sqlx::Pool<sqlx::Sqlite>,
) -> Result<(), String> {
    let base_path = get_groups_base_path(app);
    if !base_path.exists() {
        fs::create_dir_all(&base_path).map_err(|e| e.to_string())?;
        return Ok(());
    }

    let entries = fs::read_dir(base_path).map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let group_id = path.file_name().unwrap_or_default().to_string_lossy();
            if let Ok(config) = read_group_config_fs(app, &group_id) {
                // 同步话题到数据库
                for topic in &config.topics {
                    let exists: bool = sqlx::query("SELECT 1 FROM topic_index WHERE topic_id = ?")
                        .bind(&topic.id)
                        .fetch_optional(db)
                        .await
                        .map_err(|e| e.to_string())?
                        .is_some();

                    if !exists {
                        sqlx::query(
                            "INSERT INTO topic_index (topic_id, agent_id, title, mtime, locked, unread, unread_count, last_msg_preview, msg_count)
                             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)"
                        )
                        .bind(&topic.id)
                        .bind(&config.id)
                        .bind(&topic.name)
                        .bind(topic.created_at)
                        .bind(topic.locked)
                        .bind(topic.unread)
                        .bind(topic.unread_count)
                        .bind(&topic.last_msg_preview)
                        .bind(topic.msg_count)
                        .execute(db)
                        .await
                        .map_err(|e| e.to_string())?;
                    }
                }

                state.insert_group(config);
            }
        }
    }

    println!(
        "[GroupBootstrapLoader] Loaded {} groups and synced topics to DB.",
        state.get_all_groups().len()
    );
    Ok(())
}

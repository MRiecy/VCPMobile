use hex;
use log::{error, info, warn};
use sha2::{Digest, Sha256};
use sqlx::{Pool, Sqlite};
use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;
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

        info!("[IndexService] Scanning directory: {:?}", data_dir);

        for entry in WalkDir::new(&data_dir)
            .max_depth(4)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            if path.file_name().and_then(|n| n.to_str()) == Some("history.json") {
                if let Err(e) = index_history_file(&config_dir, path, pool).await {
                    error!("[IndexService] Failed to index {:?}: {}", path, e);
                }
            }
        }
    }

    Ok(())
}

pub async fn index_history_file(
    app_config_dir: &Path,
    path: &Path,
    pool: &Pool<Sqlite>,
) -> Result<(), String> {
    let metadata = fs::metadata(path).map_err(|e| e.to_string())?;
    let mtime = metadata
        .modified()
        .map_err(|e| e.to_string())?
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

    // Extract agent_id and topic_id from path: .../{data|UserData}/{agent_id}/topics/{topic_id}/history.json
    let components: Vec<_> = path.components().collect();
    if components.len() < 4 {
        return Ok(());
    }

    let topic_id = match components[components.len() - 2].as_os_str().to_str() {
        Some(s) => s.to_string(),
        None => return Ok(()),
    };
    let agent_id = match components[components.len() - 4].as_os_str().to_str() {
        Some(s) => s.to_string(),
        None => return Ok(()),
    };

    // 智能探测 Config 路径：普通智能体 vs 群组
    // 普通智能体: app_config_dir/agents/{agent_id}/config.json
    // 群组: app_config_dir/AgentGroups/{agent_id}/config.json
    let config_path = if agent_id.starts_with("____") || agent_id.starts_with("___N_P_") {
        app_config_dir
            .join("AgentGroups")
            .join(&agent_id)
            .join("config.json")
    } else {
        app_config_dir
            .join("agents")
            .join(&agent_id)
            .join("config.json")
    };

    let mut locked = false;
    let mut unread = false;
    let mut title_from_config = None;

    if config_path.exists() {
        if let Ok(config_content) = tokio::fs::read_to_string(&config_path).await {
            if let Ok(config_json) = serde_json::from_str::<serde_json::Value>(&config_content) {
                // Find the correct topic metadata in the "topics" array
                if let Some(topics) = config_json.get("topics").and_then(|t| t.as_array()) {
                    for t in topics {
                        if t.get("id").and_then(|id| id.as_str()) == Some(&topic_id) {
                            locked = t.get("locked").and_then(|v| v.as_bool()).unwrap_or(false);
                            unread = t.get("unread").and_then(|v| v.as_bool()).unwrap_or(false);
                            // 优先取 name, 次选 title
                            title_from_config = t
                                .get("name")
                                .or(t.get("title"))
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string());
                            break;
                        }
                    }
                }
            }
        }
    } else {
        warn!(
            "[IndexService] Config not found for {}, topic {}. Path attempted: {:?}",
            agent_id, topic_id, config_path
        );
    }

    // Check if we need to re-index based on mtime, and retain the old title if any
    let existing: Option<(i64, Option<String>)> =
        sqlx::query_as("SELECT mtime, title FROM topic_index WHERE topic_id = ?")
            .bind(&topic_id)
            .fetch_optional(pool)
            .await
            .map_err(|e| e.to_string())?;

    let mut existing_title = None;
    if let Some((old_mtime, old_title)) = existing {
        if old_mtime >= mtime {
            // 如果已存在且 mtime 没变，但我们从 config 拿到了更好的标题，则尝试更新标题
            if let Some(new_title) = title_from_config {
                if old_title.as_ref() != Some(&new_title) {
                    sqlx::query("UPDATE topic_index SET title = ? WHERE topic_id = ?")
                        .bind(new_title)
                        .bind(&topic_id)
                        .execute(pool)
                        .await
                        .map_err(|e| e.to_string())?;
                }
            }
            return Ok(());
        }
        existing_title = old_title;
    }

    // Read file and calculate hash/count
    let content = tokio::fs::read_to_string(path)
        .await
        .map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    let file_hash = hex::encode(hasher.finalize());

    let history: serde_json::Value = serde_json::from_str(&content).map_err(|e| e.to_string())?;
    let mut msg_count = 0;
    let mut smart_unread_count = 0;

    if let Some(history_array) = history.as_array() {
        msg_count = history_array.len() as i32;

        // "智能计数判断"
        let non_system_msgs: Vec<_> = history_array
            .iter()
            .filter(|m| m.get("role").and_then(|r| r.as_str()) != Some("system"))
            .collect();

        if non_system_msgs.len() == 1 {
            if let Some(role) = non_system_msgs[0].get("role").and_then(|r| r.as_str()) {
                if role == "assistant" {
                    smart_unread_count = 1;
                }
            }
        }
    }

    // Determine title: Use topic config > DB existing title > topic_id
    let title = title_from_config
        .or(existing_title)
        .unwrap_or_else(|| topic_id.clone());

    sqlx::query(
        "INSERT INTO topic_index (topic_id, agent_id, title, mtime, file_hash, msg_count, locked, unread, unread_count)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(topic_id) DO UPDATE SET
            title = excluded.title,
            mtime = excluded.mtime,
            file_hash = excluded.file_hash,
            msg_count = excluded.msg_count,
            locked = excluded.locked,
            unread = excluded.unread,
            unread_count = excluded.unread_count",
    )
    .bind(&topic_id)
    .bind(&agent_id)
    .bind(&title)
    .bind(mtime)
    .bind(&file_hash)
    .bind(msg_count)
    .bind(locked)
    .bind(unread)
    .bind(smart_unread_count)
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    info!(
        "[IndexService] Indexed topic: {} (Agent: {}, Messages: {})",
        topic_id, agent_id, msg_count
    );

    Ok(())
}

use hex;
use log::{error, info, warn};
use sha2::{Digest, Sha256};
use sqlx::{Pool, Sqlite};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use tauri::{AppHandle, Manager};
use walkdir::WalkDir;

#[derive(Clone, Copy, Debug)]
enum IndexedItemKind {
    Agent,
    Group,
}

impl IndexedItemKind {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Group => "group",
        }
    }
}

#[derive(Debug)]
struct IndexedHistoryTarget {
    item_id: String,
    topic_id: String,
    history_path: PathBuf,
    topic_config_path: PathBuf,
}

#[derive(Debug)]
struct ItemIdentity {
    kind: IndexedItemKind,
    config_path: PathBuf,
    config_exists: bool,
}

#[derive(Debug)]
struct TopicMetadata {
    title: Option<String>,
    locked: Option<bool>,
    unread: Option<bool>,
}

impl TopicMetadata {
    fn is_empty(&self) -> bool {
        self.title.is_none() && self.locked.is_none() && self.unread.is_none()
    }

    fn merge_from(&mut self, other: TopicMetadata) {
        if self.title.is_none() {
            self.title = other.title;
        }
        if self.locked.is_none() {
            self.locked = other.locked;
        }
        if self.unread.is_none() {
            self.unread = other.unread;
        }
    }
}

#[derive(Debug)]
struct MetadataResolution {
    metadata: TopicMetadata,
    source: &'static str,
    missing_layers: Vec<&'static str>,
}

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

fn parse_history_target(path: &Path) -> Option<IndexedHistoryTarget> {
    let components: Vec<_> = path.components().collect();
    if components.len() < 4 {
        return None;
    }

    let topic_id = components.get(components.len() - 2)?.as_os_str().to_str()?.to_string();
    let item_id = components.get(components.len() - 4)?.as_os_str().to_str()?.to_string();

    Some(IndexedHistoryTarget {
        item_id,
        topic_id,
        history_path: path.to_path_buf(),
        topic_config_path: path.with_file_name("config.json"),
    })
}

fn resolve_item_identity(app_config_dir: &Path, item_id: &str) -> ItemIdentity {
    let group_config_path = app_config_dir
        .join("AgentGroups")
        .join(item_id)
        .join("config.json");
    let agent_config_path = app_config_dir.join("Agents").join(item_id).join("config.json");

    if group_config_path.exists() {
        ItemIdentity {
            kind: IndexedItemKind::Group,
            config_path: group_config_path,
            config_exists: true,
        }
    } else if agent_config_path.exists() {
        ItemIdentity {
            kind: IndexedItemKind::Agent,
            config_path: agent_config_path,
            config_exists: true,
        }
    } else {
        ItemIdentity {
            kind: IndexedItemKind::Agent,
            config_path: agent_config_path,
            config_exists: false,
        }
    }
}

async fn load_topic_dir_metadata(topic_config_path: &Path) -> Result<Option<TopicMetadata>, String> {
    if !topic_config_path.exists() {
        return Ok(None);
    }

    let topic_content = tokio::fs::read_to_string(topic_config_path)
        .await
        .map_err(|e| e.to_string())?;
    let topic_json = serde_json::from_str::<serde_json::Value>(&topic_content)
        .map_err(|e| e.to_string())?;

    Ok(Some(TopicMetadata {
        title: topic_json
            .get("name")
            .or(topic_json.get("title"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        locked: topic_json.get("locked").and_then(|v| v.as_bool()),
        unread: topic_json.get("unread").and_then(|v| v.as_bool()),
    }))
}

fn extract_topic_entry_metadata(item_json: &serde_json::Value, topic_id: &str) -> Option<TopicMetadata> {
    item_json
        .get("topics")
        .and_then(|v| v.as_array())
        .and_then(|topics| {
            topics.iter().find(|topic| {
                topic.get("id").and_then(|v| v.as_str()) == Some(topic_id)
            })
        })
        .map(|topic_entry| TopicMetadata {
            title: topic_entry
                .get("name")
                .or(topic_entry.get("title"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            locked: topic_entry
                .get("locked")
                .or_else(|| {
                    topic_entry
                        .get("extra")
                        .and_then(|v| v.get("locked"))
                })
                .or_else(|| {
                    topic_entry
                        .get("extra_fields")
                        .and_then(|v| v.get("locked"))
                })
                .and_then(|v| v.as_bool()),
            unread: topic_entry
                .get("unread")
                .or_else(|| {
                    topic_entry
                        .get("extra")
                        .and_then(|v| v.get("unread"))
                })
                .or_else(|| {
                    topic_entry
                        .get("extra_fields")
                        .and_then(|v| v.get("unread"))
                })
                .and_then(|v| v.as_bool()),
        })
}

async fn load_item_config_metadata(
    identity: &ItemIdentity,
    topic_id: &str,
) -> Result<Option<TopicMetadata>, String> {
    if !identity.config_exists {
        return Ok(None);
    }

    let item_content = tokio::fs::read_to_string(&identity.config_path)
        .await
        .map_err(|e| e.to_string())?;
    let item_json = serde_json::from_str::<serde_json::Value>(&item_content)
        .map_err(|e| e.to_string())?;

    Ok(extract_topic_entry_metadata(&item_json, topic_id))
}

async fn resolve_topic_metadata(
    identity: &ItemIdentity,
    target: &IndexedHistoryTarget,
) -> Result<MetadataResolution, String> {
    let mut metadata = TopicMetadata {
        title: None,
        locked: None,
        unread: None,
    };
    let mut source = "history directory fallback";
    let mut missing_layers = Vec::new();

    match load_topic_dir_metadata(&target.topic_config_path).await? {
        Some(topic_dir_metadata) => {
            metadata.merge_from(topic_dir_metadata);
            source = "topic directory config";
        }
        None => missing_layers.push("topic directory config"),
    }

    match load_item_config_metadata(identity, &target.topic_id).await? {
        Some(item_metadata) => {
            let had_primary = !metadata.is_empty();
            metadata.merge_from(item_metadata);
            if !had_primary && !metadata.is_empty() {
                source = match identity.kind {
                    IndexedItemKind::Agent => "agent config topics[]",
                    IndexedItemKind::Group => "group config topics[]",
                };
            }
        }
        None => {
            if identity.config_exists {
                missing_layers.push(match identity.kind {
                    IndexedItemKind::Agent => "agent config topics[]",
                    IndexedItemKind::Group => "group config topics[]",
                });
            } else {
                missing_layers.push(match identity.kind {
                    IndexedItemKind::Agent => "agent config missing",
                    IndexedItemKind::Group => "group config missing",
                });
            }
        }
    }

    Ok(MetadataResolution {
        metadata,
        source,
        missing_layers,
    })
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

    let Some(target) = parse_history_target(path) else {
        return Ok(());
    };

    let identity = resolve_item_identity(app_config_dir, &target.item_id);
    let topic_id_source = "history directory name";
    let metadata_resolution = resolve_topic_metadata(&identity, &target).await?;

    if !identity.config_exists {
        warn!(
            "[IndexService] item_kind={} item_id={} item_config=missing path={:?}; indexing with history-only fallback",
            identity.kind.as_str(),
            target.item_id,
            identity.config_path
        );
    }

    info!(
        "[IndexService] item_kind={} item_id={} topic_id={} topic_source={} metadata_source={} missing_layers={:?}",
        identity.kind.as_str(),
        target.item_id,
        target.topic_id,
        topic_id_source,
        metadata_resolution.source,
        metadata_resolution.missing_layers
    );

    // Check if we need to re-index based on mtime, and retain the old title if any
    let existing: Option<(i64, Option<String>)> =
        sqlx::query_as("SELECT mtime, title FROM topic_index WHERE topic_id = ?")
            .bind(&target.topic_id)
            .fetch_optional(pool)
            .await
            .map_err(|e| e.to_string())?;

    let mut existing_title = None;
    if let Some((old_mtime, old_title)) = existing {
        if old_mtime >= mtime {
            if let Some(new_title) = metadata_resolution.metadata.title.clone() {
                if old_title.as_ref() != Some(&new_title) {
                    sqlx::query("UPDATE topic_index SET title = ? WHERE topic_id = ?")
                        .bind(new_title)
                        .bind(&target.topic_id)
                        .execute(pool)
                        .await
                        .map_err(|e| e.to_string())?;
                }
            }
            return Ok(());
        }
        existing_title = old_title;
    }

    let content = tokio::fs::read_to_string(&target.history_path)
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

    let title = metadata_resolution
        .metadata
        .title
        .clone()
        .or(existing_title)
        .unwrap_or_else(|| target.topic_id.clone());
    let locked = metadata_resolution.metadata.locked.unwrap_or(false);
    let unread = metadata_resolution.metadata.unread.unwrap_or(false);

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
    .bind(&target.topic_id)
    .bind(&target.item_id)
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
        "[IndexService] indexed item_kind={} item_id={} topic_id={} messages={} title={:?}",
        identity.kind.as_str(),
        target.item_id,
        target.topic_id,
        msg_count,
        title
    );

    Ok(())
}

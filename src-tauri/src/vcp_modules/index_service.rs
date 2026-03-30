use hex;
use log::{debug, error, info, warn};
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::{Pool, Sqlite};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use tauri::{AppHandle, Emitter, Manager};
use walkdir::WalkDir;

#[derive(Clone, Serialize)]
struct TopicIndexUpdatePayload {
    topic_id: String,
    agent_id: String,
    title: String,
    msg_count: i32,
    unread_count: i32,
    created_at: i64,
    locked: bool,
    unread: bool,
}

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
    created_at: Option<i64>,
}

impl TopicMetadata {
    fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.locked.is_none()
            && self.unread.is_none()
            && self.created_at.is_none()
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
        if self.created_at.is_none() {
            self.created_at = other.created_at;
        }
    }
}

#[derive(Debug)]
struct MetadataResolution {
    metadata: TopicMetadata,
    source: &'static str,
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

fn parse_history_target(path: &Path) -> Option<IndexedHistoryTarget> {
    let components: Vec<_> = path.components().collect();
    if components.len() < 4 {
        return None;
    }

    let topic_id = components
        .get(components.len() - 2)?
        .as_os_str()
        .to_str()?
        .to_string();
    let item_id = components
        .get(components.len() - 4)?
        .as_os_str()
        .to_str()?
        .to_string();

    Some(IndexedHistoryTarget {
        item_id,
        topic_id,
        history_path: path.to_path_buf(),
    })
}

fn resolve_item_identity(app_config_dir: &Path, item_id: &str) -> ItemIdentity {
    let group_config_path = app_config_dir
        .join("AgentGroups")
        .join(item_id)
        .join("config.json");
    let agent_config_path = app_config_dir
        .join("Agents")
        .join(item_id)
        .join("config.json");

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

fn extract_topic_entry_metadata(
    item_json: &serde_json::Value,
    topic_id: &str,
) -> Option<TopicMetadata> {
    item_json
        .get("topics")
        .and_then(|v| v.as_array())
        .and_then(|topics| {
            topics
                .iter()
                .find(|topic| topic.get("id").and_then(|v| v.as_str()) == Some(topic_id))
        })
        .map(|topic_entry| TopicMetadata {
            title: topic_entry
                .get("name")
                .or(topic_entry.get("title"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            locked: topic_entry
                .get("locked")
                .or_else(|| topic_entry.get("extra").and_then(|v| v.get("locked")))
                .or_else(|| {
                    topic_entry
                        .get("extra_fields")
                        .and_then(|v| v.get("locked"))
                })
                .and_then(|v| v.as_bool()),
            unread: topic_entry
                .get("unread")
                .or_else(|| topic_entry.get("extra").and_then(|v| v.get("unread")))
                .or_else(|| {
                    topic_entry
                        .get("extra_fields")
                        .and_then(|v| v.get("unread"))
                })
                .and_then(|v| v.as_bool()),
            created_at: topic_entry
                .get("createdAt")
                .or_else(|| topic_entry.get("created_at"))
                .and_then(|v| v.as_i64()),
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
    let item_json =
        serde_json::from_str::<serde_json::Value>(&item_content).map_err(|e| e.to_string())?;

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
        created_at: None,
    };
    let mut source = "history directory fallback";

    match load_item_config_metadata(identity, &target.topic_id).await? {
        Some(item_metadata) => {
            metadata.merge_from(item_metadata);
            source = match identity.kind {
                IndexedItemKind::Agent => "agent config topics[]",
                IndexedItemKind::Group => "group config topics[]",
            };
        }
        None => {}
    }

    Ok(MetadataResolution { metadata, source })
}

pub async fn index_history_file(
    app_handle: &AppHandle,
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

    debug!(
        "[IndexService] item_kind={} item_id={} topic_id={} topic_source={} metadata_source={}",
        identity.kind.as_str(),
        target.item_id,
        target.topic_id,
        topic_id_source,
        metadata_resolution.source
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

    // 兼容性处理：Agent 默认 locked=true, unread=false; Group 默认 false
    let locked = metadata_resolution
        .metadata
        .locked
        .unwrap_or_else(|| match identity.kind {
            IndexedItemKind::Agent => true,
            IndexedItemKind::Group => false,
        });
    let unread = metadata_resolution.metadata.unread.unwrap_or(false);
    let created_at = metadata_resolution.metadata.created_at.unwrap_or(0);

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

    // 发送增量更新事件到前端
    let _ = app_handle.emit(
        "topic-index-updated",
        TopicIndexUpdatePayload {
            topic_id: target.topic_id.clone(),
            agent_id: target.item_id.clone(),
            title: title.clone(),
            msg_count,
            unread_count: smart_unread_count,
            created_at,
            locked,
            unread,
        },
    );

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

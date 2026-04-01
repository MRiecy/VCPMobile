use crate::vcp_modules::chat_manager::ChatMessage;
use hex;
use log::{debug, info, warn};
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::{Pool, Sqlite};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager};

#[derive(Clone, Serialize)]
pub struct TopicIndexUpdatePayload {
    pub topic_id: String,
    pub agent_id: String,
    pub title: String,
    pub msg_count: i32,
    pub unread_count: i32,
    pub created_at: i64,
    pub locked: bool,
    pub unread: bool,
}

#[derive(Clone, Debug)]
pub struct TopicMessageProjection {
    pub msg_count: i32,
    pub last_msg_preview: Option<String>,
    pub unread_count: i32,
}

#[derive(Clone, Debug)]
pub struct TopicMetadataProjection {
    pub title: String,
    pub locked: bool,
    pub unread: bool,
    pub created_at: i64,
}

#[derive(Clone, Debug)]
pub struct TopicProjectionRecord {
    pub topic_id: String,
    pub item_id: String,
    pub mtime: i64,
    pub file_hash: Option<String>,
    pub message: TopicMessageProjection,
    pub metadata: TopicMetadataProjection,
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

#[derive(Debug, Clone)]
pub struct IndexedHistoryTarget {
    pub item_id: String,
    pub topic_id: String,
    pub history_path: PathBuf,
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

pub fn compute_topic_message_projection(history: &[ChatMessage]) -> TopicMessageProjection {
    let msg_count = history.len() as i32;
    let last_msg_preview = history.last().map(|m| preview_message_content(&m.content));

    let non_system_msgs: Vec<_> = history.iter().filter(|m| m.role != "system").collect();
    let unread_count = if non_system_msgs.len() == 1 && non_system_msgs[0].role == "assistant" {
        1
    } else {
        0
    };

    TopicMessageProjection {
        msg_count,
        last_msg_preview,
        unread_count,
    }
}

pub fn history_mtime_millis(path: &Path) -> Result<i64, String> {
    let metadata = fs::metadata(path).map_err(|e| e.to_string())?;
    let modified = metadata.modified().map_err(|e| e.to_string())?;
    Ok(system_time_to_millis(modified))
}

pub async fn apply_topic_projection(
    app_handle: &AppHandle,
    pool: &Pool<Sqlite>,
    record: &TopicProjectionRecord,
) -> Result<(), String> {
    sqlx::query(
        "INSERT INTO topic_index (topic_id, agent_id, title, mtime, file_hash, last_msg_preview, msg_count, locked, unread, unread_count)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(topic_id) DO UPDATE SET
            agent_id = excluded.agent_id,
            title = excluded.title,
            mtime = excluded.mtime,
            file_hash = excluded.file_hash,
            last_msg_preview = excluded.last_msg_preview,
            msg_count = excluded.msg_count,
            locked = excluded.locked,
            unread = excluded.unread,
            unread_count = excluded.unread_count",
    )
    .bind(&record.topic_id)
    .bind(&record.item_id)
    .bind(&record.metadata.title)
    .bind(record.mtime)
    .bind(&record.file_hash)
    .bind(&record.message.last_msg_preview)
    .bind(record.message.msg_count)
    .bind(record.metadata.locked)
    .bind(record.metadata.unread)
    .bind(record.message.unread_count)
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    let _ = app_handle.emit(
        "topic-index-updated",
        TopicIndexUpdatePayload {
            topic_id: record.topic_id.clone(),
            agent_id: record.item_id.clone(),
            title: record.metadata.title.clone(),
            msg_count: record.message.msg_count,
            unread_count: record.message.unread_count,
            created_at: record.metadata.created_at,
            locked: record.metadata.locked,
            unread: record.metadata.unread,
        },
    );

    Ok(())
}

pub async fn refresh_topic_projection_from_history(
    app_handle: &AppHandle,
    pool: &Pool<Sqlite>,
    item_id: &str,
    topic_id: &str,
    history_path: &Path,
    history: &[ChatMessage],
) -> Result<(), String> {
    let app_config_dir = app_handle
        .path()
        .app_config_dir()
        .map_err(|e: tauri::Error| e.to_string())?;
    let identity = resolve_item_identity(&app_config_dir, item_id);
    let metadata_resolution = resolve_topic_metadata(&identity, topic_id).await?;

    if !identity.config_exists {
        warn!(
            "[TopicProjection] item_kind={} item_id={} item_config=missing path={:?}; projecting with history-only fallback",
            identity.kind.as_str(),
            item_id,
            identity.config_path
        );
    }

    debug!(
        "[TopicProjection] item_kind={} item_id={} topic_id={} metadata_source={}",
        identity.kind.as_str(),
        item_id,
        topic_id,
        metadata_resolution.source
    );

    let existing_title: Option<String> =
        sqlx::query_scalar("SELECT title FROM topic_index WHERE topic_id = ?")
            .bind(topic_id)
            .fetch_optional(pool)
            .await
            .map_err(|e| e.to_string())?;

    let title = metadata_resolution
        .metadata
        .title
        .clone()
        .or(existing_title)
        .unwrap_or_else(|| topic_id.to_string());
    let locked = metadata_resolution
        .metadata
        .locked
        .unwrap_or_else(|| default_locked(identity.kind));
    let unread = metadata_resolution.metadata.unread.unwrap_or(false);
    let created_at = metadata_resolution.metadata.created_at.unwrap_or(0);
    let message = compute_topic_message_projection(history);
    let mtime = history_mtime_millis(history_path)?;
    let file_hash = compute_file_hash(history_path)?;

    apply_topic_projection(
        app_handle,
        pool,
        &TopicProjectionRecord {
            topic_id: topic_id.to_string(),
            item_id: item_id.to_string(),
            mtime,
            file_hash: Some(file_hash),
            message,
            metadata: TopicMetadataProjection {
                title: title.clone(),
                locked,
                unread,
                created_at,
            },
        },
    )
    .await?;

    info!(
        "[TopicProjection] projected item_kind={} item_id={} topic_id={} messages={} title={:?}",
        identity.kind.as_str(),
        item_id,
        topic_id,
        history.len(),
        title
    );

    Ok(())
}

pub fn parse_history_target(app_config_dir: &Path, path: &Path) -> Option<IndexedHistoryTarget> {
    let normalized = path.strip_prefix(app_config_dir).ok()?;
    let components: Vec<_> = normalized.components().collect();
    if components.len() < 5 {
        return None;
    }

    let data_root = components.first()?.as_os_str().to_str()?;
    if data_root != "UserData" && data_root != "data" {
        return None;
    }

    if components.last()?.as_os_str().to_str()? != "history.json" {
        return None;
    }

    let item_id = components.get(1)?.as_os_str().to_str()?.to_string();
    let topics_segment = components.get(2)?.as_os_str().to_str()?;
    if topics_segment != "topics" {
        return None;
    }

    let topic_dir_name = components.get(3)?.as_os_str().to_str()?;
    let topic_id = if is_group_identity(app_config_dir, &item_id) {
        topic_dir_name
            .strip_prefix("group_")
            .unwrap_or(topic_dir_name)
            .to_string()
    } else {
        topic_dir_name.to_string()
    };

    Some(IndexedHistoryTarget {
        item_id,
        topic_id,
        history_path: path.to_path_buf(),
    })
}

pub async fn rebuild_topic_projection_from_history_path(
    app_handle: &AppHandle,
    app_config_dir: &Path,
    path: &Path,
    pool: &Pool<Sqlite>,
) -> Result<(), String> {
    let Some(target) = parse_history_target(app_config_dir, path) else {
        return Ok(());
    };

    let content = tokio::fs::read_to_string(&target.history_path)
        .await
        .map_err(|e| e.to_string())?;
    let history: Vec<ChatMessage> = serde_json::from_str(&content).map_err(|e| e.to_string())?;

    refresh_topic_projection_from_history(
        app_handle,
        pool,
        &target.item_id,
        &target.topic_id,
        &target.history_path,
        &history,
    )
    .await
}

fn preview_message_content(content: &str) -> String {
    let mut preview = content.chars().take(100).collect::<String>();
    if content.chars().count() > 100 {
        preview.push_str("...");
    }
    preview
}

fn system_time_to_millis(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn compute_file_hash(path: &Path) -> Result<String, String> {
    let content = fs::read(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    hasher.update(&content);
    Ok(hex::encode(hasher.finalize()))
}

fn default_locked(kind: IndexedItemKind) -> bool {
    match kind {
        IndexedItemKind::Agent => true,
        IndexedItemKind::Group => false,
    }
}

fn is_group_identity(app_config_dir: &Path, item_id: &str) -> bool {
    app_config_dir.join("AgentGroups").join(item_id).exists()
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
    topic_id: &str,
) -> Result<MetadataResolution, String> {
    let mut metadata = TopicMetadata {
        title: None,
        locked: None,
        unread: None,
        created_at: None,
    };
    let mut source = "history directory fallback";

    if let Some(item_metadata) = load_item_config_metadata(identity, topic_id).await? {
        metadata.merge_from(item_metadata);
        source = match identity.kind {
            IndexedItemKind::Agent => "agent config topics[]",
            IndexedItemKind::Group => "group config topics[]",
        };
    }

    Ok(MetadataResolution { metadata, source })
}

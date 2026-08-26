use crate::vcp_modules::agent_service;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::group_service;
use crate::vcp_modules::sync_dto::{AgentSyncDTO, AttachmentSyncDTO, GroupSyncDTO, MessageSyncDTO};
use crate::vcp_modules::sync_error::{encode_http_sync_error_body, encode_wire_sync_error};
use crate::vcp_modules::sync_hash::HashAggregator;
use crate::vcp_modules::sync_types::{
    AvatarOwnerType, AvatarPushResponse, EntityPushData, EntityPushItem, EntityPushRequest,
    EntityPushResponse, MessagePushResponseFrame, OwnerType,
};
use crate::vcp_modules::topic_types::TopicKey;
use futures_util::StreamExt;
use serde::de::DeserializeOwned;
use sqlx::{Row, Sqlite, Transaction};
use std::collections::{HashMap, HashSet};
use std::io::Write;
use tauri::{AppHandle, Manager, Runtime};

const MAX_NDJSON_LINE_BYTES: usize = 32 * 1024 * 1024;
const MAX_SYNC_BODY_BYTES: usize = 256 * 1024 * 1024;
const MESSAGE_REQUEST_CHUNK_BYTES: usize = 8 * 1024 * 1024;
const MAX_SYNC_TOPICS: usize = 10_000;
const MAX_SYNC_MESSAGES: usize = 100_000;
const MAX_MESSAGES_PER_TOPIC: usize = 10_000;
const MESSAGE_PAGE_SIZE: usize = 100;
const MAX_CONTROL_RESPONSE_BYTES: usize = 1024 * 1024;

async fn read_response_limited(
    response: reqwest::Response,
    max_bytes: usize,
    operation: &str,
) -> Result<(reqwest::StatusCode, Vec<u8>), String> {
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        return Err(format!("{operation} response exceeds {max_bytes} bytes"));
    }
    let status = response.status();
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| format!("{operation} response read failed: {error}"))?;
        if body.len().saturating_add(chunk.len()) > max_bytes {
            return Err(format!("{operation} response exceeds {max_bytes} bytes"));
        }
        body.extend_from_slice(&chunk);
    }
    Ok((status, body))
}

fn canonical_sha256(value: &str) -> Option<String> {
    let normalized = value.to_ascii_lowercase();
    crate::vcp_modules::infra::utils::is_valid_cas_hash(&normalized).then_some(normalized)
}

async fn parse_json_response<T: DeserializeOwned>(
    response: reqwest::Response,
    operation: &str,
) -> Result<T, String> {
    let (status, bytes) =
        read_response_limited(response, MAX_CONTROL_RESPONSE_BYTES, operation).await?;
    if !status.is_success() {
        return match encode_http_sync_error_body(&bytes) {
            Ok(Some(encoded)) => Err(encoded),
            Ok(None) => Err(format!(
                "{operation} failed with HTTP {status} without a Wire 1.4 error object"
            )),
            Err(error) => Err(format!(
                "{operation} returned an invalid Wire 1.4 error: {error}"
            )),
        };
    }
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("{operation} returned invalid JSON: {error}"))
}

/// 批量 Push 单 topic 处理结果
pub struct PushBatchResult {
    pub topic: TopicKey,
    pub success: bool,
    pub error: Option<String>,
}

#[derive(Debug)]
struct MessageTombstone {
    msg_id: String,
    deleted_at: i64,
}

struct TopicMessagePreflight {
    live_count: usize,
    tombstone_count: usize,
}

struct BoundedJsonLine {
    bytes: Vec<u8>,
    limit: usize,
}

impl BoundedJsonLine {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
        }
    }

    fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

impl Write for BoundedJsonLine {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.bytes.len().saturating_add(bytes.len()) > self.limit {
            return Err(std::io::Error::other("JSON line exceeds its byte budget"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn parse_message_push_results(
    bytes: &[u8],
    expected_topics: &[TopicKey],
) -> Result<Vec<PushBatchResult>, String> {
    let response_text = std::str::from_utf8(bytes)
        .map_err(|error| format!("Batch push response is not UTF-8: {error}"))?;
    let expected = expected_topics.iter().cloned().collect::<HashSet<_>>();
    let mut seen = HashSet::new();
    let mut results = Vec::with_capacity(expected.len());
    for raw_line in response_text.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        if line.len() > MAX_NDJSON_LINE_BYTES {
            return Err("Batch push response contains a line over 32 MiB".to_string());
        }
        let frame: MessagePushResponseFrame = serde_json::from_str(line)
            .map_err(|error| format!("Batch push response contains malformed NDJSON: {error}"))?;
        let (owner_type, owner_id, topic_id, success, wire_error) = match frame {
            MessagePushResponseFrame::StreamError { error } => {
                return Err(encode_wire_sync_error(&error)?);
            }
            MessagePushResponseFrame::Topic {
                owner_type,
                owner_id,
                topic_id,
                ok,
                error,
            } => (owner_type, owner_id, topic_id, ok, error),
        };
        if owner_id.is_empty() || topic_id.is_empty() {
            return Err("Batch push response requires complete topic identity".to_string());
        }
        let topic = TopicKey::new(owner_type.as_str(), &owner_id, &topic_id);
        if !expected.contains(&topic) {
            return Err(format!(
                "Batch push response contains unexpected topic {topic_id}"
            ));
        }
        if !seen.insert(topic.clone()) {
            return Err(format!(
                "Batch push response contains duplicate topic {topic_id}"
            ));
        }
        let error = wire_error
            .as_ref()
            .map(encode_wire_sync_error)
            .transpose()
            .map_err(|parse_error| {
                format!("Batch push result for {topic_id} has invalid error: {parse_error}")
            })?;
        if !success && error.is_none() {
            return Err(format!(
                "Failed batch push result for {topic_id} requires an error message"
            ));
        }
        if success && error.is_some() {
            return Err(format!(
                "Successful batch push result for {topic_id} must not contain an error"
            ));
        }
        results.push(PushBatchResult {
            topic,
            success,
            error,
        });
    }

    if seen != expected {
        let mut missing = expected.difference(&seen).cloned().collect::<Vec<_>>();
        missing.sort();
        return Err(format!("Batch push response is missing topics {missing:?}"));
    }
    Ok(results)
}

async fn send_message_chunk(
    client: &reqwest::Client,
    http_url: &str,
    sync_token: &str,
    body: Vec<u8>,
    expected_topics: &[TopicKey],
) -> Result<Vec<PushBatchResult>, String> {
    let url = format!("{}/api/mobile-sync/messages/push", http_url);
    let response = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", sync_token))
        .header("Content-Type", "application/x-ndjson")
        .body(body)
        .send()
        .await
        .map_err(|error| format!("Batch push request failed: {error}"))?;
    let (status, bytes) =
        read_response_limited(response, MAX_NDJSON_LINE_BYTES, "Batch push").await?;
    if !status.is_success() {
        return match encode_http_sync_error_body(&bytes) {
            Ok(Some(encoded)) => Err(encoded),
            Ok(None) => Err(format!(
                "Batch push messages failed with HTTP {status} without a Wire 1.4 error object"
            )),
            Err(error) => Err(format!(
                "Batch push messages returned an invalid Wire 1.4 error: {error}"
            )),
        };
    }

    parse_message_push_results(&bytes, expected_topics)
}

async fn send_entity_items(
    client: &reqwest::Client,
    http_url: &str,
    sync_token: &str,
    items: Vec<EntityPushItem>,
    idempotency_key: Option<String>,
) -> Result<(), String> {
    if items.is_empty() {
        return Ok(());
    }
    if items.len() > MAX_SYNC_TOPICS {
        return Err(format!(
            "Entity push contains {} items, limit is {MAX_SYNC_TOPICS}",
            items.len()
        ));
    }
    let mut expected = HashSet::new();
    for item in &items {
        if !item.is_consistent() {
            return Err("Entity push item has mismatched identity and DTO type".to_string());
        }
        let identity = item.selector();
        if !expected.insert(identity.clone()) {
            return Err(format!(
                "Entity push contains duplicate {}",
                identity.label()
            ));
        }
    }

    let body = serde_json::to_vec(&EntityPushRequest { items })
        .map_err(|error| format!("Entity push serialization failed: {error}"))?;
    if body.len() > 10 * 1024 * 1024 {
        return Err("Entity push request exceeds 10 MiB".to_string());
    }
    let mut request = client
        .post(format!("{http_url}/api/mobile-sync/entities/push"))
        .header("Authorization", format!("Bearer {sync_token}"))
        .header("Content-Type", "application/json");
    if let Some(key) = idempotency_key {
        request = request.header("x-idempotency-key", key);
    }
    let response = request
        .body(body)
        .send()
        .await
        .map_err(|error| format!("Entity push request failed: {error}"))?;
    let (status, bytes) = read_response_limited(response, 10 * 1024 * 1024, "Entity push").await?;
    if !status.is_success() {
        return match encode_http_sync_error_body(&bytes) {
            Ok(Some(encoded)) => Err(encoded),
            Ok(None) => Err(format!(
                "Entity push failed with HTTP {status} without a Wire 1.4 error object"
            )),
            Err(error) => Err(format!(
                "Entity push returned an invalid Wire 1.4 error: {error}"
            )),
        };
    }
    let response: EntityPushResponse = serde_json::from_slice(&bytes)
        .map_err(|error| format!("Entity push returned invalid JSON: {error}"))?;
    let mut seen = HashSet::new();
    for result in response.results {
        let (identity, ok, error) = result.into_parts();
        if !expected.contains(&identity) {
            return Err(format!(
                "Entity push returned unexpected {}",
                identity.label()
            ));
        }
        if !seen.insert(identity.clone()) {
            return Err(format!(
                "Entity push returned duplicate {}",
                identity.label()
            ));
        }
        match ok {
            true if error.is_none() => {}
            false => {
                let error = error
                    .as_ref()
                    .ok_or_else(|| {
                        format!("Entity push {} failure requires error", identity.label())
                    })
                    .and_then(encode_wire_sync_error)?;
                return Err(format!("Entity push {} failed: {error}", identity.label()));
            }
            true => {
                return Err(format!(
                    "Successful entity push {} must not contain error",
                    identity.label()
                ));
            }
        }
    }
    if seen != expected {
        let mut missing = expected.difference(&seen).cloned().collect::<Vec<_>>();
        missing.sort();
        return Err(format!("Entity push response is missing {missing:?}"));
    }
    Ok(())
}

async fn preflight_topic_messages(
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
) -> Result<TopicMessagePreflight, String> {
    let topic_id = &key.topic_id;
    let row = sqlx::query(
        "SELECT COUNT(*) AS message_count,
                COALESCE(SUM(
                    LENGTH(CAST(msg_id AS BLOB)) + LENGTH(CAST(role AS BLOB)) +
                    COALESCE(LENGTH(CAST(name AS BLOB)), 0) +
                    COALESCE(LENGTH(CAST(agent_id AS BLOB)), 0) +
                    LENGTH(CAST(content AS BLOB)) +
                    COALESCE(LENGTH(CAST(group_id AS BLOB)), 0) +
                    COALESCE(LENGTH(CAST(finish_reason AS BLOB)), 0) +
                    LENGTH(CAST(content_hash AS BLOB))
                ), 0) AS raw_bytes
         FROM messages
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(topic_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(|error| format!("Message push preflight failed for {topic_id}: {error}"))?;
    let message_count: i64 = row
        .try_get("message_count")
        .map_err(|error| format!("Message push count decode failed for {topic_id}: {error}"))?;
    let message_count = usize::try_from(message_count)
        .map_err(|_| format!("Message push count is invalid for {topic_id}"))?;
    let message_bytes: i64 = row
        .try_get("raw_bytes")
        .map_err(|error| format!("Message push size decode failed for {topic_id}: {error}"))?;
    let message_bytes = usize::try_from(message_bytes)
        .map_err(|_| format!("Message push size is invalid for {topic_id}"))?;

    let attachment_bytes: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(
                    LENGTH(CAST(ma.hash AS BLOB)) + LENGTH(CAST(ma.display_name AS BLOB)) +
                    COALESCE(LENGTH(CAST(a.mime_type AS BLOB)), 0) +
                    COALESCE(LENGTH(CAST(a.extracted_text AS BLOB)), 0) +
                    COALESCE(LENGTH(CAST(a.image_frames AS BLOB)), 0)
                ), 0)
         FROM message_attachments ma
         LEFT JOIN attachments a ON a.hash = ma.hash
         JOIN messages m ON m.owner_type = ma.owner_type AND m.owner_id = ma.owner_id
            AND m.topic_id = ma.topic_id AND m.msg_id = ma.msg_id
         WHERE ma.owner_type = ? AND ma.owner_id = ? AND ma.topic_id = ?
           AND m.deleted_at IS NULL",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(topic_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(|error| format!("Message attachment preflight failed for {topic_id}: {error}"))?;
    let attachment_bytes = usize::try_from(attachment_bytes)
        .map_err(|_| format!("Message attachment size is invalid for {topic_id}"))?;
    let tombstone_row = sqlx::query(
        "SELECT COUNT(*) AS tombstone_count,
                COALESCE(SUM(LENGTH(CAST(msg_id AS BLOB)) + 64), 0) AS raw_bytes
         FROM messages
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NOT NULL",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(topic_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(|error| format!("Message tombstone preflight failed for {topic_id}: {error}"))?;
    let tombstone_count: i64 = tombstone_row.try_get("tombstone_count").map_err(|error| {
        format!("Message tombstone count decode failed for {topic_id}: {error}")
    })?;
    let tombstone_count = usize::try_from(tombstone_count)
        .map_err(|_| format!("Message tombstone count is invalid for {topic_id}"))?;
    let tombstone_bytes: i64 = tombstone_row
        .try_get("raw_bytes")
        .map_err(|error| format!("Message tombstone size decode failed for {topic_id}: {error}"))?;
    let tombstone_bytes = usize::try_from(tombstone_bytes)
        .map_err(|_| format!("Message tombstone size is invalid for {topic_id}"))?;
    let total_count = message_count
        .checked_add(tombstone_count)
        .ok_or_else(|| format!("Message count overflow for {topic_id}"))?;
    if total_count > MAX_MESSAGES_PER_TOPIC {
        return Err(format!(
            "Message push topic {topic_id} contains {total_count} live messages and tombstones, limit is {MAX_MESSAGES_PER_TOPIC}"
        ));
    }

    let raw_bytes = message_bytes
        .checked_add(attachment_bytes)
        .and_then(|bytes| bytes.checked_add(tombstone_bytes))
        .ok_or_else(|| format!("Message push size overflow for {topic_id}"))?;
    if raw_bytes > MAX_NDJSON_LINE_BYTES {
        return Err(format!(
            "Message push topic {topic_id} exceeds the 32 MiB line limit before serialization"
        ));
    }
    Ok(TopicMessagePreflight {
        live_count: message_count,
        tombstone_count,
    })
}

async fn load_message_tombstones(
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
    expected_count: usize,
) -> Result<Vec<MessageTombstone>, String> {
    let topic_id = &key.topic_id;
    let rows = sqlx::query(
        "SELECT msg_id, deleted_at FROM messages
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NOT NULL
         ORDER BY deleted_at ASC, msg_id ASC",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(topic_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(|error| format!("Message tombstone query failed for {topic_id}: {error}"))?;
    if rows.len() != expected_count {
        return Err(format!(
            "Message tombstones for {topic_id} changed during serialization: expected {expected_count}, got {}",
            rows.len()
        ));
    }

    rows.into_iter()
        .map(|row| {
            let message_id: String = row.try_get("msg_id").map_err(|error| {
                format!("Message tombstone id decode failed for {topic_id}: {error}")
            })?;
            let deleted_at: i64 = row.try_get("deleted_at").map_err(|error| {
                format!(
                    "Message tombstone timestamp decode failed for {topic_id}/{message_id}: {error}"
                )
            })?;
            if message_id.is_empty() || deleted_at < 0 {
                return Err(format!(
                    "Message tombstone {topic_id}/{message_id} has an invalid identity or timestamp"
                ));
            }
            Ok(MessageTombstone {
                msg_id: message_id,
                deleted_at,
            })
        })
        .collect()
}

async fn load_outbound_message_page(
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
    cursor: Option<(i64, &str)>,
) -> Result<Vec<MessageSyncDTO>, String> {
    let topic_id = &key.topic_id;
    let mut query = if cursor.is_some() {
        sqlx::query(
            "SELECT msg_id, role, name, agent_id, content, timestamp, is_group_message,
                    group_id, finish_reason, content_hash, updated_at
             FROM messages
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL
               AND (timestamp > ? OR (timestamp = ? AND msg_id > ?))
             ORDER BY timestamp ASC, msg_id ASC
             LIMIT ?",
        )
    } else {
        sqlx::query(
            "SELECT msg_id, role, name, agent_id, content, timestamp, is_group_message,
                    group_id, finish_reason, content_hash, updated_at
             FROM messages
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL
             ORDER BY timestamp ASC, msg_id ASC
             LIMIT ?",
        )
    };
    query = query
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(topic_id);
    if let Some((timestamp, message_id)) = cursor {
        query = query.bind(timestamp).bind(timestamp).bind(message_id);
    }
    query = query.bind(MESSAGE_PAGE_SIZE as i64);
    let rows = query
        .fetch_all(&mut **tx)
        .await
        .map_err(|error| format!("Message page query failed for {topic_id}: {error}"))?;

    let mut messages = Vec::with_capacity(rows.len());
    for row in rows {
        let message_id: String = row
            .try_get("msg_id")
            .map_err(|error| format!("Message id decode failed for {topic_id}: {error}"))?;
        let role: String = row.try_get("role").map_err(|error| {
            format!("Message role decode failed for {topic_id}/{message_id}: {error}")
        })?;
        if message_id.is_empty() || role.is_empty() {
            return Err(format!(
                "Outbound message {topic_id}/{message_id} requires non-empty id and role"
            ));
        }
        let timestamp: i64 = row.try_get("timestamp").map_err(|error| {
            format!("Message timestamp decode failed for {topic_id}/{message_id}: {error}")
        })?;
        let timestamp = u64::try_from(timestamp)
            .map_err(|_| format!("Message {topic_id}/{message_id} has a negative timestamp"))?;
        let content_hash: String = row.try_get("content_hash").map_err(|error| {
            format!("Message hash decode failed for {topic_id}/{message_id}: {error}")
        })?;
        let updated_at: i64 = row.try_get("updated_at").map_err(|error| {
            format!("Message update time decode failed for {topic_id}/{message_id}: {error}")
        })?;
        let updated_at = u64::try_from(updated_at)
            .map_err(|_| format!("Message {topic_id}/{message_id} has a negative update time"))?;
        let is_group_message: i64 = row.try_get("is_group_message").map_err(|error| {
            format!("Message group flag decode failed for {topic_id}/{message_id}: {error}")
        })?;
        messages.push(MessageSyncDTO {
            id: message_id,
            role,
            name: row
                .try_get("name")
                .map_err(|error| format!("Message name decode failed for {topic_id}: {error}"))?,
            content: row.try_get("content").map_err(|error| {
                format!("Message content decode failed for {topic_id}: {error}")
            })?,
            timestamp,
            updated_at,
            is_thinking: None,
            agent_id: row
                .try_get("agent_id")
                .map_err(|error| format!("Message agent decode failed for {topic_id}: {error}"))?,
            group_id: row
                .try_get("group_id")
                .map_err(|error| format!("Message group decode failed for {topic_id}: {error}"))?,
            topic_id: Some(topic_id.to_string()),
            is_group_message: (is_group_message != 0).then_some(true),
            finish_reason: row.try_get("finish_reason").map_err(|error| {
                format!("Message finish reason decode failed for {topic_id}: {error}")
            })?,
            attachments: None,
            content_hash: (!content_hash.is_empty()).then_some(content_hash),
            avatar_color: None,
        });
    }

    if messages.is_empty() {
        return Ok(messages);
    }

    let placeholders = (0..messages.len())
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(", ");
    let attachment_query = format!(
        "SELECT ma.msg_id, ma.hash AS relation_hash, ma.display_name,
                a.hash AS stored_hash, a.mime_type, a.size,
                a.extracted_text, a.image_frames, a.created_at
         FROM message_attachments ma
         LEFT JOIN attachments a ON a.hash = ma.hash
         WHERE ma.owner_type = ? AND ma.owner_id = ? AND ma.topic_id = ?
           AND ma.msg_id IN ({placeholders})
         ORDER BY ma.msg_id, ma.attachment_order ASC"
    );
    let mut attachment_query = sqlx::query(&attachment_query)
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(topic_id);
    for message in &messages {
        attachment_query = attachment_query.bind(&message.id);
    }
    let attachment_rows = attachment_query
        .fetch_all(&mut **tx)
        .await
        .map_err(|error| format!("Message attachment page query failed for {topic_id}: {error}"))?;
    let message_ids = messages
        .iter()
        .map(|message| message.id.clone())
        .collect::<HashSet<_>>();
    let mut attachments_by_message = HashMap::new();
    for row in attachment_rows {
        let message_id: String = row
            .try_get("msg_id")
            .map_err(|error| format!("Attachment message id decode failed: {error}"))?;
        if !message_ids.contains(&message_id) {
            return Err(format!(
                "Attachment query returned unexpected message {topic_id}/{message_id}"
            ));
        }
        let relation_hash_raw: String = row.try_get("relation_hash").map_err(|error| {
            format!("Attachment hash decode failed for {topic_id}/{message_id}: {error}")
        })?;
        let relation_hash = canonical_sha256(&relation_hash_raw).ok_or_else(|| {
            format!("Attachment {relation_hash_raw} referenced by {topic_id}/{message_id} has an invalid hash")
        })?;
        let stored_hash: Option<String> = row.try_get("stored_hash").map_err(|error| {
            format!("Stored attachment hash decode failed for {relation_hash}: {error}")
        })?;
        if stored_hash.as_deref() != Some(relation_hash.as_str()) {
            return Err(format!(
                "Attachment {relation_hash} referenced by {topic_id}/{message_id} is missing locally"
            ));
        }
        let size: Option<i64> = row.try_get("size").map_err(|error| {
            format!("Attachment size decode failed for {relation_hash}: {error}")
        })?;
        let size = u64::try_from(
            size.ok_or_else(|| format!("Attachment {relation_hash} has no local size metadata"))?,
        )
        .map_err(|_| format!("Attachment {relation_hash} has a negative size"))?;
        let created_at: Option<i64> = row.try_get("created_at").map_err(|error| {
            format!("Attachment timestamp decode failed for {relation_hash}: {error}")
        })?;
        let created_at = created_at
            .map(|value| {
                u64::try_from(value)
                    .map_err(|_| format!("Attachment {relation_hash} has a negative timestamp"))
            })
            .transpose()?;
        let image_frames: Option<String> = row.try_get("image_frames").map_err(|error| {
            format!("Attachment frame decode failed for {relation_hash}: {error}")
        })?;
        let image_frames = image_frames
            .map(|value| {
                serde_json::from_str(&value).map_err(|error| {
                    format!("Attachment {relation_hash} has invalid image frames: {error}")
                })
            })
            .transpose()?;
        let extracted_text: Option<String> = row.try_get("extracted_text").map_err(|error| {
            format!("Attachment extracted text decode failed for {relation_hash}: {error}")
        })?;
        attachments_by_message
            .entry(message_id)
            .or_insert_with(Vec::new)
            .push(AttachmentSyncDTO {
                r#type: row
                    .try_get::<Option<String>, _>("mime_type")
                    .map_err(|error| {
                        format!("Attachment MIME decode failed for {relation_hash}: {error}")
                    })?
                    .ok_or_else(|| format!("Attachment {relation_hash} has no MIME metadata"))?,
                name: row.try_get("display_name").map_err(|error| {
                    format!("Attachment name decode failed for {relation_hash}: {error}")
                })?,
                size,
                hash: relation_hash,
                extracted_text,
                image_frames,
                created_at,
            });
    }
    for message in &mut messages {
        if let Some(attachments) = attachments_by_message.remove(&message.id) {
            message.attachments = Some(attachments);
        }
    }
    Ok(messages)
}

async fn serialize_topic_messages(
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
    expected_message_count: usize,
    tombstones: &[MessageTombstone],
) -> Result<Vec<u8>, String> {
    let topic_id = &key.topic_id;
    let owner_type = &key.owner_type;
    let owner_id = &key.owner_id;
    let mut line = BoundedJsonLine::new(MAX_NDJSON_LINE_BYTES);
    line.write_all(br#"{"kind":"topic","topicId":"#)
        .map_err(|error| format!("Message push prefix failed for {topic_id}: {error}"))?;
    serde_json::to_writer(&mut line, topic_id)
        .map_err(|error| format!("Message push topic id serialization failed: {error}"))?;
    line.write_all(b",\"ownerType\":")
        .map_err(|error| format!("Message push owner prefix failed for {topic_id}: {error}"))?;
    serde_json::to_writer(&mut line, owner_type)
        .map_err(|error| format!("Message push owner type serialization failed: {error}"))?;
    line.write_all(b",\"ownerId\":")
        .map_err(|error| format!("Message push owner prefix failed for {topic_id}: {error}"))?;
    serde_json::to_writer(&mut line, owner_id)
        .map_err(|error| format!("Message push owner id serialization failed: {error}"))?;
    line.write_all(b",\"messages\":[")
        .map_err(|error| format!("Message push prefix failed for {topic_id}: {error}"))?;

    let mut cursor: Option<(i64, String)> = None;
    let mut serialized_count = 0usize;
    loop {
        let page = load_outbound_message_page(
            tx,
            key,
            cursor
                .as_ref()
                .map(|(timestamp, message_id)| (*timestamp, message_id.as_str())),
        )
        .await?;
        if page.is_empty() {
            break;
        }
        let page_len = page.len();
        for message in page {
            let timestamp = i64::try_from(message.timestamp).map_err(|_| {
                format!(
                    "Outbound message {} timestamp exceeds the supported range",
                    message.id
                )
            })?;
            let next_cursor = (timestamp, message.id.clone());
            if serialized_count > 0 {
                line.write_all(b",").map_err(|error| {
                    format!("Message push separator failed for {topic_id}: {error}")
                })?;
            }
            serde_json::to_writer(&mut line, &message).map_err(|error| {
                format!(
                    "Message push topic {topic_id} exceeds the 32 MiB line limit or contains invalid data: {error}"
                )
            })?;
            serialized_count = serialized_count
                .checked_add(1)
                .ok_or_else(|| "Message push count overflow".to_string())?;
            cursor = Some(next_cursor);
        }
        if page_len < MESSAGE_PAGE_SIZE {
            break;
        }
    }
    if serialized_count != expected_message_count {
        return Err(format!(
            "Message push topic {topic_id} changed during serialization: expected {expected_message_count}, got {serialized_count}"
        ));
    }
    line.write_all(b"],\"deletedMessages\":[")
        .map_err(|error| format!("Message tombstone prefix failed for {topic_id}: {error}"))?;
    for (index, tombstone) in tombstones.iter().enumerate() {
        if index > 0 {
            line.write_all(b",").map_err(|error| {
                format!("Message tombstone separator failed for {topic_id}: {error}")
            })?;
        }
        serde_json::to_writer(
            &mut line,
            &serde_json::json!({
                "msgId": &tombstone.msg_id,
                "deletedAt": tombstone.deleted_at,
            }),
        )
        .map_err(|error| {
            format!(
                "Message push topic {topic_id} tombstone {} is invalid: {error}",
                tombstone.msg_id
            )
        })?;
    }
    line.write_all(b"]}\n")
        .map_err(|error| format!("Message push suffix failed for {topic_id}: {error}"))?;
    Ok(line.into_bytes())
}

pub struct PushExecutor;

impl PushExecutor {
    pub async fn push_agent<R: Runtime>(
        app: &AppHandle<R>,
        client: &reqwest::Client,
        http_url: &str,
        sync_token: &str,
        agent_id: &str,
    ) -> Result<(), String> {
        let config =
            agent_service::read_agent_config_internal(app, &app.state(), agent_id, None).await?;
        let dto = AgentSyncDTO::from(&config);
        let config_hash = HashAggregator::compute_agent_config_hash(&dto);

        let idempotency_key = generate_idempotency_key("push", "agent", agent_id, &config_hash);
        send_entity_items(
            client,
            http_url,
            sync_token,
            vec![EntityPushItem::Owner {
                owner_type: OwnerType::Agent,
                owner_id: agent_id.to_string(),
                data: EntityPushData::Agent(dto),
            }],
            Some(idempotency_key),
        )
        .await
    }

    pub async fn push_group<R: Runtime>(
        app: &AppHandle<R>,
        client: &reqwest::Client,
        http_url: &str,
        sync_token: &str,
        group_id: &str,
    ) -> Result<(), String> {
        let config =
            group_service::read_group_config(app.clone(), app.state(), group_id.to_string())
                .await?;
        let dto = GroupSyncDTO::from(&config);
        let config_hash = HashAggregator::compute_group_config_hash(&dto);

        let idempotency_key = generate_idempotency_key("push", "group", group_id, &config_hash);
        send_entity_items(
            client,
            http_url,
            sync_token,
            vec![EntityPushItem::Owner {
                owner_type: OwnerType::Group,
                owner_id: group_id.to_string(),
                data: EntityPushData::Group(dto),
            }],
            Some(idempotency_key),
        )
        .await
    }

    /// 批量 Push 实体 (Agent/Group/Topic)
    pub async fn push_entities_batch<R: Runtime>(
        _app: &AppHandle<R>,
        client: &reqwest::Client,
        http_url: &str,
        sync_token: &str,
        items: Vec<EntityPushItem>,
    ) -> Result<(), String> {
        send_entity_items(client, http_url, sync_token, items, None).await
    }

    pub async fn push_avatar<R: Runtime>(
        app: &AppHandle<R>,
        client: &reqwest::Client,
        http_url: &str,
        sync_token: &str,
        owner_type: &str,
        owner_id: &str,
    ) -> Result<(), String> {
        if !crate::vcp_modules::sync_types::is_valid_avatar_owner(owner_type, owner_id) {
            return Err(format!("Invalid avatar owner {owner_type}/{owner_id}"));
        }
        let db = app.state::<DbState>();

        let parent_is_live = match owner_type {
            "agent" => sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM agents WHERE owner_type = 'agent' AND agent_id = ? AND deleted_at IS NULL)",
            )
            .bind(owner_id)
            .fetch_one(&db.pool)
            .await
            .map_err(|error| format!("Push avatar owner lookup failed: {error}"))?,
            "group" => sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM groups WHERE owner_type = 'group' AND group_id = ? AND deleted_at IS NULL)",
            )
            .bind(owner_id)
            .fetch_one(&db.pool)
            .await
            .map_err(|error| format!("Push avatar owner lookup failed: {error}"))?,
            "user" => true,
            _ => false,
        };
        if !parent_is_live {
            return Err(format!(
                "Avatar owner {owner_type}/{owner_id} is missing or deleted"
            ));
        }

        let image_size: Option<i64> = sqlx::query_scalar(
            "SELECT LENGTH(image_data) FROM avatars
             WHERE owner_id = ? AND owner_type = ? AND deleted_at IS NULL",
        )
        .bind(owner_id)
        .bind(owner_type)
        .fetch_optional(&db.pool)
        .await
        .map_err(|error| {
            format!("Push avatar {owner_type}/{owner_id} size lookup failed: {error}")
        })?;
        let image_size = image_size.ok_or_else(|| {
            format!("Avatar {owner_type}/{owner_id} is missing from the local database")
        })?;
        let image_size = usize::try_from(image_size)
            .map_err(|_| format!("Avatar {owner_type}/{owner_id} has an invalid image size"))?;
        if image_size > crate::vcp_modules::avatar_service::MAX_AVATAR_BYTES {
            return Err(format!(
                "Avatar {owner_type}/{owner_id} exceeds the {}-byte upload limit",
                crate::vcp_modules::avatar_service::MAX_AVATAR_BYTES
            ));
        }

        let row = sqlx::query(
            "SELECT image_data, mime_type FROM avatars
             WHERE owner_id = ? AND owner_type = ? AND deleted_at IS NULL",
        )
        .bind(owner_id)
        .bind(owner_type)
        .fetch_optional(&db.pool)
        .await
        .map_err(|e| e.to_string())?;

        let r = row.ok_or_else(|| {
            format!("Avatar {owner_type}/{owner_id} is missing from the local database")
        })?;
        let image_data: Vec<u8> = r.try_get("image_data").map_err(|error| {
            format!("Avatar {owner_type}/{owner_id} image decode failed: {error}")
        })?;
        let mime_type: String = r.try_get("mime_type").map_err(|error| {
            format!("Avatar {owner_type}/{owner_id} MIME decode failed: {error}")
        })?;

        let url = format!(
            "{}/api/mobile-sync/avatars/push?ownerType={}&ownerId={}",
            http_url, owner_type, owner_id
        );
        let response = client
            .post(&url)
            .header("Authorization", format!("Bearer {}", sync_token))
            .header("Content-Type", mime_type)
            .body(image_data)
            .send()
            .await
            .map_err(|error| format!("Push avatar {owner_type}/{owner_id} failed: {error}"))?;
        let body: AvatarPushResponse = parse_json_response(response, "Push avatar").await?;
        let expected_owner_type = AvatarOwnerType::try_from(owner_type)
            .map_err(|_| format!("Invalid avatar owner type {owner_type}"))?;
        if !body.ok || body.owner_type != expected_owner_type || body.owner_id != owner_id {
            return Err(format!(
                "Push avatar response identity mismatch for {owner_type}/{owner_id}"
            ));
        }

        Ok(())
    }

    /// 批量 Push — 一次 HTTP 请求推送多个 topic 的消息
    ///
    /// 手机端批量加载消息 → POST /messages/push (NDJSON)。附件仅随消息
    /// 传输元数据与内容 Hash，二进制 CAS 始终保留在本机。
    ///
    /// 返回每个 topic 的处理结果。
    pub async fn push_messages_batch<R: Runtime>(
        app: &AppHandle<R>,
        client: &reqwest::Client,
        http_url: &str,
        sync_token: &str,
        topic_keys: &[TopicKey],
    ) -> Result<Vec<PushBatchResult>, String> {
        if topic_keys.is_empty() {
            return Ok(Vec::new());
        }
        if topic_keys.len() > MAX_SYNC_TOPICS {
            return Err(format!(
                "Message push contains {} topics, limit is {}",
                topic_keys.len(),
                MAX_SYNC_TOPICS
            ));
        }
        let requested_topics = topic_keys.iter().cloned().collect::<HashSet<_>>();
        if requested_topics.len() != topic_keys.len()
            || requested_topics.iter().any(|topic| !topic.is_valid())
        {
            return Err("Message push topics must have unique valid identities".to_string());
        }

        let db = app.state::<DbState>();
        let mut results = Vec::new();
        let mut total_request_bytes = 0usize;
        let mut total_messages = 0usize;
        let mut request_body = Vec::new();
        let mut request_topics = Vec::new();

        // Each topic is preflighted, paged, and serialized directly into a bounded writer. This
        // avoids retaining a full history, a cloned DTO tree, and a JSON String simultaneously.
        for key in topic_keys {
            let topic_id = &key.topic_id;
            let mut read_tx =
                db.pool.begin().await.map_err(|error| {
                    format!("Message push snapshot failed for {topic_id}: {error}")
                })?;
            let topic_exists = sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM topics
                 WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL)",
            )
            .bind(&key.owner_type)
            .bind(&key.owner_id)
            .bind(&key.topic_id)
            .fetch_one(&mut *read_tx)
            .await
            .map_err(|error| format!("Message push topic query failed: {error}"))?;
            if !topic_exists {
                return Err(format!("Message push topic {topic_id} is missing locally"));
            }
            let preflight = preflight_topic_messages(&mut read_tx, key).await?;
            let topic_message_count = preflight
                .live_count
                .checked_add(preflight.tombstone_count)
                .ok_or_else(|| format!("Message push count overflow for {topic_id}"))?;
            total_messages = total_messages
                .checked_add(topic_message_count)
                .ok_or_else(|| "Message push count overflow".to_string())?;
            if total_messages > MAX_SYNC_MESSAGES {
                return Err(format!(
                    "Message push contains more than {MAX_SYNC_MESSAGES} messages"
                ));
            }

            let topic_tombstones =
                load_message_tombstones(&mut read_tx, key, preflight.tombstone_count).await?;
            let line = serialize_topic_messages(
                &mut read_tx,
                key,
                preflight.live_count,
                &topic_tombstones,
            )
            .await?;
            read_tx.commit().await.map_err(|error| {
                format!("Message push snapshot close failed for {topic_id}: {error}")
            })?;
            total_request_bytes = total_request_bytes
                .checked_add(line.len())
                .ok_or_else(|| "Message push byte count overflow".to_string())?;
            if total_request_bytes > MAX_SYNC_BODY_BYTES {
                return Err("Message push exceeds the 256 MiB total limit".to_string());
            }
            if !request_body.is_empty()
                && request_body.len().saturating_add(line.len()) > MESSAGE_REQUEST_CHUNK_BYTES
            {
                let chunk_results = send_message_chunk(
                    client,
                    http_url,
                    sync_token,
                    std::mem::take(&mut request_body),
                    &request_topics,
                )
                .await?;
                results.extend(chunk_results);
                request_topics.clear();
            }
            if request_body.is_empty() {
                request_body = line;
            } else {
                request_body.extend_from_slice(&line);
            }
            request_topics.push(key.clone());
            if request_body.len() >= MESSAGE_REQUEST_CHUNK_BYTES {
                let chunk_results = send_message_chunk(
                    client,
                    http_url,
                    sync_token,
                    std::mem::take(&mut request_body),
                    &request_topics,
                )
                .await?;
                results.extend(chunk_results);
                request_topics.clear();
            }
        }

        if !request_body.is_empty() {
            let chunk_results =
                send_message_chunk(client, http_url, sync_token, request_body, &request_topics)
                    .await?;
            results.extend(chunk_results);
        }

        // Online WebSocket notifications remain a latency optimization. The acknowledged Topic
        // push carries the same tombstones, so deletions made while disconnected still converge.
        let ok_count = results.iter().filter(|r| r.success).count();
        log::info!(
            "[PushExecutor] Batch push completed: {}/{} topics",
            ok_count,
            topic_keys.len()
        );
        Ok(results)
    }
}

fn generate_idempotency_key(
    action: &str,
    entity_type: &str,
    id: &str,
    config_hash: &str,
) -> String {
    let now = chrono::Utc::now().timestamp() / 60;
    let now_str = now.to_string();
    crate::vcp_modules::infra::utils::calculate_sha256_slices(&[
        action.as_bytes(),
        entity_type.as_bytes(),
        id.as_bytes(),
        config_hash.as_bytes(),
        now_str.as_bytes(),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn topic(topic_id: &str) -> TopicKey {
        TopicKey::new("agent", "agent-a", topic_id)
    }

    #[test]
    fn canonical_hash_is_lowercase_and_rejects_non_sha256_values() {
        assert_eq!(canonical_sha256(&"A".repeat(64)), Some("a".repeat(64)));
        assert_eq!(canonical_sha256(""), None);
        assert_eq!(canonical_sha256(&"g".repeat(64)), None);
        assert_eq!(canonical_sha256(&"a".repeat(63)), None);
    }

    #[test]
    fn message_push_result_is_independent_of_local_attachment_binaries() {
        let expected = vec![topic("topic")];
        let valid = br#"{"kind":"topic","topicId":"topic","ownerType":"agent","ownerId":"agent-a","ok":true}"#;
        let results = parse_message_push_results(valid, &expected).expect("metadata-only result");
        assert_eq!(results.len(), 1);
        assert!(results[0].success);
    }

    #[test]
    fn failed_message_push_preserves_wire_error_and_rejects_legacy_strings() {
        let expected = vec![topic("topic")];
        let valid = serde_json::to_vec(&serde_json::json!({
            "kind":"topic",
            "topicId":"topic",
            "ownerType":"agent",
            "ownerId":"agent-a",
            "ok":false,
            "error":{
                "code":"SYNC_OWNER_CONFLICT",
                "origin":"desktop_cds",
                "stage":"messages",
                "kind":"data",
                "retry":"manual",
                "message":"owner conflict",
                "failedTopicIds":["topic"]
            }
        }))
        .expect("serialize result");
        let results = parse_message_push_results(&valid, &expected).expect("structured result");
        assert_eq!(
            crate::vcp_modules::sync_error::decode_wire_sync_error(
                results[0].error.as_deref().expect("encoded error")
            )
            .expect("wire error")
            .code,
            "SYNC_OWNER_CONFLICT"
        );

        let legacy = serde_json::to_vec(&serde_json::json!({
            "kind":"topic",
            "topicId":"topic",
            "ownerType":"agent",
            "ownerId":"agent-a",
            "ok":false,
            "error":"legacy"
        }))
        .expect("serialize legacy result");
        assert!(parse_message_push_results(&legacy, &expected).is_err());

        let contradictory = serde_json::to_vec(&serde_json::json!({
            "kind":"topic",
            "topicId":"topic",
            "ownerType":"agent",
            "ownerId":"agent-a",
            "ok":true,
            "error":{
                "code":"SYNC_OWNER_CONFLICT",
                "origin":"desktop_cds",
                "stage":"messages",
                "kind":"data",
                "retry":"manual",
                "message":"owner conflict",
                "failedTopicIds":["topic"]
            }
        }))
        .expect("serialize contradictory result");
        assert!(parse_message_push_results(&contradictory, &expected).is_err());
    }

    #[test]
    fn bounded_writer_emits_valid_ndjson_and_rejects_overflow() {
        let mut line = BoundedJsonLine::new(64);
        line.write_all(br#"{"topicId":"#).unwrap();
        serde_json::to_writer(&mut line, "topic").unwrap();
        line.write_all(b",\"messages\":[]}\n").unwrap();
        let bytes = line.into_bytes();
        serde_json::from_slice::<serde_json::Value>(&bytes[..bytes.len() - 1])
            .expect("bounded line must remain valid JSON");

        let mut tiny = BoundedJsonLine::new(3);
        assert!(tiny.write_all(b"four").is_err());
        assert!(tiny.bytes.is_empty());
    }

    #[tokio::test]
    async fn outbound_message_loader_uses_stable_bounded_pages() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open database");
        sqlx::query(
            "CREATE TABLE messages (
                owner_type TEXT NOT NULL, owner_id TEXT NOT NULL,
                topic_id TEXT NOT NULL, msg_id TEXT NOT NULL, role TEXT NOT NULL,
                name TEXT, agent_id TEXT, content TEXT NOT NULL, timestamp BIGINT NOT NULL,
                is_group_message INTEGER NOT NULL, group_id TEXT, finish_reason TEXT,
                content_hash TEXT NOT NULL, created_at BIGINT NOT NULL,
                updated_at BIGINT NOT NULL, deleted_at BIGINT,
                PRIMARY KEY(owner_type, owner_id, topic_id, msg_id)
             );
             CREATE TABLE message_attachments (
                owner_type TEXT, owner_id TEXT, topic_id TEXT, msg_id TEXT,
                hash TEXT, attachment_order INTEGER, display_name TEXT,
                src TEXT, status TEXT, created_at BIGINT
             );
             CREATE TABLE attachments (
                hash TEXT PRIMARY KEY, mime_type TEXT, size BIGINT, internal_path TEXT,
                image_frames TEXT, thumbnail_path TEXT, created_at BIGINT
             );",
        )
        .execute(&pool)
        .await
        .expect("create fixture");
        for index in 0..=MESSAGE_PAGE_SIZE {
            sqlx::query(
                "INSERT INTO messages (
                    owner_type, owner_id, topic_id, msg_id, role, content, timestamp,
                    is_group_message, content_hash, created_at, updated_at
                 ) VALUES ('agent', 'agent-a', 'topic', ?, 'user', 'body', ?, 0, 'hash', ?, ?)",
            )
            .bind(format!("message-{index:03}"))
            .bind(index as i64)
            .bind(index as i64)
            .bind(index as i64)
            .execute(&pool)
            .await
            .expect("insert message");
        }
        sqlx::query(
            "INSERT INTO messages (
                owner_type, owner_id, topic_id, msg_id, role, content, timestamp,
                is_group_message, content_hash, created_at, updated_at, deleted_at
             ) VALUES ('agent', 'agent-a', 'topic', 'message-deleted', 'user',
                       'gone', 200, 0, 'DELETED', 200, 200, 1234)",
        )
        .execute(&pool)
        .await
        .expect("insert tombstone");

        let mut read_tx = pool.begin().await.expect("begin snapshot");
        let key = topic("topic");
        let preflight = preflight_topic_messages(&mut read_tx, &key)
            .await
            .expect("preflight");
        assert_eq!(preflight.live_count, MESSAGE_PAGE_SIZE + 1);
        assert_eq!(preflight.tombstone_count, 1);
        let tombstones = load_message_tombstones(&mut read_tx, &key, 1)
            .await
            .expect("load tombstone");
        assert_eq!(tombstones[0].msg_id, "message-deleted");
        assert_eq!(tombstones[0].deleted_at, 1234);
        let first = load_outbound_message_page(&mut read_tx, &key, None)
            .await
            .expect("first page");
        assert_eq!(first.len(), MESSAGE_PAGE_SIZE);
        assert_eq!(first.first().unwrap().id, "message-000");
        assert_eq!(first.last().unwrap().id, "message-099");
        let second = load_outbound_message_page(&mut read_tx, &key, Some((99, "message-099")))
            .await
            .expect("second page");
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].id, "message-100");
    }
}

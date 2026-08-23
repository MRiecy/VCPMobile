use crate::vcp_modules::agent_service;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::group_service;
use crate::vcp_modules::sync_dto::{
    AgentMessageSyncDTO, AgentSyncDTO, AttachmentSyncDTO, GroupMessageSyncDTO, GroupSyncDTO,
    UserMessageSyncDTO,
};
use crate::vcp_modules::sync_error::{encode_http_sync_error_body, encode_wire_sync_error_value};
use crate::vcp_modules::topic_types::{MessageKey, TopicKey};
use futures_util::StreamExt;
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::{Row, Sqlite, Transaction};
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::sync::Arc;
use tauri::{AppHandle, Manager, Runtime};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::sync::RwLock;
use tokio_util::io::ReaderStream;

const MAX_NDJSON_LINE_BYTES: usize = 32 * 1024 * 1024;
const MAX_SYNC_BODY_BYTES: usize = 256 * 1024 * 1024;
const MESSAGE_REQUEST_CHUNK_BYTES: usize = 8 * 1024 * 1024;
const MAX_SYNC_TOPICS: usize = 10_000;
const MAX_SYNC_MESSAGES: usize = 100_000;
const MAX_MESSAGES_PER_TOPIC: usize = 10_000;
const MESSAGE_PAGE_SIZE: usize = 100;
const MAX_CONTROL_RESPONSE_BYTES: usize = 1024 * 1024;
const MAX_ATTACHMENT_UPLOAD_BYTES: u64 = 512 * 1024 * 1024;

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

async fn parse_success_response(
    response: reqwest::Response,
    operation: &str,
) -> Result<serde_json::Value, String> {
    let (status, bytes) =
        read_response_limited(response, MAX_CONTROL_RESPONSE_BYTES, operation).await?;
    if !status.is_success() {
        return match encode_http_sync_error_body(&bytes) {
            Ok(Some(encoded)) => Err(encoded),
            Ok(None) => Err(format!(
                "{operation} failed with HTTP {status} without a Wire 1.3 error object"
            )),
            Err(error) => Err(format!(
                "{operation} returned an invalid Wire 1.3 error: {error}"
            )),
        };
    }
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("{operation} returned invalid JSON: {error}"))?;
    if value.get("success").and_then(serde_json::Value::as_bool) != Some(true) {
        let error = value.get("error").or_else(|| {
            value
                .get("results")
                .and_then(serde_json::Value::as_array)
                .and_then(|results| {
                    results
                        .iter()
                        .find(|result| {
                            result.get("success").and_then(serde_json::Value::as_bool)
                                == Some(false)
                        })
                        .and_then(|result| result.get("error"))
                })
        });
        let encoded = error
            .ok_or_else(|| format!("{operation} returned success=false without error"))
            .and_then(encode_wire_sync_error_value)?;
        return Err(encoded);
    }
    if value.get("error").is_some() {
        return Err(format!(
            "{operation} returned success=true together with error"
        ));
    }
    Ok(value)
}

async fn query_avatar_color(
    pool: &sqlx::SqlitePool,
    agent_id: &str,
) -> Result<Option<String>, String> {
    if agent_id.is_empty() {
        return Ok(None);
    }

    let color = sqlx::query_scalar::<sqlx::Sqlite, Option<String>>(
        "SELECT dominant_color FROM avatars WHERE owner_id = ? AND owner_type = 'agent' AND deleted_at IS NULL",
    )
    .bind(agent_id)
    .fetch_optional(pool)
    .await
    .map_err(|error| format!("Avatar color query failed for {agent_id}: {error}"))?
    .flatten();
    Ok(color)
}

/// 批量 Push 单 topic 处理结果
pub struct PushBatchResult {
    pub topic: TopicKey,
    pub success: bool,
    pub error: Option<String>,
}

#[derive(Debug)]
struct MessageTombstone {
    key: MessageKey,
    deleted_at: i64,
}

struct TopicMessagePreflight {
    live_count: usize,
    tombstone_count: usize,
}

struct CountingWriter {
    bytes: usize,
    limit: usize,
}

impl Write for CountingWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.bytes = self
            .bytes
            .checked_add(bytes.len())
            .ok_or_else(|| std::io::Error::other("serialized byte count overflow"))?;
        if self.bytes > self.limit {
            return Err(std::io::Error::other(
                "serialized value exceeds byte budget",
            ));
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MessageTombstoneRequest<'a> {
    owner_type: &'a str,
    owner_id: &'a str,
    topic_id: &'a str,
    msg_id: &'a str,
    deleted_at: i64,
}

struct MessagePushFrame {
    outcome: PushBatchResult,
    needed_attachment_hashes: Vec<String>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum OutboundMessageSyncDTO {
    User(UserMessageSyncDTO),
    Agent(AgentMessageSyncDTO),
    Group(GroupMessageSyncDTO),
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

fn parse_message_push_frames(
    bytes: &[u8],
    expected_topics: &[TopicKey],
) -> Result<Vec<MessagePushFrame>, String> {
    let response_text = std::str::from_utf8(bytes)
        .map_err(|error| format!("Batch push response is not UTF-8: {error}"))?;
    let expected = expected_topics.iter().cloned().collect::<HashSet<_>>();
    let mut seen = HashSet::new();
    let mut frames = Vec::with_capacity(expected.len());
    for raw_line in response_text.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        if line.len() > MAX_NDJSON_LINE_BYTES {
            return Err("Batch push response contains a line over 32 MiB".to_string());
        }
        let data: serde_json::Value = serde_json::from_str(line)
            .map_err(|error| format!("Batch push response contains malformed NDJSON: {error}"))?;
        if let Some(error) = data.get("_stream_error") {
            return Err(encode_wire_sync_error_value(error)?);
        }
        let topic_id = data
            .get("topicId")
            .and_then(serde_json::Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| "Batch push response requires a non-empty topicId".to_string())?;
        let topic = TopicKey::new(
            data.get("ownerType")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default(),
            data.get("ownerId")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default(),
            topic_id,
        );
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
        let success = data
            .get("success")
            .and_then(serde_json::Value::as_bool)
            .ok_or_else(|| format!("Batch push result for {topic_id} requires boolean success"))?;
        let error = match data.get("error") {
            Some(value) => Some(encode_wire_sync_error_value(value).map_err(|parse_error| {
                format!("Batch push result for {topic_id} has invalid error: {parse_error}")
            })?),
            None => None,
        };
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

        let values = data
            .get("neededAttachmentHashes")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| {
                format!("neededAttachmentHashes for {topic_id} must be an explicit array")
            })?;
        let mut needed_attachment_hashes = Vec::with_capacity(values.len());
        let mut unique_hashes = HashSet::new();
        for value in values {
            let raw_hash = value.as_str().ok_or_else(|| {
                format!("neededAttachmentHashes for {topic_id} must contain strings")
            })?;
            let hash = canonical_sha256(raw_hash).ok_or_else(|| {
                format!("neededAttachmentHashes for {topic_id} contains an invalid hash")
            })?;
            if !unique_hashes.insert(hash.clone()) {
                return Err(format!(
                    "neededAttachmentHashes for {topic_id} contains duplicate hash {hash}"
                ));
            }
            needed_attachment_hashes.push(hash);
        }
        if !success && !needed_attachment_hashes.is_empty() {
            return Err(format!(
                "Failed batch push result for {topic_id} must not request attachments"
            ));
        }
        frames.push(MessagePushFrame {
            outcome: PushBatchResult {
                topic,
                success,
                error,
            },
            needed_attachment_hashes,
        });
    }

    if seen != expected {
        let mut missing = expected.difference(&seen).cloned().collect::<Vec<_>>();
        missing.sort();
        return Err(format!("Batch push response is missing topics {missing:?}"));
    }
    Ok(frames)
}

async fn send_message_chunk(
    client: &reqwest::Client,
    http_url: &str,
    sync_token: &str,
    body: Vec<u8>,
    expected_topics: &[TopicKey],
) -> Result<Vec<MessagePushFrame>, String> {
    let url = format!("{}/api/mobile-sync/upload-messages-batch", http_url);
    let response = client
        .post(&url)
        .header("x-sync-token", sync_token)
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
                "Batch push messages failed with HTTP {status} without a Wire 1.3 error object"
            )),
            Err(error) => Err(format!(
                "Batch push messages returned an invalid Wire 1.3 error: {error}"
            )),
        };
    }

    parse_message_push_frames(&bytes, expected_topics)
}

fn record_message_frames(
    frames: Vec<MessagePushFrame>,
    results: &mut Vec<PushBatchResult>,
    attachment_topics: &mut HashMap<String, HashSet<TopicKey>>,
) {
    for frame in frames {
        if frame.outcome.success {
            for hash in frame.needed_attachment_hashes {
                attachment_topics
                    .entry(hash)
                    .or_default()
                    .insert(frame.outcome.topic.clone());
            }
        }
        results.push(frame.outcome);
    }
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
                    COALESCE(LENGTH(CAST(ma.src AS BLOB)), 0) +
                    COALESCE(LENGTH(CAST(ma.status AS BLOB)), 0) +
                    COALESCE(LENGTH(CAST(a.mime_type AS BLOB)), 0) +
                    COALESCE(LENGTH(CAST(a.image_frames AS BLOB)), 0)
                ), 0)
         FROM message_attachments ma
         LEFT JOIN attachments a ON a.hash = ma.hash
         JOIN messages m ON m.owner_type = ma.owner_type AND m.owner_id = ma.owner_id
            AND m.topic_id = ma.topic_id AND m.msg_id = ma.msg_id
         WHERE ma.owner_type = ? AND ma.owner_id = ? AND ma.topic_id = ?
           AND ma.deleted_at IS NULL AND m.deleted_at IS NULL",
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
                key: MessageKey::new(key.clone(), message_id),
                deleted_at,
            })
        })
        .collect()
}

fn message_tombstone_body_len(tombstone: &MessageTombstone) -> Result<usize, String> {
    let request = MessageTombstoneRequest {
        owner_type: &tombstone.key.topic.owner_type,
        owner_id: &tombstone.key.topic.owner_id,
        topic_id: &tombstone.key.topic.topic_id,
        msg_id: &tombstone.key.msg_id,
        deleted_at: tombstone.deleted_at,
    };
    let mut writer = CountingWriter {
        bytes: 0,
        limit: MAX_CONTROL_RESPONSE_BYTES,
    };
    serde_json::to_writer(&mut writer, &request).map_err(|error| {
        format!(
            "Message tombstone {}/{} exceeds its serialized byte budget: {error}",
            tombstone.key.topic.topic_id, tombstone.key.msg_id
        )
    })?;
    Ok(writer.bytes)
}

fn validate_message_tombstone_response(
    value: &serde_json::Value,
    tombstone: &MessageTombstone,
) -> Result<(), String> {
    let topic_matches = value.get("topicId").and_then(serde_json::Value::as_str)
        == Some(tombstone.key.topic.topic_id.as_str());
    let owner_type_matches = value.get("ownerType").and_then(serde_json::Value::as_str)
        == Some(tombstone.key.topic.owner_type.as_str());
    let owner_id_matches = value.get("ownerId").and_then(serde_json::Value::as_str)
        == Some(tombstone.key.topic.owner_id.as_str());
    let message_matches = value.get("msgId").and_then(serde_json::Value::as_str)
        == Some(tombstone.key.msg_id.as_str());
    if !topic_matches || !owner_type_matches || !owner_id_matches || !message_matches {
        return Err(format!(
            "Delete message response requires matching owner, topicId and msgId for {}/{}",
            tombstone.key.topic.topic_id, tombstone.key.msg_id
        ));
    }
    Ok(())
}

async fn push_message_tombstone(
    client: &reqwest::Client,
    http_url: &str,
    sync_token: &str,
    tombstone: &MessageTombstone,
) -> Result<(), String> {
    let request = MessageTombstoneRequest {
        owner_type: &tombstone.key.topic.owner_type,
        owner_id: &tombstone.key.topic.owner_id,
        topic_id: &tombstone.key.topic.topic_id,
        msg_id: &tombstone.key.msg_id,
        deleted_at: tombstone.deleted_at,
    };
    let body = serde_json::to_vec(&request).map_err(|error| {
        format!(
            "Message tombstone {}/{} serialization failed: {error}",
            tombstone.key.topic.topic_id, tombstone.key.msg_id
        )
    })?;
    if body.len() > MAX_CONTROL_RESPONSE_BYTES {
        return Err(format!(
            "Message tombstone {}/{} exceeds the request byte limit",
            tombstone.key.topic.topic_id, tombstone.key.msg_id
        ));
    }
    let idempotency_key = crate::vcp_modules::infra::utils::calculate_sha256_slices(&[
        b"delete-message",
        tombstone.key.topic.owner_type.as_bytes(),
        tombstone.key.topic.owner_id.as_bytes(),
        tombstone.key.topic.topic_id.as_bytes(),
        tombstone.key.msg_id.as_bytes(),
        tombstone.deleted_at.to_string().as_bytes(),
    ]);
    let response = client
        .post(format!("{http_url}/api/mobile-sync/delete-message"))
        .header("x-sync-token", sync_token)
        .header("Authorization", format!("Bearer {sync_token}"))
        .header("x-idempotency-key", idempotency_key)
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
        .map_err(|error| {
            format!(
                "Delete message {}/{} request failed: {error}",
                tombstone.key.topic.topic_id, tombstone.key.msg_id
            )
        })?;
    let value = parse_success_response(response, "Delete message").await?;
    validate_message_tombstone_response(&value, tombstone)
}

fn append_topic_failure(result: &mut PushBatchResult, message: String) {
    result.success = false;
    match &mut result.error {
        Some(existing) if !existing.is_empty() => {
            existing.push_str("; ");
            existing.push_str(&message);
        }
        _ => result.error = Some(message),
    }
}

async fn push_message_tombstone_chunk(
    client: &reqwest::Client,
    http_url: &str,
    sync_token: &str,
    tombstones: Vec<MessageTombstone>,
    results: &mut [PushBatchResult],
) -> Result<(), String> {
    if tombstones.is_empty() {
        return Ok(());
    }
    let result_indexes = results
        .iter()
        .enumerate()
        .map(|(index, result)| (result.topic.clone(), index))
        .collect::<HashMap<_, _>>();
    const MAX_CONCURRENT_MESSAGE_DELETES: usize = 3;
    for chunk in tombstones.chunks(MAX_CONCURRENT_MESSAGE_DELETES) {
        let futures = chunk
            .iter()
            .map(|tombstone| push_message_tombstone(client, http_url, sync_token, tombstone));
        for (tombstone, outcome) in chunk
            .iter()
            .zip(futures_util::future::join_all(futures).await)
        {
            if let Err(error) = outcome {
                let index = result_indexes
                    .get(&tombstone.key.topic)
                    .copied()
                    .ok_or_else(|| {
                        format!(
                            "Message tombstone result references missing topic {}",
                            tombstone.key.topic.topic_id
                        )
                    })?;
                append_topic_failure(
                    &mut results[index],
                    format!("Message tombstone {} failed: {error}", tombstone.key.msg_id),
                );
            }
        }
    }
    Ok(())
}

async fn load_outbound_message_page(
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
    cursor: Option<(i64, &str)>,
) -> Result<Vec<crate::vcp_modules::chat_manager::ChatMessage>, String> {
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
        messages.push(crate::vcp_modules::chat_manager::ChatMessage {
            id: message_id,
            role: row
                .try_get("role")
                .map_err(|error| format!("Message role decode failed for {topic_id}: {error}"))?,
            name: row
                .try_get("name")
                .map_err(|error| format!("Message name decode failed for {topic_id}: {error}"))?,
            content: row.try_get("content").map_err(|error| {
                format!("Message content decode failed for {topic_id}: {error}")
            })?,
            timestamp,
            updated_at: Some(updated_at),
            is_thinking: Some(false),
            agent_id: row
                .try_get("agent_id")
                .map_err(|error| format!("Message agent decode failed for {topic_id}: {error}"))?,
            group_id: row
                .try_get("group_id")
                .map_err(|error| format!("Message group decode failed for {topic_id}: {error}"))?,
            topic_id: Some(topic_id.to_string()),
            is_group_message: Some(is_group_message != 0),
            finish_reason: row.try_get("finish_reason").map_err(|error| {
                format!("Message finish reason decode failed for {topic_id}: {error}")
            })?,
            attachments: None,
            blocks: None,
            shell: None,
            content_hash: (!content_hash.is_empty()).then_some(content_hash),
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
        "SELECT ma.msg_id, ma.hash AS relation_hash, ma.display_name, ma.src, ma.status,
                a.hash AS stored_hash, a.mime_type, a.size, a.internal_path,
                a.image_frames, a.thumbnail_path, a.created_at
         FROM message_attachments ma
         LEFT JOIN attachments a ON a.hash = ma.hash
         WHERE ma.owner_type = ? AND ma.owner_id = ? AND ma.topic_id = ?
           AND ma.msg_id IN ({placeholders}) AND ma.deleted_at IS NULL
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
        let relation_hash: String = row.try_get("relation_hash").map_err(|error| {
            format!("Attachment hash decode failed for {topic_id}/{message_id}: {error}")
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
        attachments_by_message
            .entry(message_id)
            .or_insert_with(Vec::new)
            .push(crate::vcp_modules::chat_manager::Attachment {
                r#type: row
                    .try_get::<Option<String>, _>("mime_type")
                    .map_err(|error| {
                        format!("Attachment MIME decode failed for {relation_hash}: {error}")
                    })?
                    .ok_or_else(|| format!("Attachment {relation_hash} has no MIME metadata"))?,
                src: row
                    .try_get::<Option<String>, _>("src")
                    .map_err(|error| {
                        format!("Attachment source decode failed for {relation_hash}: {error}")
                    })?
                    .unwrap_or_default(),
                name: row.try_get("display_name").map_err(|error| {
                    format!("Attachment name decode failed for {relation_hash}: {error}")
                })?,
                size,
                hash: Some(relation_hash),
                status: row
                    .try_get("status")
                    .map_err(|error| format!("Attachment status decode failed: {error}"))?,
                attachment_order: None,
                internal_path: row
                    .try_get::<Option<String>, _>("internal_path")
                    .map_err(|error| format!("Attachment path decode failed: {error}"))?
                    .ok_or_else(|| "Attachment has no local path metadata".to_string())?,
                extracted_text: None,
                image_frames,
                thumbnail_path: row
                    .try_get("thumbnail_path")
                    .map_err(|error| format!("Attachment thumbnail decode failed: {error}"))?,
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

async fn build_message_dto<R: Runtime>(
    app: &AppHandle<R>,
    message: crate::vcp_modules::chat_manager::ChatMessage,
    owner_type: &str,
    avatar_colors: &mut HashMap<String, String>,
) -> Result<OutboundMessageSyncDTO, String> {
    if message.id.is_empty() || message.role.is_empty() {
        return Err("Outbound messages require non-empty id and role".to_string());
    }
    if message.role == "user" {
        let attachments = message
            .attachments
            .map(|attachments| {
                attachments
                    .into_iter()
                    .map(|attachment| {
                        let hash = attachment
                            .hash
                            .as_deref()
                            .and_then(canonical_sha256)
                            .ok_or_else(|| {
                                format!(
                                    "Outbound message {} attachment {} has no valid SHA-256 hash",
                                    message.id, attachment.name
                                )
                            })?;
                        Ok(AttachmentSyncDTO {
                            r#type: attachment.r#type,
                            name: attachment.name,
                            size: attachment.size,
                            hash,
                            status: attachment.status,
                            extracted_text: attachment.extracted_text,
                            image_frames: attachment.image_frames,
                            created_at: attachment.created_at,
                        })
                    })
                    .collect::<Result<Vec<_>, String>>()
            })
            .transpose()?;
        return Ok(OutboundMessageSyncDTO::User(UserMessageSyncDTO {
            id: message.id,
            role: message.role,
            name: message.name,
            content: message.content,
            timestamp: message.timestamp,
            updated_at: message.updated_at.unwrap_or(message.timestamp),
            attachments,
            content_hash: message.content_hash,
        }));
    }

    if !matches!(owner_type, "agent" | "group") {
        return Err(format!(
            "Outbound message owner type {owner_type} is unsupported"
        ));
    }
    let agent_id = message.agent_id.unwrap_or_default();
    let avatar_color = if let Some(color) = avatar_colors.get(&agent_id) {
        color.clone()
    } else {
        let color = query_avatar_color(&app.state::<DbState>().pool, &agent_id)
            .await?
            .unwrap_or_else(|| "#6B7280".to_string());
        avatar_colors.insert(agent_id.clone(), color.clone());
        color
    };
    if owner_type == "group" {
        Ok(OutboundMessageSyncDTO::Group(GroupMessageSyncDTO {
            id: message.id,
            role: message.role,
            name: message.name,
            content: message.content,
            timestamp: message.timestamp,
            updated_at: message.updated_at.unwrap_or(message.timestamp),
            agent_id,
            group_id: message.group_id.unwrap_or_default(),
            topic_id: message.topic_id.unwrap_or_default(),
            is_group_message: true,
            avatar_color,
            content_hash: message.content_hash,
        }))
    } else {
        Ok(OutboundMessageSyncDTO::Agent(AgentMessageSyncDTO {
            id: message.id,
            role: message.role,
            name: message.name,
            content: message.content,
            timestamp: message.timestamp,
            updated_at: message.updated_at.unwrap_or(message.timestamp),
            agent_id,
            is_thinking: message.is_thinking,
            finish_reason: message.finish_reason,
            avatar_color,
            content_hash: message.content_hash,
        }))
    }
}

async fn serialize_topic_messages<R: Runtime>(
    app: &AppHandle<R>,
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
    expected_message_count: usize,
) -> Result<Vec<u8>, String> {
    let topic_id = &key.topic_id;
    let owner_type = &key.owner_type;
    let owner_id = &key.owner_id;
    let mut line = BoundedJsonLine::new(MAX_NDJSON_LINE_BYTES);
    line.write_all(br#"{"topicId":"#)
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
    let mut avatar_colors = HashMap::new();
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
            let dto = build_message_dto(app, message, owner_type, &mut avatar_colors).await?;
            if serialized_count > 0 {
                line.write_all(b",").map_err(|error| {
                    format!("Message push separator failed for {topic_id}: {error}")
                })?;
            }
            serde_json::to_writer(&mut line, &dto).map_err(|error| {
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

        let idempotency_key = generate_idempotency_key("push", "agent", agent_id);
        let url = format!("{}/api/mobile-sync/upload-entity", http_url);

        let response = client
            .post(&url)
            .header("x-sync-token", sync_token)
            .header("Authorization", format!("Bearer {}", sync_token))
            .header("x-idempotency-key", idempotency_key)
            .json(&serde_json::json!({ "id": agent_id, "type": "agent", "data": dto }))
            .send()
            .await
            .map_err(|error| format!("Push agent {agent_id} request failed: {error}"))?;
        let body = parse_success_response(response, "Push agent").await?;
        if body.get("id").and_then(serde_json::Value::as_str) != Some(agent_id) {
            return Err(format!("Push agent response id mismatch for {agent_id}"));
        }

        Ok(())
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

        let idempotency_key = generate_idempotency_key("push", "group", group_id);
        let url = format!("{}/api/mobile-sync/upload-entity", http_url);

        let response = client
            .post(&url)
            .header("x-sync-token", sync_token)
            .header("Authorization", format!("Bearer {}", sync_token))
            .header("x-idempotency-key", idempotency_key)
            .json(&serde_json::json!({ "id": group_id, "type": "group", "data": dto }))
            .send()
            .await
            .map_err(|error| format!("Push group {group_id} request failed: {error}"))?;
        let body = parse_success_response(response, "Push group").await?;
        if body.get("id").and_then(serde_json::Value::as_str) != Some(group_id) {
            return Err(format!("Push group response id mismatch for {group_id}"));
        }

        Ok(())
    }

    /// 批量 Push 实体 (Agent/Group/Topic)
    pub async fn push_entities_batch<R: Runtime>(
        _app: &AppHandle<R>,
        client: &reqwest::Client,
        http_url: &str,
        sync_token: &str,
        items: Vec<serde_json::Value>, // 预先构建好的 [{id, type, data}]
    ) -> Result<(), String> {
        if items.is_empty() {
            return Ok(());
        }

        let mut expected_topics = HashSet::new();
        for item in &items {
            let id = item
                .get("id")
                .and_then(serde_json::Value::as_str)
                .filter(|id| !id.is_empty())
                .ok_or_else(|| "Batch push entity item requires a non-empty id".to_string())?;
            let owner_type = item
                .get("ownerType")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let owner_id = item
                .get("ownerId")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let key = TopicKey::new(owner_type, owner_id, id);
            if !key.is_valid() || !expected_topics.insert(key) {
                return Err(format!(
                    "Batch push entity request contains an invalid or duplicate topic identity for {id}"
                ));
            }
        }
        let request_body = serde_json::json!({ "items": items });
        let request_size = serde_json::to_vec(&request_body)
            .map_err(|error| format!("Batch push entity serialization failed: {error}"))?
            .len();
        if request_size > 10 * 1024 * 1024 {
            return Err("Batch push entity request exceeds 10 MiB".to_string());
        }

        let url = format!("{}/api/mobile-sync/upload-entities-batch", http_url);
        let response = client
            .post(&url)
            .header("x-sync-token", sync_token)
            .header("Authorization", format!("Bearer {}", sync_token))
            .json(&request_body)
            .send()
            .await
            .map_err(|e| format!("Batch push request failed: {}", e))?;

        let response_body = parse_success_response(response, "Batch push entities").await?;
        let results = response_body
            .get("results")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| "Batch push entities response is missing results".to_string())?;
        let mut seen_topics = HashSet::new();
        for result in results {
            let id = result
                .get("id")
                .and_then(serde_json::Value::as_str)
                .filter(|id| !id.is_empty())
                .ok_or_else(|| "Batch push entity result requires a non-empty id".to_string())?;
            let key = TopicKey::new(
                result
                    .get("ownerType")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default(),
                result
                    .get("ownerId")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default(),
                id,
            );
            if !expected_topics.contains(&key) {
                return Err(format!(
                    "Batch push entities returned unexpected topic {id}"
                ));
            }
            if !seen_topics.insert(key) {
                return Err(format!("Batch push entities returned duplicate topic {id}"));
            }
            if result.get("success").and_then(serde_json::Value::as_bool) != Some(true) {
                let error = result
                    .get("error")
                    .ok_or_else(|| format!("Batch push entity {id} failure is missing error"))
                    .and_then(encode_wire_sync_error_value)?;
                return Err(format!("Batch push entity {id} failed: {error}"));
            }
            if result.get("error").is_some() {
                return Err(format!(
                    "Successful batch push entity {id} must not contain an error"
                ));
            }
        }
        if seen_topics != expected_topics {
            let mut missing = expected_topics
                .difference(&seen_topics)
                .cloned()
                .collect::<Vec<_>>();
            missing.sort();
            return Err(format!(
                "Batch push entities missing results for {missing:?}"
            ));
        }

        Ok(())
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
            "{}/api/mobile-sync/upload-avatar?id={}&type={}",
            http_url, owner_id, owner_type
        );
        let response = client
            .post(&url)
            .header("x-sync-token", sync_token)
            .header("Authorization", format!("Bearer {}", sync_token))
            .header("Content-Type", mime_type)
            .body(image_data)
            .send()
            .await
            .map_err(|error| format!("Push avatar {owner_type}/{owner_id} failed: {error}"))?;
        let body = parse_success_response(response, "Push avatar").await?;
        if body.get("id").and_then(serde_json::Value::as_str) != Some(owner_id) {
            return Err(format!(
                "Push avatar response id mismatch for {owner_type}/{owner_id}"
            ));
        }

        Ok(())
    }

    /// 批量 Push — 一次 HTTP 请求推送多个 topic 的消息
    ///
    /// 手机端批量加载消息 → POST /upload-messages-batch (NDJSON)
    /// → 解析响应收集 neededAttachmentHashes → 去重上传附件
    ///
    /// 返回每个 topic 的处理结果。
    pub async fn push_messages_batch<R: Runtime>(
        app: &AppHandle<R>,
        client: &reqwest::Client,
        http_url: &str,
        sync_token: &str,
        topic_keys: &[TopicKey],
        uploaded_hashes: Arc<RwLock<HashSet<String>>>,
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
        let mut attachment_topics: HashMap<String, HashSet<TopicKey>> = HashMap::new();
        let mut total_request_bytes = 0usize;
        let mut total_messages = 0usize;
        let mut request_body = Vec::new();
        let mut request_topics = Vec::new();
        let mut request_tombstones = Vec::new();

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
            for tombstone in &topic_tombstones {
                total_request_bytes = total_request_bytes
                    .checked_add(message_tombstone_body_len(tombstone)?)
                    .ok_or_else(|| "Message push byte count overflow".to_string())?;
                if total_request_bytes > MAX_SYNC_BODY_BYTES {
                    return Err("Message push exceeds the 256 MiB total limit".to_string());
                }
            }

            let line =
                serialize_topic_messages(app, &mut read_tx, key, preflight.live_count).await?;
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
                let frames = send_message_chunk(
                    client,
                    http_url,
                    sync_token,
                    std::mem::take(&mut request_body),
                    &request_topics,
                )
                .await?;
                record_message_frames(frames, &mut results, &mut attachment_topics);
                push_message_tombstone_chunk(
                    client,
                    http_url,
                    sync_token,
                    std::mem::take(&mut request_tombstones),
                    &mut results,
                )
                .await?;
                request_topics.clear();
            }
            request_tombstones.extend(topic_tombstones);
            if request_body.is_empty() {
                request_body = line;
            } else {
                request_body.extend_from_slice(&line);
            }
            request_topics.push(key.clone());
            if request_body.len() >= MESSAGE_REQUEST_CHUNK_BYTES {
                let frames = send_message_chunk(
                    client,
                    http_url,
                    sync_token,
                    std::mem::take(&mut request_body),
                    &request_topics,
                )
                .await?;
                record_message_frames(frames, &mut results, &mut attachment_topics);
                push_message_tombstone_chunk(
                    client,
                    http_url,
                    sync_token,
                    std::mem::take(&mut request_tombstones),
                    &mut results,
                )
                .await?;
                request_topics.clear();
            }
        }

        if !request_body.is_empty() {
            let frames =
                send_message_chunk(client, http_url, sync_token, request_body, &request_topics)
                    .await?;
            record_message_frames(frames, &mut results, &mut attachment_topics);
            push_message_tombstone_chunk(
                client,
                http_url,
                sync_token,
                request_tombstones,
                &mut results,
            )
            .await?;
        }

        // Online WebSocket notifications are only a latency optimization. Tombstones are also
        // replayed through the acknowledged HTTP endpoint so deletions made while disconnected
        // converge on the next Phase 3 push. Tombstones are released with each 8 MiB request
        // chunk rather than accumulating up to the 256 MiB attempt budget in resident memory.
        let result_indexes = results
            .iter()
            .enumerate()
            .map(|(index, result)| (result.topic.clone(), index))
            .collect::<HashMap<_, _>>();

        let hashes_to_upload = {
            let tracker = uploaded_hashes.read().await;
            attachment_topics
                .keys()
                .filter(|hash| !tracker.contains(*hash))
                .cloned()
                .collect::<Vec<_>>()
        };
        let mut attachment_failures = HashMap::new();
        const MAX_CONCURRENT_UPLOADS: usize = 3;
        for chunk in hashes_to_upload.chunks(MAX_CONCURRENT_UPLOADS) {
            let futures = chunk
                .iter()
                .map(|hash| upload_attachment(app, client, http_url, sync_token, hash));
            for (hash, result) in chunk
                .iter()
                .zip(futures_util::future::join_all(futures).await)
            {
                match result {
                    Ok(()) => {
                        uploaded_hashes.write().await.insert(hash.clone());
                    }
                    Err(error) => {
                        attachment_failures.insert(hash.clone(), error);
                    }
                }
            }
        }

        if !attachment_failures.is_empty() {
            for (hash, error) in attachment_failures {
                if let Some(topics) = attachment_topics.get(&hash) {
                    for topic in topics {
                        if let Some(index) = result_indexes.get(topic).copied() {
                            append_topic_failure(
                                &mut results[index],
                                format!(
                                    "Attachment {hash} required by topic {} failed: {error}",
                                    topic.topic_id
                                ),
                            );
                        }
                    }
                }
            }
        }

        let ok_count = results.iter().filter(|r| r.success).count();
        log::info!(
            "[PushExecutor] Batch push completed: {}/{} topics",
            ok_count,
            topic_keys.len()
        );
        Ok(results)
    }
}

fn generate_idempotency_key(action: &str, entity_type: &str, id: &str) -> String {
    let now = chrono::Utc::now().timestamp() / 60;
    let now_str = now.to_string();
    crate::vcp_modules::infra::utils::calculate_sha256_slices(&[
        action.as_bytes(),
        entity_type.as_bytes(),
        id.as_bytes(),
        now_str.as_bytes(),
    ])
}

async fn upload_attachment<R: Runtime>(
    app: &AppHandle<R>,
    client: &reqwest::Client,
    http_url: &str,
    sync_token: &str,
    hash: &str,
) -> Result<(), String> {
    let hash = canonical_sha256(hash)
        .ok_or_else(|| "Attachment upload requires a valid SHA-256 hash".to_string())?;
    let db = app.state::<DbState>();

    let row = sqlx::query("SELECT mime_type, internal_path FROM attachments WHERE hash = ?")
        .bind(&hash)
        .fetch_optional(&db.pool)
        .await
        .map_err(|e| e.to_string())?;

    let att_row =
        row.ok_or_else(|| format!("Attachment {hash} is missing from the local index"))?;
    let mime_type: String = att_row
        .try_get("mime_type")
        .map_err(|error| format!("Attachment {hash} MIME decode failed: {error}"))?;
    let internal_path: String = att_row
        .try_get("internal_path")
        .map_err(|error| format!("Attachment {hash} path decode failed: {error}"))?;
    if internal_path.trim().is_empty() {
        return Err(format!("Attachment {hash} has no local file path"));
    }

    let name_row = sqlx::query("SELECT display_name FROM message_attachments WHERE hash = ? AND deleted_at IS NULL LIMIT 1")
        .bind(&hash)
        .fetch_optional(&db.pool)
        .await
        .map_err(|error| format!("Attachment {hash} display name query failed: {error}"))?;
    let display_name = match name_row {
        Some(row) => {
            let name: String = row.try_get("display_name").map_err(|error| {
                format!("Attachment {hash} display name decode failed: {error}")
            })?;
            if name.is_empty() {
                return Err(format!("Attachment {hash} has an empty display name"));
            }
            name
        }
        None => return Err(format!("Attachment {hash} has no live message relation")),
    };

    let file_path = internal_path.trim_start_matches("file://");
    let mut file = tokio::fs::File::open(file_path)
        .await
        .map_err(|error| format!("Attachment {hash} read failed: {error}"))?;
    let metadata = file
        .metadata()
        .await
        .map_err(|error| format!("Attachment {hash} metadata read failed: {error}"))?;
    if !metadata.is_file() {
        return Err(format!("Attachment {hash} path is not a regular file"));
    }
    if metadata.len() > MAX_ATTACHMENT_UPLOAD_BYTES {
        return Err(format!(
            "Attachment {hash} exceeds the 512 MiB upload limit"
        ));
    }
    let mut hasher = Sha256::new();
    let mut hash_buffer = vec![0u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut hash_buffer)
            .await
            .map_err(|error| format!("Attachment {hash} hash read failed: {error}"))?;
        if read == 0 {
            break;
        }
        hasher.update(&hash_buffer[..read]);
    }
    let actual_hash = format!("{:x}", hasher.finalize());
    if actual_hash != hash {
        return Err(format!(
            "Attachment {hash} content hash mismatch (actual {actual_hash})"
        ));
    }
    file.seek(std::io::SeekFrom::Start(0))
        .await
        .map_err(|error| format!("Attachment {hash} rewind failed: {error}"))?;

    let url = format!(
        "{}/api/mobile-sync/upload-attachment?hash={}&type={}&name={}",
        http_url,
        hash,
        urlencoding::encode(&mime_type),
        urlencoding::encode(&display_name)
    );

    let response = client
        .post(&url)
        .header("x-sync-token", sync_token)
        .header("Authorization", format!("Bearer {}", sync_token))
        .header("Content-Type", "application/octet-stream")
        .header(reqwest::header::CONTENT_LENGTH, metadata.len())
        .body(reqwest::Body::wrap_stream(ReaderStream::with_capacity(
            file,
            64 * 1024,
        )))
        .send()
        .await
        .map_err(|error| format!("Attachment {hash} upload failed: {error}"))?;
    let body = parse_success_response(response, "Attachment upload").await?;
    let response_hash = body
        .get("hash")
        .and_then(serde_json::Value::as_str)
        .and_then(canonical_sha256)
        .ok_or_else(|| "Attachment upload response requires a valid hash".to_string())?;
    if response_hash != hash {
        return Err(format!(
            "Attachment upload response hash mismatch: expected {hash}, got {response_hash}"
        ));
    }

    log::debug!("[PushExecutor] Attachment uploaded: {}", hash);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_hash_is_lowercase_and_rejects_non_sha256_values() {
        assert_eq!(canonical_sha256(&"A".repeat(64)), Some("a".repeat(64)));
        assert_eq!(canonical_sha256(""), None);
        assert_eq!(canonical_sha256(&"g".repeat(64)), None);
        assert_eq!(canonical_sha256(&"a".repeat(63)), None);
    }

    #[test]
    fn attachment_dependencies_are_recorded_per_successful_topic() {
        let hash = "a".repeat(64);
        let frames = vec![
            MessagePushFrame {
                outcome: PushBatchResult {
                    topic_id: "topic-a".to_string(),
                    success: true,
                    error: None,
                },
                needed_attachment_hashes: vec![hash.clone()],
            },
            MessagePushFrame {
                outcome: PushBatchResult {
                    topic_id: "topic-b".to_string(),
                    success: false,
                    error: Some("rejected".to_string()),
                },
                needed_attachment_hashes: Vec::new(),
            },
        ];
        let mut results = Vec::new();
        let mut dependencies = HashMap::new();
        record_message_frames(frames, &mut results, &mut dependencies);

        assert_eq!(results.len(), 2);
        assert_eq!(
            dependencies.get(&hash),
            Some(&HashSet::from(["topic-a".to_string()]))
        );
    }

    #[test]
    fn message_push_result_requires_explicit_attachment_hash_array() {
        let expected = vec!["topic".to_string()];
        let missing = br#"{"topicId":"topic","success":true}"#;
        let error = match parse_message_push_frames(missing, &expected) {
            Err(error) => error,
            Ok(_) => panic!("legacy result without neededAttachmentHashes must be rejected"),
        };
        assert!(error.contains("explicit array"));

        let valid = br#"{"topicId":"topic","success":true,"neededAttachmentHashes":[]}"#;
        let frames = parse_message_push_frames(valid, &expected).expect("hard-cut result");
        assert_eq!(frames.len(), 1);
        assert!(frames[0].needed_attachment_hashes.is_empty());
    }

    #[test]
    fn failed_message_push_preserves_wire_error_and_rejects_legacy_strings() {
        let expected = vec!["topic".to_string()];
        let valid = serde_json::to_vec(&serde_json::json!({
            "topicId":"topic",
            "success":false,
            "neededAttachmentHashes":[],
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
        let frames = parse_message_push_frames(&valid, &expected).expect("Wire 1.2 result");
        assert_eq!(
            crate::vcp_modules::sync_error::decode_wire_sync_error(
                frames[0].outcome.error.as_deref().expect("encoded error")
            )
            .expect("wire error")
            .code,
            "SYNC_OWNER_CONFLICT"
        );

        let legacy = serde_json::to_vec(&serde_json::json!({
            "topicId":"topic",
            "success":false,
            "neededAttachmentHashes":[],
            "error":"legacy"
        }))
        .expect("serialize legacy result");
        assert!(parse_message_push_frames(&legacy, &expected).is_err());

        let contradictory = serde_json::to_vec(&serde_json::json!({
            "topicId":"topic",
            "success":true,
            "neededAttachmentHashes":[],
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
        assert!(parse_message_push_frames(&contradictory, &expected).is_err());
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
                msg_id TEXT NOT NULL, topic_id TEXT NOT NULL, role TEXT NOT NULL,
                name TEXT, agent_id TEXT, content TEXT NOT NULL, timestamp BIGINT NOT NULL,
                is_group_message INTEGER NOT NULL, group_id TEXT, finish_reason TEXT,
                content_hash TEXT NOT NULL, deleted_at BIGINT,
                PRIMARY KEY(topic_id, msg_id)
             );
             CREATE TABLE message_attachments (
                topic_id TEXT, msg_id TEXT, hash TEXT, attachment_order INTEGER,
                display_name TEXT, src TEXT, status TEXT, deleted_at BIGINT
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
                    msg_id, topic_id, role, content, timestamp, is_group_message, content_hash
                 ) VALUES (?, 'topic', 'user', 'body', ?, 0, 'hash')",
            )
            .bind(format!("message-{index:03}"))
            .bind(index as i64)
            .execute(&pool)
            .await
            .expect("insert message");
        }
        sqlx::query(
            "INSERT INTO messages (
                msg_id, topic_id, role, content, timestamp, is_group_message,
                content_hash, deleted_at
             ) VALUES ('message-deleted', 'topic', 'user', 'gone', 200, 0, 'DELETED', 1234)",
        )
        .execute(&pool)
        .await
        .expect("insert tombstone");

        let mut read_tx = pool.begin().await.expect("begin snapshot");
        let preflight = preflight_topic_messages(&mut read_tx, "topic")
            .await
            .expect("preflight");
        assert_eq!(preflight.live_count, MESSAGE_PAGE_SIZE + 1);
        assert_eq!(preflight.tombstone_count, 1);
        let tombstones = load_message_tombstones(&mut read_tx, "topic", 1)
            .await
            .expect("load tombstone");
        assert_eq!(tombstones[0].message_id, "message-deleted");
        assert_eq!(tombstones[0].deleted_at, 1234);
        let first = load_outbound_message_page(&mut read_tx, "topic", None)
            .await
            .expect("first page");
        assert_eq!(first.len(), MESSAGE_PAGE_SIZE);
        assert_eq!(first.first().unwrap().id, "message-000");
        assert_eq!(first.last().unwrap().id, "message-099");
        let second = load_outbound_message_page(&mut read_tx, "topic", Some((99, "message-099")))
            .await
            .expect("second page");
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].id, "message-100");
    }

    #[test]
    fn tombstone_request_is_exactly_budgeted_and_response_identity_is_checked() {
        let tombstone = MessageTombstone {
            topic_id: "topic-a".to_string(),
            message_id: "message-a".to_string(),
            deleted_at: 1234,
        };
        let request = MessageTombstoneRequest {
            topic_id: &tombstone.topic_id,
            msg_id: &tombstone.message_id,
            deleted_at: tombstone.deleted_at,
        };
        assert_eq!(
            message_tombstone_body_len(&tombstone).expect("count body"),
            serde_json::to_vec(&request).expect("serialize body").len()
        );
        validate_message_tombstone_response(
            &serde_json::json!({
                "success": true,
                "topicId": "topic-a",
                "msgId": "message-a"
            }),
            &tombstone,
        )
        .expect("matching identity");
        assert!(validate_message_tombstone_response(
            &serde_json::json!({
                "success": true,
                "topicId": "topic-b",
                "msgId": "message-a"
            }),
            &tombstone,
        )
        .is_err());
        for malformed in [
            serde_json::json!({ "success": true, "msgId": "message-a" }),
            serde_json::json!({ "success": true, "topicId": "topic-a" }),
            serde_json::json!({
                "success": true,
                "topicId": 1,
                "msgId": "message-a"
            }),
            serde_json::json!({
                "success": true,
                "topicId": "topic-a",
                "msgId": 1
            }),
        ] {
            assert!(validate_message_tombstone_response(&malformed, &tombstone).is_err());
        }
    }
}

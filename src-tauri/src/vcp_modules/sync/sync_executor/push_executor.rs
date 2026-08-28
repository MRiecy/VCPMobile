use crate::vcp_modules::agent_service;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::group_service;
use crate::vcp_modules::sync_dto::{AgentSyncDTO, AttachmentSyncDTO, GroupSyncDTO, MessageSyncDTO};
use crate::vcp_modules::sync_error::{
    decode_wire_sync_error, encode_http_sync_error_body, encode_local_sync_error,
    encode_wire_sync_error, SyncErrorStage,
};
use crate::vcp_modules::sync_hash::HashAggregator;
use crate::vcp_modules::sync_types::{
    AvatarOwnerType, AvatarPushResponse, EntityPushData, EntityPushItem, EntityPushRequest,
    EntityPushResponse, EntitySelector, MessagePushResponseFrame, OwnerType,
};
use crate::vcp_modules::topic_types::TopicKey;
use futures_util::StreamExt;
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
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
const OWNER_PUSH_IDEMPOTENCY_DOMAIN: &[u8] = b"VCPMobileSync.OwnerPush.Idempotency.v1";

async fn read_response_limited(
    response: reqwest::Response,
    max_bytes: usize,
    operation: &str,
    stage: SyncErrorStage,
) -> Result<(reqwest::StatusCode, Vec<u8>), String> {
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        return Err(encode_local_sync_error(
            "RESPONSE_TOO_LARGE",
            stage,
            &format!("{operation} response exceeds {max_bytes} bytes"),
            Vec::new(),
        ));
    }
    let status = response.status();
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| {
            encode_local_sync_error(
                "HTTP_TRANSPORT_FAILED",
                stage,
                &format!("{operation} response read failed: {error}"),
                Vec::new(),
            )
        })?;
        if body.len().saturating_add(chunk.len()) > max_bytes {
            return Err(encode_local_sync_error(
                "RESPONSE_TOO_LARGE",
                stage,
                &format!("{operation} response exceeds {max_bytes} bytes"),
                Vec::new(),
            ));
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
    stage: SyncErrorStage,
) -> Result<T, String> {
    let (status, bytes) =
        read_response_limited(response, MAX_CONTROL_RESPONSE_BYTES, operation, stage).await?;
    if !status.is_success() {
        return match encode_http_sync_error_body(&bytes) {
            Ok(Some(encoded)) => Err(encoded),
            Ok(None) => Err(protocol_error(
                stage,
                format!("{operation} failed with HTTP {status} without a Wire 1.4 error object"),
            )),
            Err(error) => Err(protocol_error(
                stage,
                format!("{operation} returned an invalid Wire 1.4 error: {error}"),
            )),
        };
    }
    serde_json::from_slice(&bytes).map_err(|error| {
        protocol_error(stage, format!("{operation} returned invalid JSON: {error}"))
    })
}

fn http_transport_error(operation: &str, stage: SyncErrorStage, error: &reqwest::Error) -> String {
    encode_local_sync_error(
        "HTTP_TRANSPORT_FAILED",
        stage,
        &format!("{operation} failed: {error}"),
        Vec::new(),
    )
}

fn protocol_error(stage: SyncErrorStage, message: impl AsRef<str>) -> String {
    encode_local_sync_error("SYNC_PROTOCOL_INVALID", stage, message.as_ref(), Vec::new())
}

fn ensure_protocol_error(error: String, stage: SyncErrorStage) -> String {
    if decode_wire_sync_error(&error).is_some() {
        error
    } else {
        protocol_error(stage, error)
    }
}

/// 批量 Push 单 topic 处理结果
pub struct PushBatchResult {
    pub topic: TopicKey,
    pub success: bool,
    pub error: Option<String>,
}

struct SerializedTopicMessages {
    line: Vec<u8>,
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
        .map_err(|error| {
            http_transport_error("Batch push request", SyncErrorStage::Messages, &error)
        })?;
    let (status, bytes) = read_response_limited(
        response,
        MAX_NDJSON_LINE_BYTES,
        "Batch push",
        SyncErrorStage::Messages,
    )
    .await?;
    if !status.is_success() {
        return match encode_http_sync_error_body(&bytes) {
            Ok(Some(encoded)) => Err(encoded),
            Ok(None) => Err(protocol_error(
                SyncErrorStage::Messages,
                format!(
                    "Batch push messages failed with HTTP {status} without a Wire 1.4 error object"
                ),
            )),
            Err(error) => Err(protocol_error(
                SyncErrorStage::Messages,
                format!("Batch push messages returned an invalid Wire 1.4 error: {error}"),
            )),
        };
    }

    parse_message_push_results(&bytes, expected_topics)
        .map_err(|error| ensure_protocol_error(error, SyncErrorStage::Messages))
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
    let response_stage = if expected
        .iter()
        .any(|item| matches!(item, EntitySelector::Topic { .. }))
    {
        SyncErrorStage::TopicMetadata
    } else {
        SyncErrorStage::OwnerMetadata
    };

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
        .map_err(|error| http_transport_error("Entity push request", response_stage, &error))?;
    let (status, bytes) =
        read_response_limited(response, 10 * 1024 * 1024, "Entity push", response_stage).await?;
    if !status.is_success() {
        return match encode_http_sync_error_body(&bytes) {
            Ok(Some(encoded)) => Err(encoded),
            Ok(None) => Err(protocol_error(
                response_stage,
                format!("Entity push failed with HTTP {status} without a Wire 1.4 error object"),
            )),
            Err(error) => Err(protocol_error(
                response_stage,
                format!("Entity push returned an invalid Wire 1.4 error: {error}"),
            )),
        };
    }
    let response: EntityPushResponse = serde_json::from_slice(&bytes).map_err(|error| {
        protocol_error(
            response_stage,
            format!("Entity push returned invalid JSON: {error}"),
        )
    })?;
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

async fn load_outbound_message_page(
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
    cursor: Option<(i64, &str)>,
) -> Result<Vec<MessageSyncDTO>, String> {
    let topic_id = &key.topic_id;
    let mut query = if cursor.is_some() {
        sqlx::query(
            "SELECT msg_id, role, name, agent_id, content, timestamp, is_group_message,
                    group_id, finish_reason, updated_at
             FROM messages
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL
               AND (timestamp > ? OR (timestamp = ? AND msg_id > ?))
             ORDER BY timestamp ASC, msg_id ASC
             LIMIT ?",
        )
    } else {
        sqlx::query(
            "SELECT msg_id, role, name, agent_id, content, timestamp, is_group_message,
                    group_id, finish_reason, updated_at
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
            content_hash: None,
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
) -> Result<SerializedTopicMessages, String> {
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
            if serialized_count > MAX_MESSAGES_PER_TOPIC {
                return Err(format!(
                    "Message push topic {topic_id} exceeds the {MAX_MESSAGES_PER_TOPIC}-message limit"
                ));
            }
            cursor = Some(next_cursor);
        }
        if page_len < MESSAGE_PAGE_SIZE {
            break;
        }
    }
    line.write_all(b"],\"deletedMessages\":[")
        .map_err(|error| format!("Message tombstone prefix failed for {topic_id}: {error}"))?;
    let mut tombstone_count = 0usize;
    let mut tombstones = sqlx::query(
        "SELECT msg_id, deleted_at FROM messages
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NOT NULL
         ORDER BY deleted_at ASC, msg_id ASC",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(topic_id)
    .fetch(&mut **tx);
    while let Some(row) = tombstones.next().await {
        let row =
            row.map_err(|error| format!("Message tombstone query failed for {topic_id}: {error}"))?;
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
        if tombstone_count > 0 {
            line.write_all(b",").map_err(|error| {
                format!("Message tombstone separator failed for {topic_id}: {error}")
            })?;
        }
        serde_json::to_writer(
            &mut line,
            &serde_json::json!({
                "msgId": &message_id,
                "deletedAt": deleted_at,
            }),
        )
        .map_err(|error| {
            format!(
                "Message push topic {topic_id} tombstone {} is invalid: {error}",
                message_id
            )
        })?;
        tombstone_count = tombstone_count
            .checked_add(1)
            .ok_or_else(|| "Message tombstone count overflow".to_string())?;
        if serialized_count.saturating_add(tombstone_count) > MAX_MESSAGES_PER_TOPIC {
            return Err(format!(
                "Message push topic {topic_id} exceeds the {MAX_MESSAGES_PER_TOPIC}-message limit"
            ));
        }
    }
    drop(tombstones);
    line.write_all(b"]}\n")
        .map_err(|error| format!("Message push suffix failed for {topic_id}: {error}"))?;
    Ok(SerializedTopicMessages {
        line: line.into_bytes(),
        live_count: serialized_count,
        tombstone_count,
    })
}

pub struct PushExecutor;

async fn load_owner_push_version(
    pool: &sqlx::SqlitePool,
    owner_type: OwnerType,
    owner_id: &str,
    expected_config_hash: &str,
) -> Result<i64, String> {
    let query = match owner_type {
        OwnerType::Agent => {
            "SELECT config_hash, updated_at FROM agents
             WHERE owner_type = 'agent' AND agent_id = ? AND deleted_at IS NULL"
        }
        OwnerType::Group => {
            "SELECT config_hash, updated_at FROM groups
             WHERE owner_type = 'group' AND group_id = ? AND deleted_at IS NULL"
        }
    };
    let (stored_config_hash, updated_at): (String, i64) = sqlx::query_as(query)
        .bind(owner_id)
        .fetch_optional(pool)
        .await
        .map_err(|error| {
            format!("Owner push version query failed for {owner_type}/{owner_id}: {error}")
        })?
        .ok_or_else(|| format!("Owner {owner_type}/{owner_id} is unavailable for push"))?;
    if stored_config_hash != expected_config_hash {
        return Err(format!(
            "Owner {owner_type}/{owner_id} changed while preparing its push snapshot"
        ));
    }
    if updated_at < 0 {
        return Err(format!(
            "Owner {owner_type}/{owner_id} has an invalid negative update version"
        ));
    }
    Ok(updated_at)
}

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
        let db = app.state::<DbState>();
        let config_updated_at =
            load_owner_push_version(&db.pool, OwnerType::Agent, agent_id, &config_hash).await?;

        let idempotency_key = generate_owner_push_idempotency_key(
            OwnerType::Agent,
            agent_id,
            &config_hash,
            config_updated_at,
        );
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
        let db = app.state::<DbState>();
        let config_updated_at =
            load_owner_push_version(&db.pool, OwnerType::Group, group_id, &config_hash).await?;

        let idempotency_key = generate_owner_push_idempotency_key(
            OwnerType::Group,
            group_id,
            &config_hash,
            config_updated_at,
        );
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
            .map_err(|error| {
                http_transport_error(
                    &format!("Push avatar {owner_type}/{owner_id}"),
                    SyncErrorStage::OwnerMetadata,
                    &error,
                )
            })?;
        let body: AvatarPushResponse =
            parse_json_response(response, "Push avatar", SyncErrorStage::OwnerMetadata).await?;
        let expected_owner_type = AvatarOwnerType::try_from(owner_type)
            .map_err(|_| format!("Invalid avatar owner type {owner_type}"))?;
        if !body.ok || body.owner_type != expected_owner_type || body.owner_id != owner_id {
            return Err(protocol_error(
                SyncErrorStage::OwnerMetadata,
                format!("Push avatar response identity mismatch for {owner_type}/{owner_id}"),
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
            let serialized = serialize_topic_messages(&mut read_tx, key).await?;
            let topic_message_count = serialized
                .live_count
                .checked_add(serialized.tombstone_count)
                .ok_or_else(|| format!("Message push count overflow for {topic_id}"))?;
            total_messages = total_messages
                .checked_add(topic_message_count)
                .ok_or_else(|| "Message push count overflow".to_string())?;
            if total_messages > MAX_SYNC_MESSAGES {
                return Err(format!(
                    "Message push contains more than {MAX_SYNC_MESSAGES} messages"
                ));
            }
            read_tx.commit().await.map_err(|error| {
                format!("Message push snapshot close failed for {topic_id}: {error}")
            })?;
            let line = serialized.line;
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

fn generate_owner_push_idempotency_key(
    owner_type: OwnerType,
    owner_id: &str,
    config_hash: &str,
    config_updated_at: i64,
) -> String {
    let owner_type = owner_type.as_str();
    let version = config_updated_at.to_be_bytes();
    let mut hasher = Sha256::new();
    hasher.update(OWNER_PUSH_IDEMPOTENCY_DOMAIN);
    for field in [
        owner_type.as_bytes(),
        owner_id.as_bytes(),
        config_hash.as_bytes(),
        version.as_slice(),
    ] {
        hasher.update((field.len() as u64).to_be_bytes());
        hasher.update(field);
    }
    crate::vcp_modules::infra::utils::finalize_sha256_hex(hasher)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn topic(topic_id: &str) -> TopicKey {
        TopicKey::new("agent", "agent-a", topic_id)
    }

    #[test]
    fn owner_push_idempotency_key_has_stable_framed_contract() {
        let config_hash = "0123456789abcdef".repeat(4);
        let key = generate_owner_push_idempotency_key(
            OwnerType::Agent,
            "owner\0雪😀",
            &config_hash,
            1_725_000_123_456,
        );

        assert_eq!(
            key,
            "5e9b7ce802ae02e30bd28e7e745b82a895581eb9afdd7b1858f67dd87db0e7d7"
        );
        assert_eq!(key.len(), 64);
        assert!(key
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
        assert_eq!(
            key,
            generate_owner_push_idempotency_key(
                OwnerType::Agent,
                "owner\0雪😀",
                &config_hash,
                1_725_000_123_456,
            )
        );
    }

    #[test]
    fn owner_push_idempotency_key_binds_identity_content_and_version() {
        let config_hash = "a".repeat(64);
        let baseline =
            generate_owner_push_idempotency_key(OwnerType::Agent, "owner-a", &config_hash, 100);

        assert_ne!(
            baseline,
            generate_owner_push_idempotency_key(OwnerType::Group, "owner-a", &config_hash, 100,)
        );
        assert_ne!(
            baseline,
            generate_owner_push_idempotency_key(OwnerType::Agent, "owner-b", &config_hash, 100,)
        );
        assert_ne!(
            baseline,
            generate_owner_push_idempotency_key(OwnerType::Agent, "owner-a", &"b".repeat(64), 100,)
        );
        assert_ne!(
            baseline,
            generate_owner_push_idempotency_key(OwnerType::Agent, "owner-a", &config_hash, 101,)
        );
        assert_ne!(
            generate_owner_push_idempotency_key(OwnerType::Agent, "a", "bc", 100),
            generate_owner_push_idempotency_key(OwnerType::Agent, "ab", "c", 100)
        );
    }

    #[tokio::test]
    async fn owner_push_version_requires_the_same_live_config_snapshot() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open database");
        sqlx::query(
            "CREATE TABLE agents (
                owner_type TEXT NOT NULL, agent_id TEXT NOT NULL, config_hash TEXT NOT NULL,
                updated_at BIGINT NOT NULL, deleted_at BIGINT
             );
             CREATE TABLE groups (
                owner_type TEXT NOT NULL, group_id TEXT NOT NULL, config_hash TEXT NOT NULL,
                updated_at BIGINT NOT NULL, deleted_at BIGINT
             );
             INSERT INTO agents VALUES ('agent', 'agent-a', 'agent-hash', 101, NULL);
             INSERT INTO groups VALUES ('group', 'group-a', 'group-hash', 202, NULL);",
        )
        .execute(&pool)
        .await
        .expect("create owner fixtures");

        assert_eq!(
            load_owner_push_version(&pool, OwnerType::Agent, "agent-a", "agent-hash")
                .await
                .expect("matching agent snapshot"),
            101
        );
        assert_eq!(
            load_owner_push_version(&pool, OwnerType::Group, "group-a", "group-hash")
                .await
                .expect("matching group snapshot"),
            202
        );
        assert!(
            load_owner_push_version(&pool, OwnerType::Agent, "agent-a", "stale-hash")
                .await
                .expect_err("stale snapshot must fail")
                .contains("changed while preparing")
        );
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
                extracted_text TEXT, image_frames TEXT, thumbnail_path TEXT, created_at BIGINT
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
        let serialized = serialize_topic_messages(&mut read_tx, &key)
            .await
            .expect("serialize topic snapshot");
        assert_eq!(serialized.live_count, MESSAGE_PAGE_SIZE + 1);
        assert_eq!(serialized.tombstone_count, 1);
        let frame: serde_json::Value =
            serde_json::from_slice(&serialized.line).expect("serialized topic frame");
        assert_eq!(frame["deletedMessages"][0]["msgId"], "message-deleted");
        assert_eq!(frame["deletedMessages"][0]["deletedAt"], 1234);
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

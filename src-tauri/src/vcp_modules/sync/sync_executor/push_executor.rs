use crate::vcp_modules::agent_service;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::group_service;
use crate::vcp_modules::sync_dto::{
    AgentMessageSyncDTO, AgentSyncDTO, GroupMessageSyncDTO, GroupSyncDTO, UserMessageSyncDTO,
};
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use sqlx::Row;
use std::collections::{HashMap, HashSet};
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
        let body = String::from_utf8_lossy(&bytes);
        return Err(format!("{operation} failed: HTTP {status} body={body}"));
    }
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("{operation} returned invalid JSON: {error}"))?;
    if value.get("success").and_then(serde_json::Value::as_bool) != Some(true) {
        return Err(format!("{operation} returned success=false: {value}"));
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
    pub topic_id: String,
    pub success: bool,
    pub error: Option<String>,
}

struct MessagePushFrame {
    outcome: PushBatchResult,
    needed_attachment_hashes: Vec<String>,
}

fn parse_message_push_frames(
    bytes: &[u8],
    expected_topic_ids: &[String],
) -> Result<Vec<MessagePushFrame>, String> {
    let response_text = std::str::from_utf8(bytes)
        .map_err(|error| format!("Batch push response is not UTF-8: {error}"))?;
    let expected = expected_topic_ids.iter().cloned().collect::<HashSet<_>>();
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
        let topic_id = data
            .get("topicId")
            .and_then(serde_json::Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| "Batch push response requires a non-empty topicId".to_string())?;
        if !expected.contains(topic_id) {
            return Err(format!(
                "Batch push response contains unexpected topic {topic_id}"
            ));
        }
        if !seen.insert(topic_id.to_string()) {
            return Err(format!(
                "Batch push response contains duplicate topic {topic_id}"
            ));
        }
        let success = data
            .get("success")
            .and_then(serde_json::Value::as_bool)
            .ok_or_else(|| format!("Batch push result for {topic_id} requires boolean success"))?;
        let error = data
            .get("error")
            .and_then(serde_json::Value::as_str)
            .filter(|message| !message.is_empty())
            .map(str::to_string);
        if !success && error.is_none() {
            return Err(format!(
                "Failed batch push result for {topic_id} requires an error message"
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
                topic_id: topic_id.to_string(),
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
    body: String,
    expected_topic_ids: &[String],
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
        return Err(format!(
            "Batch push messages failed: HTTP {status} body={}",
            String::from_utf8_lossy(&bytes)
        ));
    }

    parse_message_push_frames(&bytes, expected_topic_ids)
}

fn record_message_frames(
    frames: Vec<MessagePushFrame>,
    results: &mut Vec<PushBatchResult>,
    attachment_topics: &mut HashMap<String, HashSet<String>>,
) {
    for frame in frames {
        if frame.outcome.success {
            for hash in frame.needed_attachment_hashes {
                attachment_topics
                    .entry(hash)
                    .or_default()
                    .insert(frame.outcome.topic_id.clone());
            }
        }
        results.push(frame.outcome);
    }
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

        let mut expected_ids = HashSet::new();
        for item in &items {
            let id = item
                .get("id")
                .and_then(serde_json::Value::as_str)
                .filter(|id| !id.is_empty())
                .ok_or_else(|| "Batch push entity item requires a non-empty id".to_string())?;
            if !expected_ids.insert(id.to_string()) {
                return Err(format!(
                    "Batch push entity request contains duplicate id {id}"
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
        let mut seen_ids = HashSet::new();
        for result in results {
            let id = result
                .get("id")
                .and_then(serde_json::Value::as_str)
                .filter(|id| !id.is_empty())
                .ok_or_else(|| "Batch push entity result requires a non-empty id".to_string())?;
            if !expected_ids.contains(id) {
                return Err(format!("Batch push entities returned unexpected id {id}"));
            }
            if !seen_ids.insert(id.to_string()) {
                return Err(format!("Batch push entities returned duplicate id {id}"));
            }
            if result.get("success").and_then(serde_json::Value::as_bool) != Some(true) {
                let error = result
                    .get("error")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("unknown error");
                return Err(format!("Batch push entity {id} failed: {error}"));
            }
        }
        if seen_ids != expected_ids {
            let mut missing = expected_ids
                .difference(&seen_ids)
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
        let db = app.state::<DbState>();

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
        let image_data: Vec<u8> = r.get("image_data");
        let mime_type: String = r.get("mime_type");

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
        topic_ids: &[String],
        uploaded_hashes: Arc<RwLock<HashSet<String>>>,
    ) -> Result<Vec<PushBatchResult>, String> {
        if topic_ids.is_empty() {
            return Ok(Vec::new());
        }
        if topic_ids.len() > MAX_SYNC_TOPICS {
            return Err(format!(
                "Message push contains {} topics, limit is {}",
                topic_ids.len(),
                MAX_SYNC_TOPICS
            ));
        }
        let requested_topics = topic_ids.iter().cloned().collect::<HashSet<_>>();
        if requested_topics.len() != topic_ids.len()
            || requested_topics.iter().any(|topic_id| topic_id.is_empty())
        {
            return Err("Message push topic ids must be unique and non-empty".to_string());
        }

        let db = app.state::<DbState>();
        let mut results = Vec::new();
        let mut attachment_topics: HashMap<String, HashSet<String>> = HashMap::new();
        let mut total_request_bytes = 0usize;
        let mut total_messages = 0usize;
        let mut request_body = String::new();
        let mut request_topics = Vec::new();

        // Query and serialize in bounded topic groups so neither SQLite bind variables nor the
        // HTTP client need to buffer the entire sync set at once.
        for topic_chunk in topic_ids.chunks(1) {
            let placeholders = topic_chunk
                .iter()
                .map(|_| "?")
                .collect::<Vec<_>>()
                .join(", ");
            let topic_query = format!(
                "SELECT topic_id, owner_id, owner_type FROM topics WHERE topic_id IN ({})",
                placeholders
            );
            let mut query = sqlx::query(&topic_query);
            for topic_id in topic_chunk {
                query = query.bind(topic_id);
            }
            let topic_rows = query
                .fetch_all(&db.pool)
                .await
                .map_err(|error| format!("Message push topic query failed: {error}"))?;
            let mut owner_map = HashMap::new();
            for row in topic_rows {
                owner_map.insert(
                    row.get::<String, _>("topic_id"),
                    (
                        row.get::<String, _>("owner_id"),
                        row.get::<String, _>("owner_type"),
                    ),
                );
            }
            for topic_id in topic_chunk {
                if !owner_map.contains_key(topic_id) {
                    return Err(format!("Message push topic {topic_id} is missing locally"));
                }
            }

            // Enforce the attempt-wide message budget before materializing message bodies and
            // attachment relations for this chunk. A single topic can otherwise make the
            // bounded topic query allocate far beyond the protocol's 100k-message ceiling.
            let count_query = format!(
                "SELECT COUNT(*) AS message_count FROM messages WHERE deleted_at IS NULL AND topic_id IN ({})",
                placeholders
            );
            let mut query = sqlx::query(&count_query);
            for topic_id in topic_chunk {
                query = query.bind(topic_id);
            }
            let chunk_message_count = query
                .fetch_one(&db.pool)
                .await
                .map_err(|error| format!("Message push count query failed: {error}"))?
                .get::<i64, _>("message_count");
            let chunk_message_count = usize::try_from(chunk_message_count)
                .map_err(|_| "Message push count is outside the supported range".to_string())?;
            total_messages = total_messages
                .checked_add(chunk_message_count)
                .ok_or_else(|| "Message push count overflow".to_string())?;
            if total_messages > MAX_SYNC_MESSAGES {
                return Err(format!(
                    "Message push contains more than {MAX_SYNC_MESSAGES} messages"
                ));
            }

            let messages_by_topic = crate::vcp_modules::message_service::load_multi_topic_messages(
                &db.pool,
                topic_chunk,
            )
            .await?;
            for topic_id in topic_chunk {
                let history = messages_by_topic.get(topic_id).cloned().unwrap_or_default();
                let owner_type = &owner_map
                    .get(topic_id)
                    .ok_or_else(|| format!("Message push topic {topic_id} has no owner"))?
                    .1;
                let dto_messages = build_message_dtos(app, &history, owner_type).await?;
                let mut line = serde_json::to_string(&serde_json::json!({
                    "topicId": topic_id,
                    "messages": dto_messages,
                }))
                .map_err(|error| format!("Message push serialization failed: {error}"))?;
                line.push('\n');
                if line.len() > MAX_NDJSON_LINE_BYTES {
                    return Err(format!(
                        "Message push topic {topic_id} exceeds the 32 MiB line limit"
                    ));
                }
                total_request_bytes = total_request_bytes
                    .checked_add(line.len())
                    .ok_or_else(|| "Message push byte count overflow".to_string())?;
                if total_request_bytes > MAX_SYNC_BODY_BYTES {
                    return Err("Message push exceeds the 256 MiB total limit".to_string());
                }

                if !request_body.is_empty()
                    && request_body.len() + line.len() > MESSAGE_REQUEST_CHUNK_BYTES
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
                    request_topics.clear();
                }
                request_body.push_str(&line);
                request_topics.push(topic_id.clone());
            }
        }

        if !request_body.is_empty() {
            let frames =
                send_message_chunk(client, http_url, sync_token, request_body, &request_topics)
                    .await?;
            record_message_frames(frames, &mut results, &mut attachment_topics);
        }

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
            let mut result_indexes = HashMap::new();
            for (index, result) in results.iter().enumerate() {
                result_indexes.insert(result.topic_id.clone(), index);
            }
            for (hash, error) in attachment_failures {
                if let Some(topics) = attachment_topics.get(&hash) {
                    for topic_id in topics {
                        if let Some(index) = result_indexes.get(topic_id).copied() {
                            results[index].success = false;
                            results[index].error = Some(format!(
                                "Attachment {hash} required by topic {topic_id} failed: {error}"
                            ));
                        }
                    }
                }
            }
        }

        let ok_count = results.iter().filter(|r| r.success).count();
        log::info!(
            "[PushExecutor] Batch push completed: {}/{} topics",
            ok_count,
            topic_ids.len()
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

async fn build_message_dtos<R: Runtime>(
    app: &AppHandle<R>,
    history: &[crate::vcp_modules::chat_manager::ChatMessage],
    owner_type: &str,
) -> Result<Vec<serde_json::Value>, String> {
    let db = app.state::<DbState>();
    let mut results = Vec::new();

    for msg in history {
        if msg.id.is_empty() || msg.role.is_empty() {
            return Err("Outbound messages require non-empty id and role".to_string());
        }
        if msg.timestamp > i64::MAX as u64 {
            return Err(format!(
                "Outbound message {} timestamp exceeds the supported range",
                msg.id
            ));
        }
        let msg_value = if msg.role == "user" {
            let mut dto = UserMessageSyncDTO::from(msg);
            if let Some(attachments) = dto.attachments.as_mut() {
                for attachment in attachments {
                    attachment.hash = canonical_sha256(&attachment.hash).ok_or_else(|| {
                        format!(
                            "Outbound message {} attachment {} has no valid SHA-256 hash",
                            msg.id, attachment.name
                        )
                    })?;
                }
            }
            serde_json::to_value(dto).map_err(|error| error.to_string())?
        } else if owner_type == "group" {
            let avatar_color =
                query_avatar_color(&db.pool, &msg.agent_id.clone().unwrap_or_default())
                    .await?
                    .unwrap_or_else(|| "#6B7280".to_string());
            let dto = GroupMessageSyncDTO::from_message(msg, avatar_color);
            serde_json::to_value(dto).map_err(|error| error.to_string())?
        } else {
            let avatar_color =
                query_avatar_color(&db.pool, &msg.agent_id.clone().unwrap_or_default())
                    .await?
                    .unwrap_or_else(|| "#6B7280".to_string());
            let dto = AgentMessageSyncDTO::from_message(msg, avatar_color);
            serde_json::to_value(dto).map_err(|error| error.to_string())?
        };
        results.push(msg_value);
    }

    Ok(results)
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
    let mime_type: String = att_row.get("mime_type");
    let internal_path: String = att_row.get("internal_path");
    if internal_path.trim().is_empty() {
        return Err(format!("Attachment {hash} has no local file path"));
    }

    let name_row = sqlx::query("SELECT display_name FROM message_attachments WHERE hash = ? AND deleted_at IS NULL LIMIT 1")
        .bind(&hash)
        .fetch_optional(&db.pool)
        .await
        .map_err(|error| format!("Attachment {hash} display name query failed: {error}"))?;
    let display_name = name_row
        .map(|row| row.get::<String, _>("display_name"))
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "unnamed".to_string());

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
}

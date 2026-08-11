use crate::vcp_modules::db_write_queue::{DbWriteQueue, DbWriteTask};
use crate::vcp_modules::message_repository::MessageRenderCompiler;
use crate::vcp_modules::sync_dto::{
    AgentSyncDTO, AgentTopicSyncDTO, GroupSyncDTO, GroupTopicSyncDTO,
};
use crate::vcp_modules::sync_hash::HashAggregator;
use futures_util::StreamExt;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tauri::{AppHandle, Manager, Runtime};
use tokio::sync::{mpsc, Semaphore};
use tokio::task::JoinSet;

const MAX_NDJSON_LINE_BYTES: usize = 32 * 1024 * 1024;
const MAX_NDJSON_TOTAL_BYTES: usize = 256 * 1024 * 1024;
const MAX_NDJSON_ENTITIES: usize = 100_000;
const MAX_WARNING_SAMPLES: usize = 8;
const NDJSON_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
const PULL_WORKER_BUDGET_UNIT_BYTES: usize = 1024 * 1024;
const PULL_WORKER_BUDGET_UNITS: usize = MAX_NDJSON_LINE_BYTES / PULL_WORKER_BUDGET_UNIT_BYTES;
const MAX_DIRECT_ENTITY_RESPONSE_BYTES: usize = 10 * 1024 * 1024;
const MAX_ENTITY_BATCH_ITEMS: usize = 10_000;
const MAX_AVATAR_RESPONSE_BYTES: usize = 20 * 1024 * 1024;
const MAX_ERROR_RESPONSE_BYTES: usize = 1024 * 1024;

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

struct NdjsonBudget {
    max_frames: usize,
    total_bytes: usize,
    frames: usize,
    entities: usize,
}

impl NdjsonBudget {
    fn new(max_frames: usize) -> Self {
        Self {
            max_frames,
            total_bytes: 0,
            frames: 0,
            entities: 0,
        }
    }

    fn observe_chunk(&mut self, bytes: usize) -> Result<(), String> {
        self.total_bytes = self
            .total_bytes
            .checked_add(bytes)
            .ok_or_else(|| "NDJSON response size overflow".to_string())?;
        if self.total_bytes > MAX_NDJSON_TOTAL_BYTES {
            return Err("NDJSON response exceeds 256MB budget".to_string());
        }
        Ok(())
    }

    fn observe_frame(&mut self, line_bytes: usize, entities: usize) -> Result<(), String> {
        if line_bytes > MAX_NDJSON_LINE_BYTES {
            return Err("NDJSON frame exceeds 32MB budget".to_string());
        }
        self.frames += 1;
        if self.frames > self.max_frames {
            return Err("NDJSON response contains more frames than requested topics".to_string());
        }
        self.entities = self
            .entities
            .checked_add(entities)
            .ok_or_else(|| "NDJSON entity count overflow".to_string())?;
        if self.entities > MAX_NDJSON_ENTITIES {
            return Err("NDJSON response exceeds 100000 message budget".to_string());
        }
        Ok(())
    }
}

struct TopicNDJSONFrame {
    topic_id: String,
    messages: Vec<crate::vcp_modules::sync_dto::MessagePullSyncDTO>,
    error: Option<String>,
    legacy_attachment_warnings: usize,
    warning_samples: Vec<String>,
}

#[derive(Default)]
struct BoundedWarnings {
    count: usize,
    samples: Vec<String>,
}

impl BoundedWarnings {
    fn push(&mut self, message: String) {
        self.count += 1;
        if self.samples.len() < MAX_WARNING_SAMPLES {
            self.samples.push(message);
        }
    }
}

enum HashField {
    Missing,
    Valid(String),
    Invalid,
}

fn read_hash_field(object: &serde_json::Map<String, Value>, key: &str) -> HashField {
    match object.get(key) {
        None | Some(Value::Null) => HashField::Missing,
        Some(Value::String(hash)) => {
            let normalized = hash.to_ascii_lowercase();
            if crate::vcp_modules::infra::utils::is_valid_cas_hash(&normalized) {
                HashField::Valid(normalized)
            } else {
                HashField::Invalid
            }
        }
        Some(_) => HashField::Invalid,
    }
}

fn normalize_timestamp(value: Option<&Value>, message_id: &str) -> Result<u64, String> {
    let timestamp = match value {
        Some(Value::Number(number)) => number.as_u64().ok_or_else(|| {
            format!("Message {message_id} timestamp must be a non-negative integer")
        }),
        Some(Value::String(timestamp)) => timestamp.parse::<u64>().map_err(|_| {
            format!("Message {message_id} timestamp string must be a non-negative integer")
        }),
        _ => Err(format!(
            "Message {message_id} timestamp must be a non-negative integer or integer string"
        )),
    }?;
    if timestamp > i64::MAX as u64 {
        return Err(format!(
            "Message {message_id} timestamp exceeds the SQLite integer range"
        ));
    }
    Ok(timestamp)
}

fn canonicalize_attachment(
    value: Value,
    message_id: &str,
    attachment_index: usize,
    warnings: &mut BoundedWarnings,
) -> Result<Option<Value>, String> {
    let mut object = value.as_object().cloned().ok_or_else(|| {
        format!("Message {message_id} attachment {attachment_index} must be an object")
    })?;

    let nested = match object.get("_fileManagerData") {
        None | Some(Value::Null) => None,
        Some(Value::Object(nested)) => Some(nested.clone()),
        Some(_) => {
            warnings.push(format!(
                "message={message_id} attachment={attachment_index}: invalid _fileManagerData"
            ));
            return Ok(None);
        }
    };
    let top_hash = read_hash_field(&object, "hash");
    let nested_hash = nested
        .as_ref()
        .map(|nested| read_hash_field(nested, "hash"))
        .unwrap_or(HashField::Missing);

    let normalized_hash = match (top_hash, nested_hash) {
        (HashField::Valid(top), HashField::Valid(nested)) if top == nested => Some(top),
        (HashField::Valid(_), HashField::Valid(_)) => None,
        (HashField::Valid(hash), HashField::Missing | HashField::Invalid)
        | (HashField::Missing | HashField::Invalid, HashField::Valid(hash)) => Some(hash),
        (HashField::Missing | HashField::Invalid, HashField::Missing | HashField::Invalid) => None,
    };
    let Some(hash) = normalized_hash else {
        warnings.push(format!(
            "message={message_id} attachment={attachment_index}: missing, invalid, or conflicting SHA-256"
        ));
        return Ok(None);
    };

    if let Some(nested) = nested {
        for (nested_key, public_key) in [
            ("extractedText", "extractedText"),
            ("imageFrames", "imageFrames"),
            ("createdAt", "createdAt"),
        ] {
            if !object.contains_key(public_key) {
                if let Some(value) = nested.get(nested_key) {
                    object.insert(public_key.to_string(), value.clone());
                }
            }
        }
    }

    object.insert("hash".to_string(), Value::String(hash));
    object.remove("_fileManagerData");
    object.remove("src");
    object.remove("resolvedSrc");
    object.remove("internalPath");
    object.remove("thumbnailPath");
    object.remove("path");
    object.remove("filePath");
    object.remove("status");
    Ok(Some(Value::Object(object)))
}

fn parse_topic_ndjson_frame(bytes: &[u8]) -> Result<TopicNDJSONFrame, String> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|error| format!("Malformed NDJSON frame: {error}"))?;
    let object = value
        .as_object()
        .ok_or_else(|| "NDJSON frame must be an object".to_string())?;
    let topic_id = object
        .get("topicId")
        .and_then(Value::as_str)
        .filter(|topic_id| !topic_id.is_empty())
        .ok_or_else(|| "NDJSON frame contains missing or empty topicId".to_string())?
        .to_string();
    let error = match object.get("_error") {
        None | Some(Value::Null) => None,
        Some(Value::String(error)) if !error.is_empty() => Some(error.clone()),
        Some(_) => {
            return Err(format!(
                "NDJSON error frame for {topic_id} has invalid _error"
            ))
        }
    };
    if error.is_some() {
        return Ok(TopicNDJSONFrame {
            topic_id,
            messages: Vec::new(),
            error,
            legacy_attachment_warnings: 0,
            warning_samples: Vec::new(),
        });
    }

    let raw_messages = object
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("NDJSON frame for {topic_id} requires messages array"))?;
    let mut warnings = BoundedWarnings::default();
    let mut seen_message_ids = HashSet::new();
    let mut messages = Vec::with_capacity(raw_messages.len());
    for raw_message in raw_messages {
        let mut message = raw_message
            .as_object()
            .cloned()
            .ok_or_else(|| format!("Topic {topic_id} contains a non-object message"))?;
        let message_id = message
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| format!("Topic {topic_id} contains a message with missing or empty id"))?
            .to_string();
        if !seen_message_ids.insert(message_id.clone()) {
            return Err(format!(
                "Topic {topic_id} contains duplicate message {message_id}"
            ));
        }
        message
            .get("role")
            .and_then(Value::as_str)
            .filter(|role| !role.is_empty())
            .ok_or_else(|| format!("Message {message_id} has missing or empty role"))?;
        match message.get("topicId") {
            None | Some(Value::Null) => {}
            Some(Value::String(message_topic)) if message_topic == &topic_id => {}
            Some(Value::String(message_topic)) => {
                return Err(format!(
                    "Message {message_id} topicId {message_topic} conflicts with frame topic {topic_id}"
                ));
            }
            Some(_) => {
                return Err(format!("Message {message_id} topicId must be a string"));
            }
        }
        if message.get("status").and_then(Value::as_str) == Some("removed")
            || message
                .get("deletedAt")
                .is_some_and(|value| !value.is_null())
        {
            return Err(format!(
                "Tombstoned message {message_id} must not appear in a live pull frame"
            ));
        }
        let timestamp = normalize_timestamp(message.get("timestamp"), &message_id)?;
        message.insert("timestamp".to_string(), Value::from(timestamp));
        message.remove("contentHash");
        message.remove("content_hash");

        match message.remove("attachments") {
            None | Some(Value::Null) => {}
            Some(Value::Array(attachments)) => {
                let mut canonical = Vec::with_capacity(attachments.len());
                for (index, attachment) in attachments.into_iter().enumerate() {
                    if let Some(attachment) =
                        canonicalize_attachment(attachment, &message_id, index, &mut warnings)?
                    {
                        canonical.push(attachment);
                    }
                }
                if !canonical.is_empty() {
                    message.insert("attachments".to_string(), Value::Array(canonical));
                }
            }
            Some(_) => {
                return Err(format!(
                    "Message {message_id} attachments must be an array or null"
                ));
            }
        }

        let dto = serde_json::from_value(Value::Object(message))
            .map_err(|error| format!("Message {message_id} violates protocol 1.1: {error}"))?;
        messages.push(dto);
    }

    Ok(TopicNDJSONFrame {
        topic_id,
        messages,
        error: None,
        legacy_attachment_warnings: warnings.count,
        warning_samples: warnings.samples,
    })
}

fn validate_requested_message_ids(
    topic_id: &str,
    expected: Option<&HashSet<String>>,
    messages: &[crate::vcp_modules::sync_dto::MessagePullSyncDTO],
) -> Result<(), String> {
    let Some(expected) = expected else {
        return Ok(());
    };
    let actual = messages
        .iter()
        .map(|message| message.id.clone())
        .collect::<HashSet<_>>();
    if actual == *expected {
        return Ok(());
    }
    let mut missing = expected.difference(&actual).cloned().collect::<Vec<_>>();
    let mut unexpected = actual.difference(expected).cloned().collect::<Vec<_>>();
    missing.sort();
    unexpected.sort();
    Err(format!(
        "NDJSON message set mismatch for {topic_id}: missing={missing:?}, unexpected={unexpected:?}"
    ))
}

fn pull_worker_permits(frame_bytes: usize) -> Result<u32, String> {
    let units = frame_bytes.saturating_add(PULL_WORKER_BUDGET_UNIT_BYTES - 1)
        / PULL_WORKER_BUDGET_UNIT_BYTES;
    u32::try_from(units.max(1)).map_err(|_| "Pull worker permit count overflow".to_string())
}

/// 共享消息处理管线：附件路径批量查询 → 填充 → 预渲染并文本压缩(通过Rayon并行化) → 写入队列
/// 被 `pull_messages_batch` 内各并发任务复用。
/// 返回 `(parsed_count, failed_count)`。
async fn process_topic_messages<R: Runtime>(
    app: &AppHandle<R>,
    topic_id: &str,
    mut parsed_messages: Vec<crate::vcp_modules::chat_manager::ChatMessage>,
    write_queue: &DbWriteQueue,
    prerender_enabled: bool,
) -> Result<(usize, usize), String> {
    let t_start = std::time::Instant::now();
    use crate::vcp_modules::db_manager::DbState;
    use sqlx::Row;
    let db = app.state::<DbState>();

    // 1. 批量收集所有附件 hash，一次性查询本地路径（替代 N+1 查询）
    let t_att_start = std::time::Instant::now();
    let mut all_hashes = HashSet::new();
    for msg in &parsed_messages {
        if let Some(ref atts) = msg.attachments {
            for att in atts {
                if let Some(ref hash) = att.hash {
                    if !hash.is_empty() {
                        all_hashes.insert(hash.to_string());
                    }
                }
            }
        }
    }

    let mut path_map = std::collections::HashMap::new();
    if !all_hashes.is_empty() {
        let all_hashes = all_hashes.into_iter().collect::<Vec<_>>();
        for hash_chunk in all_hashes.chunks(500) {
            let placeholders = hash_chunk
                .iter()
                .map(|_| "?")
                .collect::<Vec<_>>()
                .join(", ");
            let query = format!(
                "SELECT hash, internal_path FROM attachments WHERE hash IN ({})",
                placeholders
            );
            let mut query = sqlx::query(&query);
            for hash in hash_chunk {
                query = query.bind(hash);
            }
            let rows = query.fetch_all(&db.pool).await.map_err(|error| {
                format!("Failed to resolve local attachment CAS paths: {error}")
            })?;
            for row in rows {
                let hash = row
                    .try_get::<String, _>("hash")
                    .map_err(|error| format!("Failed to decode attachment hash: {error}"))?;
                let path = row
                    .try_get::<String, _>("internal_path")
                    .map_err(|error| format!("Failed to decode attachment path: {error}"))?;
                let clean_path = path.trim_start_matches("file://");
                if !clean_path.is_empty()
                    && tokio::fs::metadata(clean_path)
                        .await
                        .is_ok_and(|metadata| metadata.is_file())
                {
                    path_map.insert(hash, clean_path.to_string());
                }
            }
        }
    }
    let t_att = t_att_start.elapsed();

    // 2. 用缓存的 path_map 填充附件路径与状态
    for msg in &mut parsed_messages {
        if let Some(ref mut atts) = msg.attachments {
            for att in atts {
                if let Some(ref hash) = att.hash {
                    if !hash.is_empty() {
                        if let Some(path) = path_map.get(hash) {
                            att.internal_path = path.clone();
                            att.src = format!("file://{}", path);
                            att.status = Some("ready".to_string());
                        } else {
                            att.internal_path.clear();
                            att.src.clear();
                            att.status = Some("desktop_only".to_string());
                        }
                    }
                }
            }
        }
    }

    let parsed_count = parsed_messages.len();
    let mut t_block = std::time::Duration::from_secs(0);
    let mut t_submit = std::time::Duration::from_secs(0);

    if !parsed_messages.is_empty() {
        // 3. 将预渲染和 Zstd 压缩等 CPU 密集型任务完美剥离至 spawn_blocking 线程池，解除 Tokio Worker 线程阻塞
        let t_block_start = std::time::Instant::now();
        let topic_id_clone = topic_id.to_string();
        let (parsed_messages_back, content_hashes, render_bytes_list, contents) =
            tokio::task::spawn_blocking(move || {
                let count = parsed_messages.len();
                let mut content_hashes = Vec::with_capacity(count);
                let mut render_bytes_list = Vec::with_capacity(count);
                let mut contents = Vec::with_capacity(count);

                for msg in &parsed_messages {
                    // A. 计算/直读指纹
                    let attachment_hashes: Vec<String> = msg
                        .attachments
                        .as_ref()
                        .map(|atts| {
                            atts.iter()
                                .map(|a| a.hash.clone().unwrap_or_default())
                                .filter(|h| !h.is_empty())
                                .collect()
                        })
                        .unwrap_or_default();

                    // contentHash 只依据最终规范化消息计算，禁止复用桌面端在内部字段
                    // 尚未剥离时生成的旧指纹。
                    let content_hash = HashAggregator::compute_message_fingerprint(
                        &msg.content,
                        &attachment_hashes,
                    );

                    // B. 预渲染（按开关控制）
                    let content = &msg.content;
                    let topic_id_log = topic_id_clone.clone();
                    let msg_id_log = msg.id.clone();

                    let rb = if prerender_enabled {
                        let comp_res =
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                let blocks = MessageRenderCompiler::compile(content);
                                MessageRenderCompiler::serialize(&blocks).unwrap_or_default()
                            }));
                        match comp_res {
                            Ok(val) => val,
                            Err(_) => {
                                log::warn!(
                                    "[PullExecutor] Compile panicked for msg {} (topic {})",
                                    msg_id_log,
                                    topic_id_log
                                );
                                Vec::new()
                            }
                        }
                    } else {
                        Vec::new()
                    };

                    content_hashes.push(content_hash);
                    render_bytes_list.push(rb);
                    contents.push(content.clone());
                }

                (parsed_messages, content_hashes, render_bytes_list, contents)
            })
            .await
            .map_err(|e| format!("Spawn blocking failed: {}", e))?;
        t_block = t_block_start.elapsed();

        // 4. 提交落盘
        let t_submit_start = std::time::Instant::now();
        // 限制单个事务的消息规模；队列仍会合并相邻小任务，但总量上限为 500。
        const WRITE_CHUNK_MESSAGES: usize = 250;
        let mut messages = parsed_messages_back.into_iter();
        let mut contents = contents.into_iter();
        let mut render_bytes = render_bytes_list.into_iter();
        let mut hashes = content_hashes.into_iter();
        loop {
            let message_chunk: Vec<_> = messages.by_ref().take(WRITE_CHUNK_MESSAGES).collect();
            if message_chunk.is_empty() {
                break;
            }
            let chunk_len = message_chunk.len();
            write_queue
                .submit(DbWriteTask::TopicMessages {
                    topic_id: topic_id.to_string(),
                    messages: message_chunk,
                    contents: contents.by_ref().take(chunk_len).collect(),
                    render_bytes: render_bytes.by_ref().take(chunk_len).collect(),
                    content_hashes: hashes.by_ref().take(chunk_len).collect(),
                    skip_bubble: true,
                })
                .await?;
        }
        t_submit = t_submit_start.elapsed();
    }

    let t_total = t_start.elapsed();
    if parsed_count > 0 {
        log::debug!(
            "[PullExecutor] [ProfileDetail] topic={} msgs={} | sql_att={:?} spawn_blocking={:?} submit_queue={:?} | total_proc={:?}",
            topic_id, parsed_count, t_att, t_block, t_submit, t_total
        );
    }

    Ok((parsed_count, 0))
}

/// 批量 Pull 单 topic 处理结果
#[allow(dead_code)]
pub struct BatchPullResult {
    pub topic_id: String,
    pub success: bool,
    pub parsed_count: usize,
    pub failed_count: usize,
    pub legacy_attachment_warnings: usize,
    pub error: Option<String>,
}

pub struct PullExecutor;

impl PullExecutor {
    pub async fn pull_agent<R: Runtime>(
        _app: &AppHandle<R>,
        client: &reqwest::Client,
        http_url: &str,
        sync_token: &str,
        agent_id: &str,
        write_queue: &DbWriteQueue,
    ) -> Result<(), String> {
        let url = format!(
            "{}/api/mobile-sync/download-entity?id={}&type=agent",
            http_url, agent_id
        );
        let res = client
            .get(&url)
            .header("x-sync-token", sync_token)
            .header("Authorization", format!("Bearer {}", sync_token))
            .send()
            .await
            .map_err(|e| e.to_string())?;

        let (status, bytes) =
            read_response_limited(res, MAX_DIRECT_ENTITY_RESPONSE_BYTES, "Pull agent").await?;
        if !status.is_success() {
            return Err(format!(
                "Pull agent failed: {status} body={}",
                String::from_utf8_lossy(&bytes)
            ));
        }
        let dto: AgentSyncDTO = serde_json::from_slice(&bytes)
            .map_err(|error| format!("Pull agent returned invalid JSON: {error}"))?;
        write_queue
            .submit(DbWriteTask::Agent {
                id: agent_id.to_string(),
                dto,
            })
            .await?;

        Ok(())
    }

    pub async fn pull_group<R: Runtime>(
        _app: &AppHandle<R>,
        client: &reqwest::Client,
        http_url: &str,
        sync_token: &str,
        group_id: &str,
        write_queue: &DbWriteQueue,
    ) -> Result<(), String> {
        let url = format!(
            "{}/api/mobile-sync/download-entity?id={}&type=group",
            http_url, group_id
        );
        let res = client
            .get(&url)
            .header("x-sync-token", sync_token)
            .header("Authorization", format!("Bearer {}", sync_token))
            .send()
            .await
            .map_err(|e| e.to_string())?;

        let (status, bytes) =
            read_response_limited(res, MAX_DIRECT_ENTITY_RESPONSE_BYTES, "Pull group").await?;
        if !status.is_success() {
            return Err(format!(
                "Pull group failed: {status} body={}",
                String::from_utf8_lossy(&bytes)
            ));
        }
        let dto: GroupSyncDTO = serde_json::from_slice(&bytes)
            .map_err(|error| format!("Pull group returned invalid JSON: {error}"))?;
        write_queue
            .submit(DbWriteTask::Group {
                id: group_id.to_string(),
                dto,
            })
            .await?;

        Ok(())
    }

    pub async fn pull_entities_batch<R: Runtime>(
        app: &AppHandle<R>,
        client: &reqwest::Client,
        http_url: &str,
        sync_token: &str,
        requests: Vec<serde_json::Value>,
        write_queue: &DbWriteQueue,
    ) -> Result<(), String> {
        if requests.len() > MAX_ENTITY_BATCH_ITEMS {
            return Err(format!(
                "Entity pull request contains more than {MAX_ENTITY_BATCH_ITEMS} items"
            ));
        }
        let mut expected = HashSet::new();
        for request in &requests {
            let id = request
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .ok_or_else(|| "Entity pull request requires a non-empty id".to_string())?;
            let entity_type = request
                .get("type")
                .and_then(Value::as_str)
                .filter(|value| matches!(*value, "agent" | "group" | "agent_topic" | "group_topic"))
                .ok_or_else(|| format!("Entity pull request {id} has an invalid type"))?;
            if !expected.insert((id.to_string(), entity_type.to_string())) {
                return Err(format!(
                    "Entity pull request contains duplicate {entity_type}/{id}"
                ));
            }
        }
        let request_body = serde_json::to_vec(&serde_json::json!({ "requests": requests }))
            .map_err(|error| format!("Entity pull request serialization failed: {error}"))?;
        if request_body.len() > MAX_DIRECT_ENTITY_RESPONSE_BYTES {
            return Err("Entity pull request exceeds 10 MiB".to_string());
        }
        let url = format!("{}/api/mobile-sync/download-entities", http_url);
        let res = client
            .post(&url)
            .header("x-sync-token", sync_token)
            .header("Authorization", format!("Bearer {}", sync_token))
            .header("Content-Type", "application/json")
            .body(request_body)
            .send()
            .await
            .map_err(|e| e.to_string())?;

        let (status, bytes) =
            read_response_limited(res, MAX_DIRECT_ENTITY_RESPONSE_BYTES, "Entity pull").await?;
        if !status.is_success() {
            return Err(format!(
                "Pull entities batch failed: {status} body={}",
                String::from_utf8_lossy(&bytes)
            ));
        }
        let results: Vec<serde_json::Value> =
            serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        log::info!(
            "[PullExecutor] Received {} entities from server",
            results.len()
        );

        let mut agent_topics = Vec::new();
        let mut group_topics = Vec::new();
        let mut seen = HashSet::new();

        for item in results {
            let id = item
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .ok_or_else(|| "Entity pull result requires a non-empty id".to_string())?;
            let r#type = item
                .get("type")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("Entity pull result {id} requires type"))?;
            let key = (id.to_string(), r#type.to_string());
            if !expected.contains(&key) {
                return Err(format!(
                    "Entity pull returned unexpected result {type_name}/{id}",
                    type_name = r#type
                ));
            }
            if !seen.insert(key) {
                return Err(format!(
                    "Entity pull returned duplicate result {type_name}/{id}",
                    type_name = r#type
                ));
            }
            if item.get("success").and_then(Value::as_bool) != Some(true) {
                let error = item
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown error");
                return Err(format!(
                    "Entity pull {type_name}/{id} failed: {error}",
                    type_name = r#type
                ));
            }
            let data = item.get("data").cloned().ok_or_else(|| {
                format!(
                    "Entity pull result {type_name}/{id} requires data",
                    type_name = r#type
                )
            })?;

            match r#type {
                "agent" => {
                    let dto = serde_json::from_value::<AgentSyncDTO>(data)
                        .map_err(|error| format!("Invalid agent {id}: {error}"))?;
                    write_queue
                        .submit(DbWriteTask::Agent {
                            id: id.to_string(),
                            dto,
                        })
                        .await?;
                }
                "group" => {
                    let dto = serde_json::from_value::<GroupSyncDTO>(data)
                        .map_err(|error| format!("Invalid group {id}: {error}"))?;
                    write_queue
                        .submit(DbWriteTask::Group {
                            id: id.to_string(),
                            dto,
                        })
                        .await?;
                }
                "agent_topic" => {
                    if id == "default" {
                        continue;
                    }
                    let dto = serde_json::from_value::<AgentTopicSyncDTO>(data)
                        .map_err(|error| format!("Invalid agent topic {id}: {error}"))?;
                    agent_topics.push((id.to_string(), dto));
                }
                "group_topic" => {
                    if id == "default" {
                        continue;
                    }
                    let dto = serde_json::from_value::<GroupTopicSyncDTO>(data)
                        .map_err(|error| format!("Invalid group topic {id}: {error}"))?;
                    group_topics.push((id.to_string(), dto));
                }
                _ => return Err(format!("Entity pull returned unsupported type {}", r#type)),
            }
        }

        if seen != expected {
            let mut missing = expected.difference(&seen).cloned().collect::<Vec<_>>();
            missing.sort();
            return Err(format!(
                "Entity pull response is missing results {missing:?}"
            ));
        }

        if !agent_topics.is_empty() {
            log::debug!(
                "[PullExecutor] Submitting {} agent topics to write queue",
                agent_topics.len()
            );
            write_queue
                .submit(DbWriteTask::AgentTopicBatch {
                    topics: agent_topics,
                })
                .await?;
        }
        if !group_topics.is_empty() {
            log::debug!(
                "[PullExecutor] Submitting {} group topics to write queue",
                group_topics.len()
            );
            write_queue
                .submit(DbWriteTask::GroupTopicBatch {
                    topics: group_topics,
                })
                .await?;
        }

        crate::vcp_modules::sync::sync_service::emit_sync_log(
            app,
            "info",
            "[PullExecutor] Batch pull completed",
        );
        Ok(())
    }

    pub async fn pull_avatar<R: Runtime>(
        _app: &AppHandle<R>,
        client: &reqwest::Client,
        http_url: &str,
        sync_token: &str,
        owner_type: &str,
        owner_id: &str,
        write_queue: &DbWriteQueue,
    ) -> Result<(), String> {
        let url = format!(
            "{}/api/mobile-sync/download-avatar?id={}&type={}",
            http_url, owner_id, owner_type
        );

        // 指数退避重试：avatar 下载受网络波动影响较大
        let mut retries = 0;
        let max_retries = 3;
        let mut delay_ms = 200u64;
        loop {
            match client
                .get(&url)
                .header("x-sync-token", sync_token)
                .header("Authorization", format!("Bearer {}", sync_token))
                .send()
                .await
            {
                Ok(res) => {
                    let status = res.status();
                    if !status.is_success() {
                        return Err(format!("Pull avatar failed: {status}"));
                    }
                    match read_response_limited(res, MAX_AVATAR_RESPONSE_BYTES, "Pull avatar").await
                    {
                        Ok((_, bytes)) => {
                            write_queue
                                .submit(DbWriteTask::Avatar {
                                    owner_type: owner_type.to_string(),
                                    owner_id: owner_id.to_string(),
                                    bytes,
                                })
                                .await?;
                            if retries > 0 {
                                log::info!(
                                    "[PullExecutor] Avatar {} {} succeeded after {} retries",
                                    owner_type,
                                    owner_id,
                                    retries
                                );
                            }
                            return Ok(());
                        }
                        Err(e) if retries < max_retries => {
                            retries += 1;
                            log::warn!("[PullExecutor] Avatar {} {} decode failed (retry {}/{}): {}. Waiting {}ms", owner_type, owner_id, retries, max_retries, e, delay_ms);
                            tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                            delay_ms *= 2;
                        }
                        Err(e) => {
                            return Err(format!(
                                "Pull avatar decode failed after {} retries: {}",
                                max_retries, e
                            ));
                        }
                    }
                }
                Err(e) if retries < max_retries => {
                    retries += 1;
                    log::warn!("[PullExecutor] Avatar {} {} request failed (retry {}/{}): {}. Waiting {}ms", owner_type, owner_id, retries, max_retries, e, delay_ms);
                    tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                    delay_ms *= 2;
                }
                Err(e) => {
                    return Err(format!(
                        "Pull avatar request failed after {} retries: {}",
                        max_retries, e
                    ));
                }
            }
        }
    }

    pub async fn pull_agent_topic<R: Runtime>(
        _app: &AppHandle<R>,
        client: &reqwest::Client,
        http_url: &str,
        sync_token: &str,
        topic_id: &str,
        write_queue: &DbWriteQueue,
    ) -> Result<(), String> {
        let url = format!(
            "{}/api/mobile-sync/download-entity?id={}&type=agent_topic",
            http_url, topic_id
        );
        let res = client
            .get(&url)
            .header("x-sync-token", sync_token)
            .header("Authorization", format!("Bearer {}", sync_token))
            .send()
            .await
            .map_err(|e| e.to_string())?;

        let (status, bytes) =
            read_response_limited(res, MAX_DIRECT_ENTITY_RESPONSE_BYTES, "Pull agent topic")
                .await?;
        if !status.is_success() {
            return Err(format!(
                "Pull agent_topic failed: {status} body={}",
                String::from_utf8_lossy(&bytes)
            ));
        }
        let dto: AgentTopicSyncDTO = serde_json::from_slice(&bytes)
            .map_err(|error| format!("Pull agent topic returned invalid JSON: {error}"))?;
        write_queue
            .submit(DbWriteTask::AgentTopic {
                topic_id: topic_id.to_string(),
                dto,
            })
            .await?;

        Ok(())
    }

    pub async fn pull_group_topic<R: Runtime>(
        _app: &AppHandle<R>,
        client: &reqwest::Client,
        http_url: &str,
        sync_token: &str,
        topic_id: &str,
        write_queue: &DbWriteQueue,
    ) -> Result<(), String> {
        let url = format!(
            "{}/api/mobile-sync/download-entity?id={}&type=group_topic",
            http_url, topic_id
        );
        let res = client
            .get(&url)
            .header("x-sync-token", sync_token)
            .header("Authorization", format!("Bearer {}", sync_token))
            .send()
            .await
            .map_err(|e| e.to_string())?;

        let (status, bytes) =
            read_response_limited(res, MAX_DIRECT_ENTITY_RESPONSE_BYTES, "Pull group topic")
                .await?;
        if !status.is_success() {
            return Err(format!(
                "Pull group_topic failed: {status} body={}",
                String::from_utf8_lossy(&bytes)
            ));
        }
        let dto: GroupTopicSyncDTO = serde_json::from_slice(&bytes)
            .map_err(|error| format!("Pull group topic returned invalid JSON: {error}"))?;
        write_queue
            .submit(DbWriteTask::GroupTopic {
                topic_id: topic_id.to_string(),
                dto,
            })
            .await?;

        Ok(())
    }

    /// 流式批量 Pull — 一次 HTTP 请求拉取多个 topic 的消息
    ///
    /// 桌面端以 NDJSON 逐 topic 分帧返回，手机端逐行消费，
    /// 不等待整个响应结束。任何 topic 失败都会通过结果传播并终止当前 attempt。
    ///
    /// **并发控制**: 按原始 frame 字节加权的 Semaphore + tokio spawn 处理 topic 消息，
    /// 在途原始帧预算为 32 MiB；有界 mpsc channel 实时推送进度日志并施加背压。
    ///
    /// 返回每个 topic 的处理结果。
    pub async fn pull_messages_batch<R: Runtime>(
        app: &AppHandle<R>,
        client: &reqwest::Client,
        http_url: &str,
        sync_token: &str,
        requests: &[(String, Vec<String>)], // (topic_id, msg_ids), 空 vec = 拉全部消息
        write_queue: &DbWriteQueue,
        prerender_enabled: bool,
    ) -> Result<Vec<BatchPullResult>, String> {
        if requests.is_empty() {
            return Ok(Vec::new());
        }
        let mut expected_message_ids = HashMap::new();
        for (topic_id, message_ids) in requests {
            if topic_id.is_empty() || expected_message_ids.contains_key(topic_id) {
                return Err("Pull request contains empty or duplicate topicId".to_string());
            }
            let exact_messages = if message_ids.is_empty() {
                None
            } else {
                let ids = message_ids.iter().cloned().collect::<HashSet<_>>();
                if ids.len() != message_ids.len() || ids.iter().any(|id| id.is_empty()) {
                    return Err(format!(
                        "Pull request for {topic_id} contains empty or duplicate message id"
                    ));
                }
                Some(ids)
            };
            expected_message_ids.insert(topic_id.clone(), exact_messages);
        }
        let expected_topics = expected_message_ids.keys().cloned().collect::<HashSet<_>>();
        let mut seen_topics = HashSet::new();

        let url = format!("{}/api/mobile-sync/download-messages-stream", http_url);
        let req_body: Vec<serde_json::Value> = requests
            .iter()
            .map(|(tid, ids)| serde_json::json!({ "topicId": tid, "msgIds": ids }))
            .collect();

        let res = client
            .post(&url)
            .header("x-sync-token", sync_token)
            .header("Authorization", format!("Bearer {}", sync_token))
            .json(&serde_json::json!({ "requests": req_body }))
            .send()
            .await
            .map_err(|e| e.to_string())?;

        if !res.status().is_success() {
            let status = res.status();
            let (_, err_body) =
                read_response_limited(res, MAX_ERROR_RESPONSE_BYTES, "Batch pull error").await?;
            return Err(format!(
                "Batch pull messages failed: HTTP {} body={}",
                status,
                String::from_utf8_lossy(&err_body)
            ));
        }

        // ── 并发基础设施 ──
        let sem = Arc::new(Semaphore::new(PULL_WORKER_BUDGET_UNITS));
        let (tx, mut rx) = mpsc::channel::<BatchPullResult>(64);
        let mut spawn_handles = JoinSet::new();
        let total = requests.len();

        // 启动接收协程：实时消费 channel 输出进度日志
        let app_receiver = app.clone();
        let receiver = async move {
            let mut results = Vec::new();
            let mut completed = 0usize;
            while let Some(result) = rx.recv().await {
                completed += 1;
                if result.success {
                    let msg = format!(
                        "[PullExecutor] Batch pull: topic {} completed ({}/{})",
                        result.topic_id, completed, total
                    );
                    crate::vcp_modules::sync::sync_service::emit_sync_log(
                        &app_receiver,
                        "info",
                        &msg,
                    );
                } else {
                    let err = result.error.as_deref().unwrap_or("unknown");
                    let msg = format!(
                        "[PullExecutor] Batch pull: topic {} FAILED ({}/{}): {}",
                        result.topic_id, completed, total, err
                    );
                    crate::vcp_modules::sync::sync_service::emit_sync_log(
                        &app_receiver,
                        "error",
                        &msg,
                    );
                }
                results.push(result);
            }
            results
        };
        let receiver_handle = tokio::spawn(receiver);

        // ── NDJSON 解析协程 ──
        let mut stream = res.bytes_stream();
        let mut buffer: Vec<u8> = Vec::new();
        let mut search_start = 0; // 核心优化：新增扫描游标，避免 O(N^2) 重复扫描
        let mut ndjson_budget = NdjsonBudget::new(requests.len());

        loop {
            let next_chunk = tokio::time::timeout(NDJSON_IDLE_TIMEOUT, stream.next())
                .await
                .map_err(|_| "NDJSON stream idle timeout after 30 seconds".to_string())?;
            let Some(chunk_result) = next_chunk else {
                break;
            };
            let chunk = chunk_result.map_err(|e| format!("Stream read error: {}", e))?;
            ndjson_budget.observe_chunk(chunk.len())?;

            // 检测流级错误帧
            if chunk.starts_with(b"{\"_stream_error\"") || chunk.starts_with(br#"{"_stream_error""#)
            {
                if let Ok(val) = serde_json::from_slice::<serde_json::Value>(&chunk) {
                    let msg = val["_stream_error"]
                        .as_str()
                        .unwrap_or("unknown stream error");
                    return Err(format!("Desktop stream error: {}", msg));
                }
            }

            buffer.extend_from_slice(&chunk);

            // 逐行解析 NDJSON（优化为从游标处开始扫描，实现 O(N) 性能）
            while let Some(pos) = buffer[search_start..].iter().position(|&b| b == b'\n') {
                let line_end = search_start + pos;
                if line_end + 1 > MAX_NDJSON_LINE_BYTES {
                    return Err("NDJSON frame exceeds 32MB budget".to_string());
                }
                let line = buffer.drain(..=line_end).collect::<Vec<_>>();
                search_start = 0; // 成功切分一行后，后续扫描从头开始（因为 buffer 已被 drain）

                if line.len() <= 1 {
                    continue;
                }

                let frame = parse_topic_ndjson_frame(&line)?;
                ndjson_budget.observe_frame(line.len(), frame.messages.len())?;
                let topic_id = frame.topic_id;
                if !expected_topics.contains(&topic_id) {
                    return Err(format!("NDJSON returned unexpected topicId {topic_id}"));
                }
                if !seen_topics.insert(topic_id.clone()) {
                    return Err(format!("NDJSON returned duplicate topicId {topic_id}"));
                }
                for warning in &frame.warning_samples {
                    crate::vcp_modules::sync::sync_service::emit_sync_log(
                        app,
                        "warning",
                        &format!("旧附件已省略: {warning}"),
                    );
                }
                if let Some(topic_err) = frame.error {
                    tx.send(BatchPullResult {
                        topic_id,
                        success: false,
                        parsed_count: 0,
                        failed_count: 0,
                        legacy_attachment_warnings: frame.legacy_attachment_warnings,
                        error: Some(format!("Desktop error: {}", topic_err)),
                    })
                    .await
                    .map_err(|_| "Pull result receiver closed".to_string())?;
                    continue;
                }
                validate_requested_message_ids(
                    &topic_id,
                    expected_message_ids.get(&topic_id).and_then(Option::as_ref),
                    &frame.messages,
                )?;
                if frame.messages.is_empty() {
                    tx.send(BatchPullResult {
                        topic_id,
                        success: true,
                        parsed_count: 0,
                        failed_count: 0,
                        legacy_attachment_warnings: frame.legacy_attachment_warnings,
                        error: None,
                    })
                    .await
                    .map_err(|_| "Pull result receiver closed".to_string())?;
                    continue;
                }

                // Permit 在 spawn 前取得，避免大量已生成任务在运行时队列中堆积。
                while let Some(result) = spawn_handles.try_join_next() {
                    if let Err(error) = result {
                        log::warn!("[PullExecutor] Batch pull worker failed: {}", error);
                    }
                }
                let permit = sem
                    .clone()
                    .acquire_many_owned(pull_worker_permits(line.len())?)
                    .await
                    .map_err(|e| format!("Pull worker semaphore closed: {}", e))?;
                let app_clone = app.clone();
                let wq_clone = write_queue.clone();
                let tx_clone = tx.clone();
                let pull_dtos = frame.messages;
                let legacy_attachment_warnings = frame.legacy_attachment_warnings;

                spawn_handles.spawn(async move {
                    let start_t = std::time::Instant::now();
                    // ⚡ 核心转换：通过 DTO From 实现三层完全隔离，净化核心 ChatMessage
                    let messages: Vec<crate::vcp_modules::chat_manager::ChatMessage> = pull_dtos
                        .into_iter()
                        .map(crate::vcp_modules::chat_manager::ChatMessage::from)
                        .collect();

                    let decode_t = start_t.elapsed();
                    let _permit = permit;
                            let proc_start = std::time::Instant::now();
                            match process_topic_messages(
                                &app_clone,
                                &topic_id,
                                messages,
                                &wq_clone,
                                prerender_enabled,
                            )
                            .await
                            {
                                Ok((parsed, failed)) => {
                                    let proc_t = proc_start.elapsed();
                                    let total_t = start_t.elapsed();
                                    log::debug!(
                                        "[PullExecutor] [ProfileSummary] topic={} msgs={} | decode={:?} sem_wait={:?} process={:?} | total={:?}",
                                        topic_id, parsed, decode_t, std::time::Duration::ZERO, proc_t, total_t
                                    );
                                    let _ = tx_clone.send(BatchPullResult {
                                        topic_id,
                                        success: true,
                                        parsed_count: parsed,
                                        failed_count: failed,
                                        legacy_attachment_warnings,
                                        error: None,
                                    }).await;
                                }
                                Err(e) => {
                                    let _ = tx_clone.send(BatchPullResult {
                                        topic_id,
                                        success: false,
                                        parsed_count: 0,
                                        failed_count: 0,
                                        legacy_attachment_warnings,
                                        error: Some(e),
                                    }).await;
                                }
                            }
                });
            }

            // 循环结束后，游标指向 buffer 末尾，下一轮 chunk 进来时只需扫描新增部分
            if buffer.len() > MAX_NDJSON_LINE_BYTES {
                return Err("NDJSON frame exceeds 32MB budget".to_string());
            }
            search_start = buffer.len();
        }

        // 处理流结束后 buffer 中残留的非换行数据（兜底）
        if !buffer.is_empty() {
            if buffer.len() > MAX_NDJSON_LINE_BYTES {
                return Err("NDJSON trailing frame exceeds 32MB budget".to_string());
            }
            let frame = parse_topic_ndjson_frame(&buffer)?;
            ndjson_budget.observe_frame(buffer.len(), frame.messages.len())?;
            let topic_id = frame.topic_id;
            if !expected_topics.contains(&topic_id) {
                return Err(format!("NDJSON returned unexpected topicId {topic_id}"));
            }
            if !seen_topics.insert(topic_id.clone()) {
                return Err(format!("NDJSON returned duplicate topicId {topic_id}"));
            }
            for warning in &frame.warning_samples {
                crate::vcp_modules::sync::sync_service::emit_sync_log(
                    app,
                    "warning",
                    &format!("旧附件已省略: {warning}"),
                );
            }
            if let Some(topic_err) = frame.error {
                tx.send(BatchPullResult {
                    topic_id,
                    success: false,
                    parsed_count: 0,
                    failed_count: 0,
                    legacy_attachment_warnings: frame.legacy_attachment_warnings,
                    error: Some(format!("Desktop error: {}", topic_err)),
                })
                .await
                .map_err(|_| "Pull result receiver closed".to_string())?;
            } else {
                validate_requested_message_ids(
                    &topic_id,
                    expected_message_ids.get(&topic_id).and_then(Option::as_ref),
                    &frame.messages,
                )?;
                let pull_dtos = frame.messages;
                let legacy_attachment_warnings = frame.legacy_attachment_warnings;
                if !pull_dtos.is_empty() {
                    while let Some(result) = spawn_handles.try_join_next() {
                        if let Err(error) = result {
                            log::warn!("[PullExecutor] Batch pull worker failed: {}", error);
                        }
                    }
                    let app_clone = app.clone();
                    let wq_clone = write_queue.clone();
                    let tx_clone = tx.clone();
                    let permit = sem
                        .clone()
                        .acquire_many_owned(pull_worker_permits(buffer.len())?)
                        .await
                        .map_err(|e| format!("Pull worker semaphore closed: {}", e))?;
                    spawn_handles.spawn(async move {
                        let _permit = permit;
                        let messages: Vec<crate::vcp_modules::chat_manager::ChatMessage> =
                            pull_dtos
                                .into_iter()
                                .map(crate::vcp_modules::chat_manager::ChatMessage::from)
                                .collect();
                        match process_topic_messages(
                            &app_clone,
                            &topic_id,
                            messages,
                            &wq_clone,
                            prerender_enabled,
                        )
                        .await
                        {
                            Ok((parsed, failed)) => {
                                let _ = tx_clone
                                    .send(BatchPullResult {
                                        topic_id,
                                        success: true,
                                        parsed_count: parsed,
                                        failed_count: failed,
                                        legacy_attachment_warnings,
                                        error: None,
                                    })
                                    .await;
                            }
                            Err(e) => {
                                let _ = tx_clone
                                    .send(BatchPullResult {
                                        topic_id,
                                        success: false,
                                        parsed_count: 0,
                                        failed_count: 0,
                                        legacy_attachment_warnings,
                                        error: Some(e),
                                    })
                                    .await;
                            }
                        }
                    });
                } else {
                    tx.send(BatchPullResult {
                        topic_id,
                        success: true,
                        parsed_count: 0,
                        failed_count: 0,
                        legacy_attachment_warnings,
                        error: None,
                    })
                    .await
                    .map_err(|_| "Pull result receiver closed".to_string())?;
                }
            }
        }

        if seen_topics != expected_topics {
            let mut missing = expected_topics
                .difference(&seen_topics)
                .cloned()
                .collect::<Vec<_>>();
            missing.sort();
            return Err(format!("NDJSON response is missing topics {missing:?}"));
        }

        // ── 等待所有任务完成 ──
        drop(tx); // 关闭 channel，通知 receiver 不再有新消息
        let wait_for_workers = async move {
            while let Some(result) = spawn_handles.join_next().await {
                if let Err(error) = result {
                    log::warn!("[PullExecutor] Batch pull worker failed: {}", error);
                }
            }
        };
        wait_for_workers.await;
        let results = receiver_handle
            .await
            .map_err(|error| format!("Pull result receiver failed: {error}"))?;

        let ok_count = results.iter().filter(|r| r.success).count();
        let err_count = results.iter().filter(|r| !r.success).count();
        let msg = format!(
            "[PullExecutor] Batch pull completed: {}/{} topics processed, {} errors",
            ok_count, total, err_count
        );
        crate::vcp_modules::sync::sync_service::emit_sync_log(app, "info", &msg);
        Ok(results)
    }
}

#[cfg(test)]
mod ndjson_budget_tests {
    use super::{
        parse_topic_ndjson_frame, pull_worker_permits, validate_requested_message_ids,
        NdjsonBudget, MAX_NDJSON_LINE_BYTES, MAX_NDJSON_TOTAL_BYTES, PULL_WORKER_BUDGET_UNITS,
    };
    use serde_json::json;
    use std::collections::HashSet;

    const PROTOCOL_1_1_GOLDEN: &[u8] = include_bytes!("../fixtures/protocol_1_1_golden.json");
    const PROTOCOL_1_1_GOLDEN_SHA256: &str =
        "62654d4ecf6fc144c40b4635b5248e851a880ca003229bad37337e693bdb81ae";

    #[test]
    fn protocol_1_1_golden_bundle_and_canonical_output_are_stable() {
        assert_eq!(
            crate::vcp_modules::infra::utils::calculate_sha256(PROTOCOL_1_1_GOLDEN),
            PROTOCOL_1_1_GOLDEN_SHA256
        );
        let bundle: serde_json::Value =
            serde_json::from_slice(PROTOCOL_1_1_GOLDEN).expect("golden bundle JSON");
        assert_eq!(bundle["wireProtocol"], "1.1");

        for case in bundle["validFrames"]
            .as_array()
            .expect("valid golden frames")
        {
            let bytes = serde_json::to_vec(&case["input"]).expect("serialize golden frame");
            let parsed = parse_topic_ndjson_frame(&bytes).expect("valid golden frame");
            let expected = &case["expected"];
            assert_eq!(parsed.topic_id, expected["topicId"]);
            assert_eq!(parsed.messages.len() as u64, expected["messageCount"]);
            assert_eq!(
                parsed.legacy_attachment_warnings as u64,
                expected["warningCount"]
            );
            let hashes = parsed.messages[0]
                .attachments
                .as_ref()
                .expect("canonical attachments")
                .iter()
                .map(|attachment| attachment.hash.clone())
                .collect::<Vec<_>>();
            let expected_hashes = expected["attachmentHashes"]
                .as_array()
                .expect("expected hashes")
                .iter()
                .map(|value| value.as_str().expect("hash string").to_string())
                .collect::<Vec<_>>();
            assert_eq!(hashes, expected_hashes);
            assert_eq!(
                parsed.messages[0].attachments.as_ref().unwrap()[1]
                    .extracted_text
                    .as_deref(),
                expected["nestedExtractedText"].as_str()
            );
            assert!(parsed.messages[0].content.contains("你好"));
            assert!(parsed.messages[0].content.contains("raw html"));
            assert!(parsed.messages[1].content.is_empty());
            assert_eq!(
                serde_json::to_value(&parsed.messages).expect("canonical messages JSON"),
                expected["canonicalMessages"]
            );
            let content_hashes = parsed
                .messages
                .iter()
                .map(|message| {
                    let hashes = message
                        .attachments
                        .as_ref()
                        .map(|attachments| {
                            attachments
                                .iter()
                                .map(|attachment| attachment.hash.clone())
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    crate::vcp_modules::sync_hash::HashAggregator::compute_message_fingerprint(
                        &message.content,
                        &hashes,
                    )
                })
                .collect::<Vec<_>>();
            assert_eq!(
                serde_json::to_value(content_hashes).expect("content hashes JSON"),
                expected["contentHashes"]
            );
        }

        for case in bundle["invalidFrames"]
            .as_array()
            .expect("invalid golden frames")
        {
            let bytes = serde_json::to_vec(&case["input"]).expect("serialize invalid frame");
            let error = match parse_topic_ndjson_frame(&bytes) {
                Err(error) => error,
                Ok(_) => panic!("invalid golden frame was accepted"),
            };
            assert!(error.contains(
                case["errorContains"]
                    .as_str()
                    .expect("expected error fragment")
            ));
        }
    }

    #[test]
    fn rejects_oversized_line_total_and_frame_fanout() {
        let mut line = NdjsonBudget::new(1);
        assert!(line.observe_frame(MAX_NDJSON_LINE_BYTES + 1, 0).is_err());

        let mut total = NdjsonBudget::new(1);
        assert!(total.observe_chunk(MAX_NDJSON_TOTAL_BYTES).is_ok());
        assert!(total.observe_chunk(1).is_err());

        let mut frames = NdjsonBudget::new(1);
        assert!(frames.observe_frame(1, 1).is_ok());
        assert!(frames.observe_frame(1, 1).is_err());
    }

    #[test]
    fn rejects_entity_budget_before_unbounded_work_is_spawned() {
        let mut budget = NdjsonBudget::new(2);
        assert!(budget.observe_frame(1, 75_000).is_ok());
        assert!(budget.observe_frame(1, 25_001).is_err());
    }

    #[test]
    fn inbound_cleaner_preserves_messages_and_omits_only_bad_legacy_attachments() {
        let hash_a = "A".repeat(64);
        let hash_b = "b".repeat(64);
        let frame = json!({
            "topicId": "topic-unicode",
            "messages": [{
                "id": "message-1",
                "role": "user",
                "content": "你好 <section data-raw=\"yes\">raw html</section>",
                "timestamp": "123",
                "contentHash": "must-not-be-trusted",
                "attachments": [
                    { "type": "text/plain", "name": "flat.txt", "size": 1, "hash": hash_a },
                    { "type": "text/plain", "name": "nested.txt", "size": 2,
                      "_fileManagerData": { "hash": hash_b, "extractedText": "trusted text" },
                      "src": "desktop-only-path" },
                    { "type": "text/plain", "name": "null.txt", "size": 3, "hash": null },
                    { "type": "text/plain", "name": "missing.txt", "size": 4 },
                    { "type": "text/plain", "name": "conflict.txt", "size": 5,
                      "hash": "c".repeat(64), "_fileManagerData": { "hash": "d".repeat(64) } }
                ]
            }]
        });

        let parsed = parse_topic_ndjson_frame(frame.to_string().as_bytes())
            .expect("legacy frame should be narrowly cleaned");
        assert_eq!(parsed.topic_id, "topic-unicode");
        assert_eq!(parsed.legacy_attachment_warnings, 3);
        assert_eq!(parsed.warning_samples.len(), 3);
        let message = &parsed.messages[0];
        assert_eq!(message.timestamp, 123);
        assert!(message.content.contains("你好"));
        assert!(message.content_hash.is_none());
        let attachments = message.attachments.as_ref().expect("valid attachments");
        assert_eq!(attachments.len(), 2);
        assert_eq!(attachments[0].hash, "a".repeat(64));
        assert_eq!(attachments[1].hash, "b".repeat(64));
        assert_eq!(
            attachments[1].extracted_text.as_deref(),
            Some("trusted text")
        );
    }

    #[test]
    fn inbound_cleaner_uses_the_only_valid_hash_location() {
        let frame = json!({
            "topicId": "topic",
            "messages": [{
                "id": "message",
                "role": "user",
                "content": "",
                "timestamp": 1,
                "attachments": [
                    {
                        "type": "text/plain", "name": "nested.txt", "size": 1,
                        "hash": "invalid",
                        "_fileManagerData": { "hash": "f".repeat(64) }
                    },
                    {
                        "type": "text/plain", "name": "flat.txt", "size": 1,
                        "hash": "e".repeat(64),
                        "_fileManagerData": { "hash": null }
                    }
                ]
            }]
        });

        let parsed = parse_topic_ndjson_frame(frame.to_string().as_bytes())
            .expect("the sole valid hash must be used");
        assert_eq!(parsed.legacy_attachment_warnings, 0);
        let attachments = parsed.messages[0].attachments.as_ref().unwrap();
        assert_eq!(attachments[0].hash, "f".repeat(64));
        assert_eq!(attachments[1].hash, "e".repeat(64));
    }

    #[test]
    fn requested_message_ids_require_exact_response_coverage() {
        let messages = vec![
            crate::vcp_modules::sync_dto::MessagePullSyncDTO {
                id: "message-a".into(),
                role: "user".into(),
                name: None,
                content: String::new(),
                timestamp: 1,
                is_thinking: None,
                agent_id: None,
                group_id: None,
                topic_id: None,
                is_group_message: None,
                finish_reason: None,
                attachments: None,
                content_hash: None,
                avatar_color: None,
            },
            crate::vcp_modules::sync_dto::MessagePullSyncDTO {
                id: "message-b".into(),
                role: "assistant".into(),
                name: None,
                content: String::new(),
                timestamp: 2,
                is_thinking: None,
                agent_id: None,
                group_id: None,
                topic_id: None,
                is_group_message: None,
                finish_reason: None,
                attachments: None,
                content_hash: None,
                avatar_color: None,
            },
        ];
        assert!(validate_requested_message_ids(
            "topic",
            Some(&HashSet::from([
                "message-a".to_string(),
                "message-b".to_string(),
            ])),
            &messages,
        )
        .is_ok());
        for expected in [
            HashSet::new(),
            HashSet::from(["message-a".to_string()]),
            HashSet::from([
                "message-a".to_string(),
                "message-b".to_string(),
                "message-c".to_string(),
            ]),
        ] {
            assert!(validate_requested_message_ids("topic", Some(&expected), &messages).is_err());
        }
        assert!(validate_requested_message_ids("topic", None, &messages).is_ok());
    }

    #[test]
    fn pull_worker_permits_are_weighted_by_raw_frame_bytes() {
        assert_eq!(pull_worker_permits(1).unwrap(), 1);
        assert_eq!(pull_worker_permits(1024 * 1024).unwrap(), 1);
        assert_eq!(pull_worker_permits(1024 * 1024 + 1).unwrap(), 2);
        assert_eq!(
            pull_worker_permits(MAX_NDJSON_LINE_BYTES).unwrap(),
            PULL_WORKER_BUDGET_UNITS as u32
        );
    }

    #[test]
    fn inbound_cleaner_fails_closed_on_message_contract_errors() {
        for frame in [
            json!({ "topicId": "topic", "messages": [{ "id": "", "role": "user", "timestamp": 1 }] }),
            json!({ "topicId": "topic", "messages": [{ "id": "message", "role": "", "timestamp": 1 }] }),
            json!({ "topicId": "topic", "messages": [{ "id": "message", "role": "user", "timestamp": -1 }] }),
            json!({ "topicId": "topic", "messages": [{ "id": "message", "role": "user", "timestamp": u64::MAX.to_string() }] }),
            json!({ "topicId": "topic", "messages": [{ "id": "message", "role": "user", "timestamp": 1, "attachments": {} }] }),
        ] {
            assert!(parse_topic_ndjson_frame(frame.to_string().as_bytes()).is_err());
        }
    }
}

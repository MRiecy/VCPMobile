use crate::vcp_modules::chat_manager::ChatMessage;
use crate::vcp_modules::content_parser::{parse_content, ContentBlock};
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::sync_hash::HashAggregator;
use crate::vcp_modules::topic_types::{MessageKey, TopicActivityDto, TopicKey};
use serde::Serialize;

use sqlx::Row;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::mpsc;

pub const RENDERER_SCHEMA_VERSION: i64 = 1;
const MAX_SAFE_JSON_INTEGER: u64 = (1_u64 << 53) - 1;

pub(crate) fn resolve_message_updated_at(
    explicit_updated_at: Option<u64>,
    message_timestamp: u64,
    content_hash: &str,
    existing: Option<(&str, i64)>,
    now: i64,
) -> Result<i64, String> {
    if let Some((previous_hash, previous_updated_at)) = existing {
        let resolved = if previous_hash == content_hash {
            previous_updated_at
        } else {
            now.max(previous_updated_at.saturating_add(1))
        };
        if resolved < 0 || resolved as u64 > MAX_SAFE_JSON_INTEGER {
            return Err("message updatedAt exceeds the safe integer range".to_string());
        }
        return Ok(resolved);
    }
    if let Some(updated_at) = explicit_updated_at {
        if updated_at > MAX_SAFE_JSON_INTEGER {
            return Err("message updatedAt exceeds the safe integer range".to_string());
        }
        return i64::try_from(updated_at).map_err(|_| "message updatedAt is too large".to_string());
    }
    if message_timestamp > MAX_SAFE_JSON_INTEGER {
        return Err("message timestamp exceeds the safe integer range".to_string());
    }
    i64::try_from(message_timestamp).map_err(|_| "message timestamp is too large".to_string())
}

fn render_work_semaphore() -> std::sync::Arc<tokio::sync::Semaphore> {
    static SEMAPHORE: std::sync::OnceLock<std::sync::Arc<tokio::sync::Semaphore>> =
        std::sync::OnceLock::new();
    SEMAPHORE
        .get_or_init(|| {
            let permits = std::thread::available_parallelism()
                .map(|value| value.get())
                .unwrap_or(2)
                .clamp(1, 4);
            std::sync::Arc::new(tokio::sync::Semaphore::new(permits))
        })
        .clone()
}

async fn run_render_work<T, F>(work: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    let permit = render_work_semaphore()
        .acquire_owned()
        .await
        .map_err(|_| "render worker pool closed".to_string())?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        work()
    })
    .await
    .map_err(|error| format!("render worker failed: {}", error))?
}

pub async fn compile_render_async(content: String) -> Result<Vec<ContentBlock>, String> {
    run_render_work(move || Ok(MessageRenderCompiler::compile(&content))).await
}

pub async fn compile_and_serialize_render_async(
    content: String,
) -> Result<(Vec<ContentBlock>, Vec<u8>), String> {
    run_render_work(move || {
        let blocks = MessageRenderCompiler::compile(&content);
        let bytes = MessageRenderCompiler::serialize(&blocks)?;
        Ok((blocks, bytes))
    })
    .await
}

pub async fn serialize_render_async(blocks: Vec<ContentBlock>) -> Result<Vec<u8>, String> {
    run_render_work(move || MessageRenderCompiler::serialize(&blocks)).await
}

pub async fn deserialize_render_async(bytes: Vec<u8>) -> Result<Vec<ContentBlock>, String> {
    run_render_work(move || MessageRenderCompiler::deserialize(&bytes)).await
}

/// Writes a render result only while the source message still has the hash that was compiled.
/// This prevents a slow cache miss/re-render from overwriting a newer edit.
pub async fn write_render_cache_cas(
    db_state: &DbState,
    key: &MessageKey,
    observed_content_hash: &str,
    render_content: &[u8],
) -> Result<bool, String> {
    let now = chrono::Utc::now().timestamp_millis();
    let mut tx = db_state.write_transaction("message.render-cache").await?;
    let result = sqlx::query(
        "INSERT INTO render_cache (
            owner_type, owner_id, topic_id, msg_id, render_content, content_hash,
            renderer_schema_version, updated_at
         ) SELECT ?, ?, ?, ?, ?, ?, ?, ?
         WHERE EXISTS (
            SELECT 1 FROM messages
            WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?
              AND content_hash = ? AND deleted_at IS NULL
         )
         ON CONFLICT(owner_type, owner_id, topic_id, msg_id) DO UPDATE SET
            render_content = excluded.render_content,
            content_hash = excluded.content_hash,
            renderer_schema_version = excluded.renderer_schema_version,
            updated_at = excluded.updated_at
         WHERE EXISTS (
            SELECT 1 FROM messages
            WHERE owner_type = excluded.owner_type AND owner_id = excluded.owner_id
              AND topic_id = excluded.topic_id AND msg_id = excluded.msg_id
              AND content_hash = excluded.content_hash AND deleted_at IS NULL
         )",
    )
    .bind(&key.topic.owner_type)
    .bind(&key.topic.owner_id)
    .bind(&key.topic.topic_id)
    .bind(&key.msg_id)
    .bind(render_content)
    .bind(observed_content_hash)
    .bind(RENDERER_SCHEMA_VERSION)
    .bind(now)
    .bind(&key.topic.owner_type)
    .bind(&key.topic.owner_id)
    .bind(&key.topic.topic_id)
    .bind(&key.msg_id)
    .bind(observed_content_hash)
    .execute(&mut *tx)
    .await
    .map_err(|error| error.to_string())?;
    tx.commit().await.map_err(|error| error.to_string())?;
    Ok(result.rows_affected() == 1)
}

pub struct MessageRenderCompiler;

impl MessageRenderCompiler {
    /// Compiles raw message content into AST blocks (the "astbin" format base)
    pub fn compile(content: &str) -> Vec<ContentBlock> {
        // Core parse (now robust enough to handle HTML natively via content_parser)
        parse_content(content)
    }

    /// Serializes AST blocks to compressed binary (JSON + zstd)
    pub fn serialize(blocks: &[ContentBlock]) -> Result<Vec<u8>, String> {
        let json_bytes =
            serde_json::to_vec(blocks).map_err(|e| format!("json serialize failed: {}", e))?;
        let compressed = zstd::bulk::compress(&json_bytes, 3)
            .map_err(|e| format!("zstd compress failed: {}", e))?;
        Ok(compressed)
    }

    /// Deserializes compressed binary back to AST blocks (JSON + zstd)
    pub fn deserialize(bytes: &[u8]) -> Result<Vec<ContentBlock>, String> {
        // Use a generous upper bound for decompression; zstd will return exact size
        let decompressed = zstd::bulk::decompress(bytes, 16 * 1024 * 1024)
            .map_err(|e| format!("zstd decompress failed: {}", e))?;
        serde_json::from_slice(&decompressed).map_err(|e| format!("json deserialize failed: {}", e))
    }
}

#[tauri::command]
pub async fn process_message_content(
    _app_handle: AppHandle,
    content: String,
) -> Result<Vec<ContentBlock>, String> {
    // 1. 全量预解析 (调用统一的渲染编译器)
    compile_render_async(content).await
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RebuildProgress {
    pub current: usize,
    pub total: usize,
}

// =================================================================
// 通用三段流水线基础设施（Reader → Processor → Writer）
// =================================================================

type CachedMessageSource = (MessageKey, String, String);
type RenderCacheWrite = (MessageKey, String, Vec<u8>);

fn open_maintenance_rusqlite(db_path: &std::path::Path) -> Result<rusqlite::Connection, String> {
    let conn = rusqlite::Connection::open(db_path).map_err(|e| e.to_string())?;
    let journal_mode: String = conn
        .pragma_query_value(None, "journal_mode", |row| row.get(0))
        .map_err(|error| error.to_string())?;
    if !journal_mode.eq_ignore_ascii_case("wal") {
        return Err(format!(
            "Render-cache writer requires bootstrapped WAL mode, got {journal_mode}"
        ));
    }
    conn.execute("PRAGMA synchronous = NORMAL", []).ok();
    conn.execute("PRAGMA busy_timeout = 30000", []).ok();
    Ok(conn)
}

/// 分页流式读取已有渲染缓存的消息完整身份与明文内容。
async fn stream_cached_message_contents(
    pool: &sqlx::SqlitePool,
    tx: mpsc::Sender<CachedMessageSource>,
) -> Result<(), String> {
    let mut last_rowid = 0i64;
    const FETCH_SIZE: i64 = 500;

    loop {
        let rows = sqlx::query(
            "SELECT m.rowid, m.owner_type, m.owner_id, m.topic_id, m.msg_id, m.content, m.content_hash \
             FROM messages m \
             INNER JOIN render_cache r
               ON m.owner_type = r.owner_type AND m.owner_id = r.owner_id
              AND m.topic_id = r.topic_id AND m.msg_id = r.msg_id \
             WHERE m.rowid > ? \
             ORDER BY m.rowid \
             LIMIT ?",
        )
        .bind(last_rowid)
        .bind(FETCH_SIZE)
        .fetch_all(pool)
        .await
        .map_err(|error| format!("读取预渲染缓存来源失败: {error}"))?;

        if rows.is_empty() {
            break;
        }
        if let Some(last) = rows.last() {
            last_rowid = last.get::<i64, _>(0);
        }
        for row in rows {
            let owner_type: String = row.get("owner_type");
            let owner_id: String = row.get("owner_id");
            let topic_id: String = row.get("topic_id");
            let msg_id: String = row.get("msg_id");
            let content: String = row.get("content");
            let content_hash: String = row.get("content_hash");
            let key = MessageKey::new(TopicKey::new(owner_type, owner_id, topic_id), msg_id);
            if tx.send((key, content, content_hash)).await.is_err() {
                return Ok(());
            }
        }
    }
    Ok(())
}

fn update_render_cache_if_current(
    conn: &rusqlite::Connection,
    key: &MessageKey,
    content_hash: &str,
    bytes: &[u8],
    now: i64,
) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE render_cache SET
            render_content = ?1,
            content_hash = ?2,
            renderer_schema_version = ?3,
            updated_at = ?4
         WHERE owner_type = ?5 AND owner_id = ?6 AND topic_id = ?7 AND msg_id = ?8
           AND EXISTS (
             SELECT 1 FROM messages
             WHERE owner_type = ?5 AND owner_id = ?6 AND topic_id = ?7 AND msg_id = ?8
               AND content_hash = ?2 AND deleted_at IS NULL
           )",
        rusqlite::params![
            bytes,
            content_hash,
            RENDERER_SCHEMA_VERSION,
            now,
            &key.topic.owner_type,
            &key.topic.owner_id,
            &key.topic.topic_id,
            &key.msg_id,
        ],
    )
}

/// 渲染缓存批量 CAS Writer，带进度发射。
fn run_render_cache_update_writer(
    db_state: DbState,
    mut rx: mpsc::Receiver<Vec<RenderCacheWrite>>,
    progress_event: &str,
    app_handle: AppHandle,
    total: usize,
) -> tokio::task::JoinHandle<Result<(), String>> {
    let progress_event = progress_event.to_string();
    let connection = std::sync::Arc::new(std::sync::Mutex::new(None));

    tokio::spawn(async move {
        let mut processed = 0;
        let mut last_emit_time = std::time::Instant::now();
        let emit_interval = std::time::Duration::from_millis(32);

        while let Some(batch) = rx.recv().await {
            let batch_len = batch.len();
            let write_lease = db_state
                .write_coordinator()
                .acquire_lease("maintenance.render-cache")
                .await;
            let connection = connection.clone();
            let db_path = db_state.path.clone();
            tokio::task::spawn_blocking(move || -> Result<(), String> {
                let mut write_lease = write_lease;
                let write_started = std::time::Instant::now();
                write_lease.mark_outcome("transaction_failed");
                let mut connection = connection
                    .lock()
                    .map_err(|_| "预渲染缓存连接锁已损坏".to_string())?;
                if connection.is_none() {
                    *connection = Some(open_maintenance_rusqlite(&db_path)?);
                }
                let connection = connection
                    .as_mut()
                    .ok_or_else(|| "预渲染缓存连接初始化失败".to_string())?;
                let tx = connection
                    .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                    .map_err(|error| error.to_string())?;
                let now = chrono::Utc::now().timestamp_millis();
                for (key, content_hash, bytes) in batch {
                    update_render_cache_if_current(&tx, &key, &content_hash, &bytes, now)
                        .map_err(|e| e.to_string())?;
                }
                tx.commit().map_err(|e| e.to_string())?;
                write_lease.finish("committed", write_started.elapsed());
                Ok(())
            })
            .await
            .map_err(|error| format!("预渲染缓存写线程失败: {error}"))??;
            processed += batch_len;

            if last_emit_time.elapsed() >= emit_interval || processed == total {
                let _ = app_handle.emit(
                    &progress_event,
                    RebuildProgress {
                        current: processed,
                        total,
                    },
                );
                last_emit_time = std::time::Instant::now();
            }
        }
        Ok(())
    })
}

#[cfg(test)]
mod rebuild_cache_tests {
    use super::{resolve_message_updated_at, update_render_cache_if_current};
    use crate::vcp_modules::topic_types::{MessageKey, TopicKey};

    #[test]
    fn message_update_time_preserves_remote_and_advances_local_edits() {
        assert_eq!(
            resolve_message_updated_at(Some(200), 100, "new", Some(("old", 200)), 400)
                .expect("local edit with stale explicit time"),
            400
        );
        assert_eq!(
            resolve_message_updated_at(Some(500), 100, "new", None, 400)
                .expect("new message explicit time"),
            500
        );
        assert_eq!(
            resolve_message_updated_at(None, 100, "same", Some(("same", 200)), 400)
                .expect("unchanged message time"),
            200
        );
        assert_eq!(
            resolve_message_updated_at(None, 100, "new", Some(("old", 500)), 400)
                .expect("edited message time"),
            501
        );
        assert_eq!(
            resolve_message_updated_at(None, 100, "new", None, 400).expect("new message timestamp"),
            100
        );
    }

    #[test]
    fn rebuild_writer_never_overwrites_cache_for_a_newer_message_hash() {
        let conn = rusqlite::Connection::open_in_memory().expect("open test database");
        conn.execute_batch(
            "CREATE TABLE messages (
                owner_type TEXT NOT NULL,
                owner_id TEXT NOT NULL,
                topic_id TEXT NOT NULL,
                msg_id TEXT NOT NULL,
                content_hash TEXT NOT NULL,
                deleted_at INTEGER,
                PRIMARY KEY(owner_type, owner_id, topic_id, msg_id)
             );
             CREATE TABLE render_cache (
                owner_type TEXT NOT NULL,
                owner_id TEXT NOT NULL,
                topic_id TEXT NOT NULL,
                msg_id TEXT NOT NULL,
                render_content BLOB NOT NULL,
                content_hash TEXT NOT NULL,
                renderer_schema_version INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                PRIMARY KEY(owner_type, owner_id, topic_id, msg_id)
             );
             INSERT INTO messages VALUES
                ('agent', 'agent-a', 'topic', 'message', 'new-hash', NULL);
             INSERT INTO render_cache VALUES
                ('agent', 'agent-a', 'topic', 'message', x'09', 'new-hash', 1, 2);",
        )
        .expect("create cache fixture");
        let key = MessageKey::new(TopicKey::new("agent", "agent-a", "topic"), "message");

        let stale = update_render_cache_if_current(&conn, &key, "old-hash", &[1, 2, 3], 3)
            .expect("stale CAS update");
        assert_eq!(stale, 0);

        let (bytes, hash): (Vec<u8>, String) = conn
            .query_row(
                "SELECT render_content, content_hash FROM render_cache",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read cache after stale update");
        assert_eq!(bytes, vec![9]);
        assert_eq!(hash, "new-hash");

        let current = update_render_cache_if_current(&conn, &key, "new-hash", &[4, 5, 6], 4)
            .expect("current CAS update");
        assert_eq!(current, 1);
    }
}

// =================================================================
// 任务 1：刷新现有预渲染缓存
// =================================================================

#[tauri::command]
pub async fn rebuild_all_pre_renders(app_handle: AppHandle) -> Result<(), String> {
    let db_state = app_handle.state::<crate::vcp_modules::db_manager::DbState>();
    let pool = db_state.pool.clone();

    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM render_cache")
        .fetch_one(&pool)
        .await
        .map_err(|e| e.to_string())?;

    if total == 0 {
        return Ok(());
    }

    #[cfg(target_os = "android")]
    let _ = tauri_plugin_vcp_mobile::stream::start_stream_service_inner(
        &app_handle,
        "[预渲染缓存刷新] VCP Mobile",
    );

    let (tx_compiler, rx_compiler) = mpsc::channel::<CachedMessageSource>(1000);
    let (tx_writer, rx_writer) = mpsc::channel::<Vec<RenderCacheWrite>>(100);
    let total_count = total as usize;

    // --- Stage 3: Writer ---
    let writer_handle = run_render_cache_update_writer(
        db_state.inner().clone(),
        rx_writer,
        "render_rebuild_progress",
        app_handle.clone(),
        total_count,
    );

    // --- Stage 2: Parallel Compiler Workers ---
    let concurrency = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(2, 12);

    let rx_compiler = std::sync::Arc::new(tokio::sync::Mutex::new(rx_compiler));
    let mut compiler_handles = Vec::new();

    for _ in 0..concurrency {
        let rx_clone = rx_compiler.clone();
        let tx_writer_clone = tx_writer.clone();

        let handle = tokio::task::spawn_blocking(move || -> Result<(), String> {
            let mut batch = Vec::with_capacity(50);
            loop {
                let item = {
                    let mut rx = rx_clone.blocking_lock();
                    rx.blocking_recv()
                };

                match item {
                    Some((key, content, content_hash)) => {
                        let blocks = MessageRenderCompiler::compile(&content);
                        let bytes = MessageRenderCompiler::serialize(&blocks)?;
                        batch.push((key, content_hash, bytes));

                        if batch.len() >= 50
                            && tx_writer_clone
                                .blocking_send(std::mem::take(&mut batch))
                                .is_err()
                        {
                            return Err("预渲染缓存写入任务已提前结束".to_string());
                        }
                    }
                    None => {
                        if !batch.is_empty() {
                            tx_writer_clone
                                .blocking_send(batch)
                                .map_err(|_| "预渲染缓存写入任务已提前结束".to_string())?;
                        }
                        break;
                    }
                }
            }
            Ok(())
        });
        compiler_handles.push(handle);
    }
    drop(rx_compiler);

    // --- Stage 1: Reader ---
    let reader_handle =
        tokio::spawn(async move { stream_cached_message_contents(&pool, tx_compiler).await });

    // 等待流水线排空
    let reader_result = match reader_handle.await {
        Ok(result) => result,
        Err(error) => Err(format!("预渲染缓存读取任务失败: {error}")),
    };
    let compiler_result = futures_util::future::join_all(compiler_handles)
        .await
        .into_iter()
        .try_for_each(|result| match result {
            Ok(result) => result,
            Err(error) => Err(format!("预渲染编译任务失败: {error}")),
        });
    drop(tx_writer);

    let writer_result = match writer_handle.await {
        Ok(result) => result,
        Err(error) => Err(format!("预渲染缓存写入任务失败: {error}")),
    };

    #[cfg(target_os = "android")]
    let _ = tauri_plugin_vcp_mobile::stream::stop_stream_service_inner(
        &app_handle,
        "[预渲染缓存刷新] VCP Mobile",
    );

    reader_result?;
    compiler_result?;
    writer_result?;

    // 补偿 100% 进度
    let _ = app_handle.emit(
        "render_rebuild_progress",
        RebuildProgress {
            current: total_count,
            total: total_count,
        },
    );
    Ok(())
}

/// Internal message repository for DB operations
pub struct MessageRepository;

struct ExistingMessageState {
    role: String,
    name: Option<String>,
    agent_id: Option<String>,
    content: String,
    timestamp: i64,
    is_group_message: bool,
    group_id: Option<String>,
    finish_reason: Option<String>,
    content_hash: String,
    updated_at: i64,
    deleted_at: Option<i64>,
}

impl MessageRepository {
    fn attachment_hash(
        attachment: &crate::vcp_modules::chat_manager::Attachment,
    ) -> Result<&str, String> {
        let hash = attachment.hash.as_deref().ok_or_else(|| {
            format!(
                "Attachment {} requires a SHA-256 content hash",
                attachment.name
            )
        })?;
        if hash != hash.to_ascii_lowercase()
            || !crate::vcp_modules::infra::utils::is_valid_cas_hash(hash)
        {
            return Err(format!(
                "Attachment {} has an invalid SHA-256 content hash",
                attachment.name
            ));
        }
        Ok(hash)
    }

    async fn load_upsert_target_state(
        tx: &mut sqlx::SqliteConnection,
        key: &TopicKey,
        msg_id: &str,
    ) -> Result<Option<ExistingMessageState>, String> {
        let topic_is_live: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM topics
                WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL
             )",
        )
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|error| error.to_string())?;
        if !topic_is_live {
            return Err(format!("topic {} is deleted or missing", key.topic_id));
        }

        let existing_row = sqlx::query(
            "SELECT role, name, agent_id, content, timestamp, is_group_message,
                    group_id, finish_reason, content_hash, updated_at, deleted_at
             FROM messages
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?",
        )
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id)
        .bind(msg_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|error| error.to_string())?;
        let existing = existing_row
            .map(|row| {
                Ok::<_, String>(ExistingMessageState {
                    role: row.try_get("role").map_err(|error| error.to_string())?,
                    name: row.try_get("name").map_err(|error| error.to_string())?,
                    agent_id: row.try_get("agent_id").map_err(|error| error.to_string())?,
                    content: row.try_get("content").map_err(|error| error.to_string())?,
                    timestamp: row
                        .try_get("timestamp")
                        .map_err(|error| error.to_string())?,
                    is_group_message: row
                        .try_get::<i64, _>("is_group_message")
                        .map_err(|error| error.to_string())?
                        != 0,
                    group_id: row.try_get("group_id").map_err(|error| error.to_string())?,
                    finish_reason: row
                        .try_get("finish_reason")
                        .map_err(|error| error.to_string())?,
                    content_hash: row
                        .try_get("content_hash")
                        .map_err(|error| error.to_string())?,
                    updated_at: row
                        .try_get("updated_at")
                        .map_err(|error| error.to_string())?,
                    deleted_at: row
                        .try_get("deleted_at")
                        .map_err(|error| error.to_string())?,
                })
            })
            .transpose()?;
        if existing
            .as_ref()
            .is_some_and(|state| state.deleted_at.is_some())
        {
            return Err(format!(
                "message {msg_id} is tombstoned and cannot be restored by upsert"
            ));
        }
        Ok(existing)
    }

    pub async fn upsert_message(
        tx: &mut sqlx::SqliteConnection,
        message: &ChatMessage,
        key: &TopicKey,
        render_content: &[u8],
        skip_bubble: bool,
    ) -> Result<Option<TopicActivityDto>, String> {
        // 1. 计算核心内容指纹 (通过 HashAggregator)
        let attachment_hashes: Vec<String> = message
            .attachments
            .as_ref()
            .map(|atts| {
                atts.iter()
                    .map(|attachment| Self::attachment_hash(attachment).map(str::to_owned))
                    .collect::<Result<Vec<_>, String>>()
            })
            .transpose()?
            .unwrap_or_default();

        let content_hash = HashAggregator::compute_message_fingerprint(
            &message.id,
            &message.role,
            message.name.as_deref(),
            &message.content,
            message.timestamp,
            message.agent_id.as_deref(),
            &attachment_hashes,
        );
        let existing = Self::load_upsert_target_state(tx, key, &message.id).await?;
        let effective_updated_at = resolve_message_updated_at(
            message.updated_at,
            message.timestamp,
            &content_hash,
            existing
                .as_ref()
                .map(|state| (state.content_hash.as_str(), state.updated_at)),
            chrono::Utc::now().timestamp_millis(),
        )
        .map_err(|error| format!("message {} {error}", message.id))?;
        let is_group_message = message.is_group_message.unwrap_or(false);
        let message_timestamp = message.timestamp as i64;
        let fingerprint_changed = existing
            .as_ref()
            .is_none_or(|state| state.content_hash != content_hash);
        let content_changed = existing
            .as_ref()
            .is_none_or(|state| state.content != message.content);
        let core_changed = existing.as_ref().is_none_or(|state| {
            state.role != message.role
                || state.name != message.name
                || state.agent_id != message.agent_id
                || state.content != message.content
                || state.timestamp != message_timestamp
                || state.is_group_message != is_group_message
                || state.group_id != message.group_id
                || state.finish_reason != message.finish_reason
                || state.content_hash != content_hash
                || state.updated_at != effective_updated_at
        });

        // 2. 插入或更新消息 (不含 render_content)
        if core_changed {
            let changed = sqlx::query(
                "INSERT INTO messages (
                owner_type, owner_id, topic_id, msg_id, role, name, agent_id, content, timestamp,
                is_group_message, group_id, finish_reason,
                content_hash,
                created_at, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(owner_type, owner_id, topic_id, msg_id) DO UPDATE SET
                content = excluded.content,
                role = excluded.role,
                name = excluded.name,
                agent_id = excluded.agent_id,
                timestamp = excluded.timestamp,
                is_group_message = excluded.is_group_message,
                group_id = excluded.group_id,
                finish_reason = excluded.finish_reason,
                content_hash = excluded.content_hash,
                updated_at = excluded.updated_at
             WHERE messages.deleted_at IS NULL",
            )
            .bind(&key.owner_type)
            .bind(&key.owner_id)
            .bind(&key.topic_id)
            .bind(&message.id)
            .bind(&message.role)
            .bind(&message.name)
            .bind(&message.agent_id)
            .bind(&message.content)
            .bind(message_timestamp)
            .bind(is_group_message)
            .bind(&message.group_id)
            .bind(&message.finish_reason)
            .bind(&content_hash)
            .bind(message_timestamp) // created_at
            .bind(effective_updated_at)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
            if changed.rows_affected() != 1 {
                return Err(format!("Message {} disappeared during upsert", message.id));
            }
        }

        // 2.1 插入或更新渲染缓存 (独立表)
        sqlx::query(
            "INSERT INTO render_cache (
                owner_type, owner_id, topic_id, msg_id, render_content, content_hash,
                renderer_schema_version, updated_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(owner_type, owner_id, topic_id, msg_id) DO UPDATE SET
                render_content = excluded.render_content,
                content_hash = excluded.content_hash,
                renderer_schema_version = excluded.renderer_schema_version,
                updated_at = excluded.updated_at
             WHERE render_cache.render_content IS NOT excluded.render_content
                OR render_cache.content_hash IS NOT excluded.content_hash
                OR render_cache.renderer_schema_version IS NOT excluded.renderer_schema_version
                OR render_cache.updated_at IS NOT excluded.updated_at",
        )
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id)
        .bind(&message.id)
        .bind(render_content)
        .bind(&content_hash)
        .bind(RENDERER_SCHEMA_VERSION)
        .bind(message.timestamp as i64)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

        // 2.2 FTS 只消费正文变化；角色、附件或本地状态变化不重写索引。
        if content_changed {
            sqlx::query(
                "DELETE FROM messages_fts
                 WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?",
            )
            .bind(&key.owner_type)
            .bind(&key.owner_id)
            .bind(&key.topic_id)
            .bind(&message.id)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;

            sqlx::query(
                "INSERT INTO messages_fts (msg_id, topic_id, content, owner_type, owner_id)
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(&message.id)
            .bind(&key.topic_id)
            .bind(&message.content)
            .bind(&key.owner_type)
            .bind(&key.owner_id)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
        }

        // Handle attachments
        if let Some(ref attachments) = message.attachments {
            Self::upsert_attachments_for_message(
                tx,
                key,
                &message.id,
                message.timestamp as i64,
                attachments,
            )
            .await?;
        } else {
            sqlx::query(
                "DELETE FROM message_attachments
                 WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?",
            )
            .bind(&key.owner_type)
            .bind(&key.owner_id)
            .bind(&key.topic_id)
            .bind(&message.id)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
        }

        // 3. 触发聚合哈希冒泡 (通过 HashAggregator 统一处理)
        if !skip_bubble && fingerprint_changed {
            return HashAggregator::bubble_from_topic(tx, key).await.map(Some);
        }

        Ok(None)
    }

    async fn upsert_attachments_for_message(
        tx: &mut sqlx::SqliteConnection,
        key: &TopicKey,
        msg_id: &str,
        timestamp: i64,
        attachments: &[crate::vcp_modules::chat_manager::Attachment],
    ) -> Result<(), String> {
        for (i, att) in attachments.iter().enumerate() {
            let hash = Self::attachment_hash(att)?;

            let image_frames = att
                .image_frames
                .as_ref()
                .and_then(|frames| serde_json::to_string(frames).ok());

            sqlx::query(
                "INSERT INTO attachments (
                    hash, mime_type, size, internal_path, extracted_text, image_frames, thumbnail_path,
                    created_at, updated_at
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
                 ON CONFLICT(hash) DO UPDATE SET
                    mime_type = excluded.mime_type,
                    size = excluded.size,
                    internal_path = CASE
                        WHEN excluded.internal_path <> '' THEN excluded.internal_path
                        ELSE attachments.internal_path
                    END,
                    extracted_text = COALESCE(attachments.extracted_text, excluded.extracted_text),
                    image_frames = COALESCE(attachments.image_frames, excluded.image_frames),
                    thumbnail_path = COALESCE(attachments.thumbnail_path, excluded.thumbnail_path),
                    updated_at = excluded.updated_at
                 WHERE attachments.mime_type IS NOT excluded.mime_type
                    OR attachments.size IS NOT excluded.size
                    OR (excluded.internal_path <> '' AND attachments.internal_path IS NOT excluded.internal_path)
                    OR (attachments.extracted_text IS NULL AND excluded.extracted_text IS NOT NULL)
                    OR (attachments.image_frames IS NULL AND excluded.image_frames IS NOT NULL)
                    OR (attachments.thumbnail_path IS NULL AND excluded.thumbnail_path IS NOT NULL)"
            )
            .bind(hash)
            .bind(&att.r#type)
            .bind(att.size as i64)
            .bind(&att.internal_path)
            .bind(&att.extracted_text)
            .bind(image_frames)
            .bind(&att.thumbnail_path)
            .bind(timestamp)
            .bind(timestamp)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;

            sqlx::query(
                "INSERT INTO message_attachments (
                    owner_type, owner_id, topic_id, msg_id, hash, attachment_order,
                    display_name, src, status, created_at
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                 ON CONFLICT(owner_type, owner_id, topic_id, msg_id, attachment_order)
                 DO UPDATE SET
                    hash = excluded.hash,
                    display_name = excluded.display_name,
                    src = excluded.src,
                    status = excluded.status,
                    created_at = excluded.created_at
                 WHERE message_attachments.hash IS NOT excluded.hash
                    OR message_attachments.display_name IS NOT excluded.display_name
                    OR message_attachments.src IS NOT excluded.src
                    OR message_attachments.status IS NOT excluded.status
                    OR message_attachments.created_at IS NOT excluded.created_at",
            )
            .bind(&key.owner_type)
            .bind(&key.owner_id)
            .bind(&key.topic_id)
            .bind(msg_id)
            .bind(hash)
            .bind(i as i32)
            .bind(&att.name)
            .bind(&att.src)
            .bind(&att.status)
            .bind(timestamp)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
        }

        sqlx::query(
            "DELETE FROM message_attachments
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?
               AND attachment_order >= ?",
        )
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id)
        .bind(msg_id)
        .bind(i32::try_from(attachments.len()).map_err(|_| "Too many attachments".to_string())?)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

        Ok(())
    }
}

#[cfg(test)]
mod tombstone_tests {
    use super::MessageRepository;
    use crate::vcp_modules::topic_types::TopicKey;

    async fn test_pool() -> sqlx::SqlitePool {
        sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open test database")
    }

    #[tokio::test]
    async fn generic_upsert_rejects_deleted_topic_and_message_targets() {
        let pool = test_pool().await;
        sqlx::query(
            "CREATE TABLE topics (
                owner_type TEXT, owner_id TEXT, topic_id TEXT, deleted_at INTEGER,
                PRIMARY KEY(owner_type, owner_id, topic_id)
             );
             CREATE TABLE messages (
                owner_type TEXT NOT NULL,
                owner_id TEXT NOT NULL,
                topic_id TEXT NOT NULL,
                msg_id TEXT NOT NULL,
                role TEXT NOT NULL DEFAULT '',
                name TEXT,
                agent_id TEXT,
                content TEXT NOT NULL DEFAULT '',
                timestamp INTEGER NOT NULL DEFAULT 0,
                is_group_message INTEGER NOT NULL DEFAULT 0,
                group_id TEXT,
                finish_reason TEXT,
                content_hash TEXT NOT NULL DEFAULT '',
                updated_at INTEGER NOT NULL DEFAULT 0,
                deleted_at INTEGER,
                PRIMARY KEY(owner_type, owner_id, topic_id, msg_id)
             );",
        )
        .execute(&pool)
        .await
        .expect("create tombstone tables");
        sqlx::query(
            "INSERT INTO topics VALUES
                ('agent', 'agent-a', 'live-topic', NULL),
                ('agent', 'agent-a', 'deleted-topic', 7);
             INSERT INTO messages VALUES
                ('agent', 'agent-a', 'live-topic', 'deleted-message', '', NULL, NULL, '', 0, 0,
                 NULL, NULL, '', 0, 8);",
        )
        .execute(&pool)
        .await
        .expect("insert tombstones");

        let mut tx = pool.begin().await.expect("begin transaction");
        let live = TopicKey::new("agent", "agent-a", "live-topic");
        let deleted = TopicKey::new("agent", "agent-a", "deleted-topic");
        MessageRepository::load_upsert_target_state(&mut tx, &live, "new-message")
            .await
            .expect("new message in a live topic is allowed");
        let message_error =
            MessageRepository::load_upsert_target_state(&mut tx, &live, "deleted-message")
                .await
                .err()
                .expect("message tombstone is monotonic");
        assert!(message_error.contains("tombstoned"));
        let topic_error =
            MessageRepository::load_upsert_target_state(&mut tx, &deleted, "new-message")
                .await
                .err()
                .expect("topic tombstone blocks child writes");
        assert!(topic_error.contains("deleted or missing"));
    }
}

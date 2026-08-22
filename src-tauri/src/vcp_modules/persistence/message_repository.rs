use crate::vcp_modules::chat_manager::ChatMessage;
use crate::vcp_modules::content_parser::{parse_content, ContentBlock};
use crate::vcp_modules::sync_hash::HashAggregator;
use serde::Serialize;

use sqlx::Row;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::mpsc;

pub const RENDERER_SCHEMA_VERSION: i64 = 1;
const MAX_SAFE_JSON_INTEGER: u64 = (1_u64 << 53) - 1;

fn resolve_message_updated_at(
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
    pool: &sqlx::SqlitePool,
    topic_id: &str,
    msg_id: &str,
    observed_content_hash: &str,
    render_content: &[u8],
) -> Result<bool, String> {
    let now = chrono::Utc::now().timestamp_millis();
    let result = sqlx::query(
        "INSERT INTO render_cache (
            topic_id, msg_id, render_content, content_hash, renderer_schema_version, updated_at
         ) SELECT ?, ?, ?, ?, ?, ?
         WHERE EXISTS (
            SELECT 1 FROM messages
            WHERE topic_id = ? AND msg_id = ? AND content_hash = ? AND deleted_at IS NULL
         )
         ON CONFLICT(topic_id, msg_id) DO UPDATE SET
            render_content = excluded.render_content,
            content_hash = excluded.content_hash,
            renderer_schema_version = excluded.renderer_schema_version,
            updated_at = excluded.updated_at
         WHERE EXISTS (
            SELECT 1 FROM messages
            WHERE topic_id = excluded.topic_id AND msg_id = excluded.msg_id
              AND content_hash = excluded.content_hash AND deleted_at IS NULL
         )",
    )
    .bind(topic_id)
    .bind(msg_id)
    .bind(render_content)
    .bind(observed_content_hash)
    .bind(RENDERER_SCHEMA_VERSION)
    .bind(now)
    .bind(topic_id)
    .bind(msg_id)
    .bind(observed_content_hash)
    .execute(pool)
    .await
    .map_err(|error| error.to_string())?;
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

/// Simple zstd compressor for raw text content.
/// Text compresses very well (often 3-10x) with low overhead.
pub struct ContentCompressor;

impl ContentCompressor {
    #[allow(dead_code)]
    pub fn compress(text: &str) -> Result<Vec<u8>, String> {
        zstd::bulk::compress(text.as_bytes(), 3)
            .map_err(|e| format!("zstd compress content failed: {}", e))
    }

    pub fn decompress(bytes: &[u8]) -> Result<String, String> {
        let decompressed = zstd::bulk::decompress(bytes, 16 * 1024 * 1024)
            .map_err(|e| format!("zstd decompress content failed: {}", e))?;
        String::from_utf8(decompressed)
            .map_err(|e| format!("content decompression not valid utf-8: {}", e))
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

type CachedMessageSource = (String, String, String, String);
type RenderCacheWrite = (String, String, String, Vec<u8>);

fn open_maintenance_rusqlite(db_path: &std::path::Path) -> Result<rusqlite::Connection, String> {
    let conn = rusqlite::Connection::open(db_path).map_err(|e| e.to_string())?;
    conn.execute("PRAGMA journal_mode = WAL", []).ok();
    conn.execute("PRAGMA synchronous = NORMAL", []).ok();
    conn.execute("PRAGMA busy_timeout = 30000", []).ok();
    Ok(conn)
}

/// 分页流式读取已有渲染缓存的消息的 (topic_id, msg_id, content)，content 为明文字符串
async fn stream_cached_message_contents(
    pool: &sqlx::SqlitePool,
    tx: mpsc::Sender<CachedMessageSource>,
) -> Result<(), String> {
    let mut last_rowid = 0i64;
    const FETCH_SIZE: i64 = 500;

    loop {
        let rows = sqlx::query(
            "SELECT m.rowid, m.topic_id, m.msg_id, m.content, m.content_hash \
             FROM messages m \
             INNER JOIN render_cache r ON m.topic_id = r.topic_id AND m.msg_id = r.msg_id \
             WHERE m.rowid > ? \
             ORDER BY m.rowid \
             LIMIT ?",
        )
        .bind(last_rowid)
        .bind(FETCH_SIZE)
        .fetch_all(pool)
        .await;

        match rows {
            Ok(rows) if !rows.is_empty() => {
                if let Some(last) = rows.last() {
                    last_rowid = last.get::<i64, _>(0);
                }
                for row in rows {
                    let topic_id: String = row.get("topic_id");
                    let msg_id: String = row.get("msg_id");
                    let content: String = row.get("content");
                    let content_hash: String = row.get("content_hash");
                    if tx
                        .send((topic_id, msg_id, content, content_hash))
                        .await
                        .is_err()
                    {
                        return Ok(());
                    }
                }
            }
            _ => break,
        }
    }
    Ok(())
}

fn update_render_cache_if_current(
    conn: &rusqlite::Connection,
    topic_id: &str,
    msg_id: &str,
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
         WHERE topic_id = ?5 AND msg_id = ?6
           AND EXISTS (
             SELECT 1 FROM messages
             WHERE topic_id = ?5 AND msg_id = ?6
               AND content_hash = ?2 AND deleted_at IS NULL
           )",
        rusqlite::params![
            bytes,
            content_hash,
            RENDERER_SCHEMA_VERSION,
            now,
            topic_id,
            msg_id,
        ],
    )
}

/// 渲染缓存批量 CAS Writer，带进度发射。
fn run_render_cache_update_writer(
    db_path: &std::path::Path,
    mut rx: mpsc::Receiver<Vec<RenderCacheWrite>>,
    progress_event: &str,
    app_handle: AppHandle,
    total: usize,
) -> tokio::task::JoinHandle<Result<(), String>> {
    let progress_event = progress_event.to_string();
    let db_path = db_path.to_path_buf();

    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let mut conn = open_maintenance_rusqlite(&db_path)?;
        let mut processed = 0;
        let mut last_emit_time = std::time::Instant::now();
        let emit_interval = std::time::Duration::from_millis(32);

        while let Some(batch) = rx.blocking_recv() {
            let tx = conn.transaction().map_err(|e| e.to_string())?;
            {
                let now = chrono::Utc::now().timestamp_millis();
                for (topic_id, msg_id, content_hash, bytes) in batch {
                    update_render_cache_if_current(
                        &tx,
                        &topic_id,
                        &msg_id,
                        &content_hash,
                        &bytes,
                        now,
                    )
                    .map_err(|e| e.to_string())?;
                    processed += 1;
                }
            }
            tx.commit().map_err(|e| e.to_string())?;

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
                topic_id TEXT NOT NULL,
                msg_id TEXT NOT NULL,
                content_hash TEXT NOT NULL,
                deleted_at INTEGER,
                PRIMARY KEY(topic_id, msg_id)
             );
             CREATE TABLE render_cache (
                topic_id TEXT NOT NULL,
                msg_id TEXT NOT NULL,
                render_content BLOB NOT NULL,
                content_hash TEXT NOT NULL,
                renderer_schema_version INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                PRIMARY KEY(topic_id, msg_id)
             );
             INSERT INTO messages VALUES ('topic', 'message', 'new-hash', NULL);
             INSERT INTO render_cache VALUES ('topic', 'message', x'09', 'new-hash', 1, 2);",
        )
        .expect("create cache fixture");

        let stale =
            update_render_cache_if_current(&conn, "topic", "message", "old-hash", &[1, 2, 3], 3)
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

        let current =
            update_render_cache_if_current(&conn, "topic", "message", "new-hash", &[4, 5, 6], 4)
                .expect("current CAS update");
        assert_eq!(current, 1);
    }
}

// =================================================================
// 任务 1：全量预渲染重建
// =================================================================

#[tauri::command]
pub async fn rebuild_all_pre_renders(app_handle: AppHandle) -> Result<(), String> {
    let db_state = app_handle.state::<crate::vcp_modules::db_manager::DbState>();
    let pool = db_state.pool.clone();
    let db_path = db_state.path.clone();

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
        "[预渲染重建] VCP Mobile",
    );

    let (tx_compiler, rx_compiler) = mpsc::channel::<CachedMessageSource>(1000);
    let (tx_writer, rx_writer) = mpsc::channel::<Vec<RenderCacheWrite>>(100);
    let total_count = total as usize;

    // --- Stage 3: Writer ---
    let writer_handle = run_render_cache_update_writer(
        &db_path,
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

        let handle = tokio::task::spawn_blocking(move || {
            let mut batch = Vec::with_capacity(50);
            loop {
                let item = {
                    let mut rx = rx_clone.blocking_lock();
                    rx.blocking_recv()
                };

                match item {
                    Some((topic_id, msg_id, content, content_hash)) => {
                        let blocks = MessageRenderCompiler::compile(&content);
                        if let Ok(bytes) = MessageRenderCompiler::serialize(&blocks) {
                            batch.push((topic_id, msg_id, content_hash, bytes));
                        }

                        if batch.len() >= 50
                            && tx_writer_clone
                                .blocking_send(std::mem::take(&mut batch))
                                .is_err()
                        {
                            break;
                        }
                    }
                    None => {
                        if !batch.is_empty() {
                            let _ = tx_writer_clone.blocking_send(batch);
                        }
                        break;
                    }
                }
            }
        });
        compiler_handles.push(handle);
    }

    // --- Stage 1: Reader ---
    let reader_handle = tokio::spawn(async move {
        let (tx_inner, mut rx_inner) = mpsc::channel::<CachedMessageSource>(1000);

        let stream_handle = tokio::spawn(async move {
            let _ = stream_cached_message_contents(&pool, tx_inner).await;
        });

        while let Some((topic_id, msg_id, content, content_hash)) = rx_inner.recv().await {
            if tx_compiler
                .send((topic_id, msg_id, content, content_hash))
                .await
                .is_err()
            {
                break;
            }
        }
        drop(tx_compiler);
        let _ = stream_handle.await;
    });

    // 等待流水线排空
    let _ = reader_handle.await;
    let _ = futures_util::future::join_all(compiler_handles).await;
    drop(tx_writer);

    let write_res = writer_handle.await.map_err(|e| e.to_string());

    #[cfg(target_os = "android")]
    let _ = tauri_plugin_vcp_mobile::stream::stop_stream_service_inner(
        &app_handle,
        "[预渲染重建] VCP Mobile",
    );

    write_res??;

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

impl MessageRepository {
    async fn ensure_upsert_target_live(
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        topic_id: &str,
        msg_id: &str,
    ) -> Result<(), String> {
        let topic_is_live: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM topics WHERE topic_id = ? AND deleted_at IS NULL)",
        )
        .bind(topic_id)
        .fetch_one(&mut **tx)
        .await
        .map_err(|error| error.to_string())?;
        if !topic_is_live {
            return Err(format!("topic {topic_id} is deleted or missing"));
        }

        let deleted_at: Option<Option<i64>> =
            sqlx::query_scalar("SELECT deleted_at FROM messages WHERE topic_id = ? AND msg_id = ?")
                .bind(topic_id)
                .bind(msg_id)
                .fetch_optional(&mut **tx)
                .await
                .map_err(|error| error.to_string())?;
        if matches!(deleted_at, Some(Some(_))) {
            return Err(format!(
                "message {msg_id} is tombstoned and cannot be restored by upsert"
            ));
        }
        Ok(())
    }

    pub async fn upsert_message(
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        message: &ChatMessage,
        topic_id: &str,
        render_content: &[u8],
        skip_bubble: bool,
    ) -> Result<(), String> {
        Self::ensure_upsert_target_live(tx, topic_id, &message.id).await?;

        // 1. 计算核心内容指纹 (通过 HashAggregator)
        let attachment_hashes: Vec<String> = message
            .attachments
            .as_ref()
            .map(|atts| {
                atts.iter()
                    .map(|a| a.hash.clone().unwrap_or_default())
                    .filter(|h| !h.is_empty())
                    .collect()
            })
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
        let existing: Option<(String, i64)> = sqlx::query_as(
            "SELECT content_hash, updated_at FROM messages WHERE topic_id = ? AND msg_id = ?",
        )
        .bind(topic_id)
        .bind(&message.id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|error| error.to_string())?;
        let effective_updated_at = resolve_message_updated_at(
            message.updated_at,
            message.timestamp,
            &content_hash,
            existing
                .as_ref()
                .map(|(hash, updated_at)| (hash.as_str(), *updated_at)),
            chrono::Utc::now().timestamp_millis(),
        )
        .map_err(|error| format!("message {} {error}", message.id))?;

        // 2. 插入或更新消息 (不含 render_content)
        sqlx::query(
            "INSERT INTO messages (
                msg_id, topic_id, role, name, agent_id, content, timestamp,
                is_group_message, group_id, finish_reason,
                content_hash,
                created_at, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(topic_id, msg_id) DO UPDATE SET
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
        .bind(&message.id)
        .bind(topic_id)
        .bind(&message.role)
        .bind(&message.name)
        .bind(&message.agent_id)
        .bind(&message.content)
        .bind(message.timestamp as i64)
        .bind(message.is_group_message.unwrap_or(false))
        .bind(&message.group_id)
        .bind(&message.finish_reason)
        .bind(&content_hash)
        .bind(message.timestamp as i64) // created_at
        .bind(effective_updated_at)
        .execute(&mut **tx)
        .await
        .map_err(|e| e.to_string())?;

        // 2.1 插入或更新渲染缓存 (独立表)
        sqlx::query(
            "INSERT INTO render_cache (
                topic_id, msg_id, render_content, content_hash, renderer_schema_version, updated_at
             ) VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT(topic_id, msg_id) DO UPDATE SET
                render_content = excluded.render_content,
                content_hash = excluded.content_hash,
                renderer_schema_version = excluded.renderer_schema_version,
                updated_at = excluded.updated_at",
        )
        .bind(topic_id)
        .bind(&message.id)
        .bind(render_content)
        .bind(&content_hash)
        .bind(RENDERER_SCHEMA_VERSION)
        .bind(message.timestamp as i64)
        .execute(&mut **tx)
        .await
        .map_err(|e| e.to_string())?;

        // 2.2 同步写入全文检索 FTS5 虚拟表 (仅在消息未删除时同步明文，FTS5 不支持 ON CONFLICT)
        // trigram 分词器（migration 0008 起）直接索引原文，无需 CJK 预处理
        sqlx::query("DELETE FROM messages_fts WHERE topic_id = ? AND msg_id = ?")
            .bind(topic_id)
            .bind(&message.id)
            .execute(&mut **tx)
            .await
            .map_err(|e| e.to_string())?;

        sqlx::query("INSERT INTO messages_fts (msg_id, topic_id, content) VALUES (?, ?, ?)")
            .bind(&message.id)
            .bind(topic_id)
            .bind(&message.content)
            .execute(&mut **tx)
            .await
            .map_err(|e| e.to_string())?;

        // Handle attachments
        if let Some(ref attachments) = message.attachments {
            Self::upsert_attachments_for_message(
                tx,
                topic_id,
                &message.id,
                message.timestamp as i64,
                attachments,
            )
            .await?;
        } else {
            sqlx::query("DELETE FROM message_attachments WHERE topic_id = ? AND msg_id = ?")
                .bind(topic_id)
                .bind(&message.id)
                .execute(&mut **tx)
                .await
                .map_err(|e| e.to_string())?;
        }

        // 3. 触发聚合哈希冒泡 (通过 HashAggregator 统一处理)
        if !skip_bubble {
            HashAggregator::bubble_from_topic(tx, topic_id).await?;
        }

        Ok(())
    }

    async fn upsert_attachments_for_message(
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        topic_id: &str,
        msg_id: &str,
        timestamp: i64,
        attachments: &[crate::vcp_modules::chat_manager::Attachment],
    ) -> Result<(), String> {
        sqlx::query("DELETE FROM message_attachments WHERE topic_id = ? AND msg_id = ?")
            .bind(topic_id)
            .bind(msg_id)
            .execute(&mut **tx)
            .await
            .map_err(|e| e.to_string())?;

        for (i, att) in attachments.iter().enumerate() {
            let hash = att.hash.clone().unwrap_or_else(|| {
                crate::vcp_modules::infra::utils::calculate_sha256(att.src.as_bytes())
            });

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
                    updated_at = excluded.updated_at"
            )
            .bind(&hash)
            .bind(&att.r#type)
            .bind(att.size as i64)
            .bind(&att.internal_path)
            .bind(&att.extracted_text)
            .bind(image_frames)
            .bind(&att.thumbnail_path)
            .bind(timestamp)
            .bind(timestamp)
            .execute(&mut **tx)
            .await
            .map_err(|e| e.to_string())?;

            sqlx::query(
                "INSERT INTO message_attachments (
                    topic_id, msg_id, hash, attachment_order, display_name, src, status, created_at
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(topic_id)
            .bind(msg_id)
            .bind(&hash)
            .bind(i as i32)
            .bind(&att.name)
            .bind(&att.src)
            .bind(&att.status)
            .bind(timestamp)
            .execute(&mut **tx)
            .await
            .map_err(|e| e.to_string())?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tombstone_tests {
    use super::MessageRepository;

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
            "CREATE TABLE topics (topic_id TEXT PRIMARY KEY, deleted_at INTEGER);
             CREATE TABLE messages (
                topic_id TEXT NOT NULL,
                msg_id TEXT NOT NULL,
                deleted_at INTEGER,
                PRIMARY KEY(topic_id, msg_id)
             );",
        )
        .execute(&pool)
        .await
        .expect("create tombstone tables");
        sqlx::query(
            "INSERT INTO topics VALUES ('live-topic', NULL), ('deleted-topic', 7);
             INSERT INTO messages VALUES ('live-topic', 'deleted-message', 8);",
        )
        .execute(&pool)
        .await
        .expect("insert tombstones");

        let mut tx = pool.begin().await.expect("begin transaction");
        MessageRepository::ensure_upsert_target_live(&mut tx, "live-topic", "new-message")
            .await
            .expect("new message in a live topic is allowed");
        let message_error =
            MessageRepository::ensure_upsert_target_live(&mut tx, "live-topic", "deleted-message")
                .await
                .expect_err("message tombstone is monotonic");
        assert!(message_error.contains("tombstoned"));
        let topic_error =
            MessageRepository::ensure_upsert_target_live(&mut tx, "deleted-topic", "new-message")
                .await
                .expect_err("topic tombstone blocks child writes");
        assert!(topic_error.contains("deleted or missing"));
    }
}

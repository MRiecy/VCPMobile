use crate::vcp_modules::chat_manager::ChatMessage;
use crate::vcp_modules::content_parser::{parse_content, ContentBlock};
use crate::vcp_modules::sync_hash::HashAggregator;
use serde::Serialize;
use sha2::Digest;
use sqlx::Row;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::mpsc;

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
    let blocks = MessageRenderCompiler::compile(&content);

    Ok(blocks)
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

fn open_maintenance_rusqlite(db_path: &std::path::Path) -> Result<rusqlite::Connection, String> {
    let conn = rusqlite::Connection::open(db_path).map_err(|e| e.to_string())?;
    conn.execute("PRAGMA journal_mode = WAL", []).ok();
    conn.execute("PRAGMA synchronous = NORMAL", []).ok();
    conn.execute("PRAGMA busy_timeout = 30000", []).ok();
    Ok(conn)
}

/// 分页流式读取已有渲染缓存的消息的 (topic_id, msg_id, content_bytes)，不做任何解压
async fn stream_cached_message_contents(
    pool: &sqlx::SqlitePool,
    tx: mpsc::Sender<(String, String, Vec<u8>)>,
) -> Result<(), String> {
    let mut last_rowid = 0i64;
    const FETCH_SIZE: i64 = 500;

    loop {
        let rows = sqlx::query(
            "SELECT m.rowid, m.topic_id, m.msg_id, m.content \
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
                    let content_bytes: Vec<u8> = row.get("content");
                    if tx.send((topic_id, msg_id, content_bytes)).await.is_err() {
                        return Ok(());
                    }
                }
            }
            _ => break,
        }
    }
    Ok(())
}

/// 通用批量 UPDATE Writer，带进度发射
fn run_batch_update_writer(
    db_path: &std::path::Path,
    mut rx: mpsc::Receiver<Vec<(String, String, Vec<u8>)>>,
    update_sql: &str,
    progress_event: &str,
    app_handle: AppHandle,
    total: usize,
) -> tokio::task::JoinHandle<Result<(), String>> {
    let update_sql = update_sql.to_string();
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
                let mut stmt = tx.prepare_cached(&update_sql).map_err(|e| e.to_string())?;
                let now = chrono::Utc::now().timestamp_millis();
                for (topic_id, msg_id, bytes) in batch {
                    // 适配 render_cache 的 4 参数 SQL (topic_id, msg_id, bytes, now)
                    // 或 content_compress 的 3 参数 SQL (bytes, topic_id, msg_id)
                    if update_sql.contains("render_cache") {
                        stmt.execute(rusqlite::params![topic_id, msg_id, bytes, now])
                            .map_err(|e| e.to_string())?;
                    } else {
                        stmt.execute(rusqlite::params![bytes, topic_id, msg_id])
                            .map_err(|e| e.to_string())?;
                    }
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

    let (tx_compiler, rx_compiler) = mpsc::channel::<(String, String, String)>(1000);
    let (tx_writer, rx_writer) = mpsc::channel::<Vec<(String, String, Vec<u8>)>>(100);
    let total_count = total as usize;

    // --- Stage 3: Writer ---
    let writer_handle = run_batch_update_writer(
        &db_path,
        rx_writer,
        "INSERT INTO render_cache (topic_id, msg_id, render_content, updated_at) VALUES (?, ?, ?, ?) \
         ON CONFLICT(topic_id, msg_id) DO UPDATE SET render_content = excluded.render_content, updated_at = excluded.updated_at",
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
                    Some((topic_id, msg_id, content)) => {
                        let blocks = MessageRenderCompiler::compile(&content);
                        if let Ok(bytes) = MessageRenderCompiler::serialize(&blocks) {
                            batch.push((topic_id, msg_id, bytes));
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
        let (tx_inner, mut rx_inner) = mpsc::channel::<(String, String, Vec<u8>)>(1000);

        let stream_handle = tokio::spawn(async move {
            let _ = stream_cached_message_contents(&pool, tx_inner).await;
        });

        while let Some((topic_id, msg_id, content_bytes)) = rx_inner.recv().await {
            let content = ContentCompressor::decompress(&content_bytes)
                .unwrap_or_else(|_| String::from_utf8_lossy(&content_bytes).to_string());
            if tx_compiler.send((topic_id, msg_id, content)).await.is_err() {
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

    writer_handle.await.map_err(|e| e.to_string())??;

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
    pub async fn upsert_message(
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        message: &ChatMessage,
        topic_id: &str,
        render_content: &[u8],
        skip_bubble: bool,
    ) -> Result<(), String> {
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

        let content_hash =
            HashAggregator::compute_message_fingerprint(&message.content, &attachment_hashes);

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
                is_group_message = excluded.is_group_message,
                group_id = excluded.group_id,
                finish_reason = excluded.finish_reason,
                content_hash = excluded.content_hash,
                updated_at = excluded.updated_at,
                deleted_at = NULL",
        )
        .bind(&message.id)
        .bind(topic_id)
        .bind(&message.role)
        .bind(&message.name)
        .bind(&message.agent_id)
        .bind(ContentCompressor::compress(&message.content)?)
        .bind(message.timestamp as i64)
        .bind(message.is_group_message.unwrap_or(false))
        .bind(&message.group_id)
        .bind(&message.finish_reason)
        .bind(&content_hash)
        .bind(message.timestamp as i64) // created_at
        .bind(message.timestamp as i64) // updated_at
        .execute(&mut **tx)
        .await
        .map_err(|e| e.to_string())?;

        // 2.1 插入或更新渲染缓存 (独立表)
        sqlx::query(
            "INSERT INTO render_cache (topic_id, msg_id, render_content, updated_at)
             VALUES (?, ?, ?, ?)
             ON CONFLICT(topic_id, msg_id) DO UPDATE SET
                render_content = excluded.render_content,
                updated_at = excluded.updated_at",
        )
        .bind(topic_id)
        .bind(&message.id)
        .bind(render_content)
        .bind(message.timestamp as i64)
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
                let mut hasher = sha2::Sha256::new();
                sha2::Digest::update(&mut hasher, att.src.as_bytes());
                format!("{:x}", sha2::Digest::finalize(hasher))
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
                    internal_path = excluded.internal_path,
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

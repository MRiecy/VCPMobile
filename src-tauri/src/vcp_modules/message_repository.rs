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

    /// Serializes AST blocks to compressed binary (postcard + zstd)
    pub fn serialize(blocks: &[ContentBlock]) -> Result<Vec<u8>, String> {
        let postcard_bytes = postcard::to_allocvec(blocks)
            .map_err(|e| format!("postcard serialize failed: {}", e))?;
        let compressed = zstd::bulk::compress(&postcard_bytes, 3)
            .map_err(|e| format!("zstd compress failed: {}", e))?;
        Ok(compressed)
    }

    /// Deserializes compressed binary back to AST blocks (postcard + zstd)
    pub fn deserialize(bytes: &[u8]) -> Result<Vec<ContentBlock>, String> {
        // Use a generous upper bound for decompression; zstd will return exact size
        let decompressed = zstd::bulk::decompress(bytes, 16 * 1024 * 1024)
            .map_err(|e| format!("zstd decompress failed: {}", e))?;
        postcard::from_bytes(&decompressed)
            .map_err(|e| format!("postcard deserialize failed: {}", e))
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

#[tauri::command]
pub async fn rebuild_all_pre_renders(app_handle: AppHandle) -> Result<(), String> {
    let db_state = app_handle.state::<crate::vcp_modules::db_manager::DbState>();
    let pool = db_state.pool.clone();
    let db_path = db_state.path.clone();

    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages")
        .fetch_one(&pool)
        .await
        .map_err(|e| e.to_string())?;

    if total == 0 {
        return Ok(());
    }

    // --- Channel setup (Bounded for backpressure) ---
    let (tx_compiler, rx_compiler) = mpsc::channel::<(String, String)>(1000);
    let (tx_writer, mut rx_writer) = mpsc::channel::<Vec<(String, Vec<u8>)>>(100);

    // --- Stage 3: Synchronous Writer Worker (Rusqlite) ---
    let app_handle_writer = app_handle.clone();
    let total_count = total as usize;
    let writer_handle = tokio::task::spawn_blocking(move || -> Result<(), String> {
        let mut conn = rusqlite::Connection::open(&db_path).map_err(|e| e.to_string())?;

        // 配置高性能写入参数
        conn.execute("PRAGMA journal_mode = WAL", []).ok();
        conn.execute("PRAGMA synchronous = NORMAL", []).ok();
        conn.execute("PRAGMA busy_timeout = 30000", []).ok();

        let mut processed = 0;
        let mut last_emit_time = std::time::Instant::now();
        let emit_interval = std::time::Duration::from_millis(32); // 限制 UI 更新频率约为 30FPS

        while let Some(batch) = rx_writer.blocking_recv() {
            let tx = conn.transaction().map_err(|e| e.to_string())?;
            {
                let mut stmt = tx
                    .prepare_cached("UPDATE messages SET render_content = ? WHERE msg_id = ?")
                    .map_err(|e| e.to_string())?;
                for (msg_id, bytes) in batch {
                    stmt.execute(rusqlite::params![bytes, msg_id])
                        .map_err(|e| e.to_string())?;
                    processed += 1;
                }
            }
            tx.commit().map_err(|e| e.to_string())?;

            // 时间节流：避免后端写入过快导致前端重绘堆积
            if last_emit_time.elapsed() >= emit_interval || processed == total_count {
                let _ = app_handle_writer.emit(
                    "render_rebuild_progress",
                    RebuildProgress {
                        current: processed,
                        total: total_count,
                    },
                );
                last_emit_time = std::time::Instant::now();
            }
        }
        Ok(())
    });

    // --- Stage 2: Parallel Compiler Workers (Dedicated Threads) ---
    let concurrency = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(2, 12); // 根据核心数动态调整，最高 12 并发

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
                    Some((msg_id, content)) => {
                        let blocks = MessageRenderCompiler::compile(&content);
                        if let Ok(bytes) = MessageRenderCompiler::serialize(&blocks) {
                            batch.push((msg_id, bytes));
                        }

                        if batch.len() >= 50 {
                            if tx_writer_clone.blocking_send(std::mem::take(&mut batch)).is_err() {
                                break;
                            }
                        }
                    }
                    None => {
                        // 频道已关闭且读空，发送剩余并退出
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

    // --- Stage 1: Async Reader Task (Streaming Fetch) ---
    let reader_handle = tokio::spawn(async move {
        let mut last_rowid = 0i64;
        const FETCH_SIZE: i64 = 500;

        loop {
            // 使用 rowid 游标分页，保证大表读取性能
            let rows = sqlx::query(
                "SELECT rowid, msg_id, content FROM messages WHERE rowid > ? ORDER BY rowid LIMIT ?",
            )
            .bind(last_rowid)
            .bind(FETCH_SIZE)
            .fetch_all(&pool)
            .await;

            match rows {
                Ok(rows) if !rows.is_empty() => {
                    if let Some(last) = rows.last() {
                        last_rowid = last.get::<i64, _>(0);
                    }
                    for row in rows {
                        let msg_id: String = row.get("msg_id");
                        let content: String = row.get("content");
                        if tx_compiler.send((msg_id, content)).await.is_err() {
                            return;
                        }
                    }
                }
                _ => break,
            }
        }
        // 显式丢弃 tx，通知 Compilers 读取已结束
        drop(tx_compiler);
    });

    // ── 等待流水线逐步排空 ──
    let _ = reader_handle.await;
    let _ = futures_util::future::join_all(compiler_handles).await;
    drop(tx_writer); // 通知 Writer 所有任务已处理完毕

    writer_handle.await.map_err(|e| e.to_string())??;

    // 补偿 100% 进度显示
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

        // 2. 插入或更新消息
        sqlx::query(
            "INSERT INTO messages (
                msg_id, topic_id, role, name, agent_id, content, timestamp,
                is_thinking, is_group_message, group_id, finish_reason,
                render_content,
                content_hash,
                created_at, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(msg_id) DO UPDATE SET
                content = excluded.content,
                role = excluded.role,
                name = excluded.name,
                is_thinking = excluded.is_thinking,
                agent_id = excluded.agent_id,
                is_group_message = excluded.is_group_message,
                group_id = excluded.group_id,
                finish_reason = excluded.finish_reason,
                render_content = excluded.render_content,
                content_hash = excluded.content_hash,
                updated_at = excluded.updated_at,
                deleted_at = NULL",
        )
        .bind(&message.id)
        .bind(topic_id)
        .bind(&message.role)
        .bind(&message.name)
        .bind(&message.agent_id)
        .bind(&message.content)
        .bind(message.timestamp as i64)
        .bind(message.is_thinking.unwrap_or(false))
        .bind(message.is_group_message.unwrap_or(false))
        .bind(&message.group_id)
        .bind(&message.finish_reason)
        .bind(render_content)
        .bind(&content_hash)
        .bind(message.timestamp as i64) // created_at
        .bind(message.timestamp as i64) // updated_at
        .execute(&mut **tx)
        .await
        .map_err(|e| e.to_string())?;

        // Handle attachments
        if let Some(ref attachments) = message.attachments {
            Self::upsert_attachments_for_message(
                tx,
                &message.id,
                message.timestamp as i64,
                attachments,
            )
            .await?;
        } else {
            sqlx::query("DELETE FROM message_attachments WHERE msg_id = ?")
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
        msg_id: &str,
        timestamp: i64,
        attachments: &[crate::vcp_modules::chat_manager::Attachment],
    ) -> Result<(), String> {
        sqlx::query("DELETE FROM message_attachments WHERE msg_id = ?")
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
                    extracted_text = excluded.extracted_text,
                    image_frames = excluded.image_frames,
                    thumbnail_path = excluded.thumbnail_path,
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
                    msg_id, hash, attachment_order, display_name, src, status, created_at
                ) VALUES (?, ?, ?, ?, ?, ?, ?)",
            )
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

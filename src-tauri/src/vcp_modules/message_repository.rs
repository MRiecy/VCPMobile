use crate::vcp_modules::chat_manager::ChatMessage;
use crate::vcp_modules::content_parser::{parse_content, ContentBlock};
use crate::vcp_modules::sync_hash::HashAggregator;
use futures_util::StreamExt;
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

    /// Serializes AST blocks to binary (currently just JSON for simplicity, but abstracted)
    pub fn serialize(blocks: &[ContentBlock]) -> Result<Vec<u8>, String> {
        serde_json::to_vec(blocks).map_err(|e| e.to_string())
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

    let _ = app_handle.emit(
        "render_rebuild_progress",
        RebuildProgress {
            current: 0,
            total: total as usize,
        },
    );

    if total == 0 {
        return Ok(());
    }

    // Stage 3: Synchronous Writer Worker (Rusqlite Turbo Mode)
    let (tx_writer, mut rx_writer) = mpsc::channel::<Vec<(String, Vec<u8>)>>(50);
    let app_handle_clone = app_handle.clone();
    let total_count = total as usize;

    let writer_handle = tokio::task::spawn_blocking(move || -> Result<(), String> {
        let mut conn = rusqlite::Connection::open(&db_path).map_err(|e| e.to_string())?;

        // 配置高性能写入参数
        conn.execute("PRAGMA journal_mode = WAL", []).ok();
        conn.execute("PRAGMA synchronous = NORMAL", []).ok();
        conn.execute("PRAGMA busy_timeout = 30000", []).ok();

        let mut processed = 0;

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

            // 由 Writer 发送进度，确保 UI 看到的是真实写入后的状态
            let _ = app_handle_clone.emit(
                "render_rebuild_progress",
                RebuildProgress {
                    current: processed,
                    total: total_count,
                },
            );
        }
        Ok(())
    });

    // Stage 1 & 2: Cursor Reader & Parallel Compilers
    const BATCH_SIZE: usize = 500;
    let concurrency = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .saturating_sub(2)
        .clamp(2, 6);

    let mut last_rowid = 0i64;
    loop {
        // 使用 rowid 游标分页，性能优于 LIMIT OFFSET (O(log N) vs O(N))
        let rows = sqlx::query(
            "SELECT rowid, msg_id, content FROM messages WHERE rowid > ? ORDER BY rowid LIMIT ?",
        )
        .bind(last_rowid)
        .bind(BATCH_SIZE as i64)
        .fetch_all(&pool)
        .await
        .map_err(|e| e.to_string())?;

        if rows.is_empty() {
            break;
        }

        // 更新最后一次看到的 rowid
        if let Some(last) = rows.last() {
            last_rowid = last.get::<i64, _>(0);
        }

        let mut tasks = Vec::new();
        for row in rows {
            let msg_id: String = row.get("msg_id");
            let content: String = row.get("content");

            // 并行生产：编译任务分发到线程池
            tasks.push(async move {
                tokio::task::spawn_blocking(move || {
                    let blocks = MessageRenderCompiler::compile(&content);
                    let bytes = MessageRenderCompiler::serialize(&blocks).ok();
                    (msg_id, bytes)
                })
                .await
            });
        }

        let mut results = futures_util::stream::iter(tasks).buffer_unordered(concurrency);
        let mut batch_data = Vec::new();

        while let Some(res) = results.next().await {
            match res {
                Ok((msg_id, Some(bytes))) => {
                    batch_data.push((msg_id, bytes));
                }
                _ => continue,
            }
        }

        // 送入写入队列
        if !batch_data.is_empty() {
            if tx_writer.send(batch_data).await.is_err() {
                break;
            }
        }
    }

    // 释放发送端，等待 Writer 完成所有待处理任务
    drop(tx_writer);
    writer_handle.await.map_err(|e| e.to_string())??;

    // 补偿可能的四舍五入或过滤导致的进度差
    let _ = app_handle.emit(
        "render_rebuild_progress",
        RebuildProgress {
            current: total as usize,
            total: total as usize,
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

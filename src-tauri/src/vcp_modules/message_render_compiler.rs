use crate::vcp_modules::content_parser::{ensure_html_fenced, parse_content, ContentBlock};
use futures_util::StreamExt;
use serde::Serialize;
use sqlx::Row;
use tauri::{AppHandle, Emitter, Manager};
use tokio::task;

pub struct MessageRenderCompiler;

impl MessageRenderCompiler {
    /// Compiles raw message content into AST blocks (the "astbin" format base)
    pub fn compile(content: &str) -> Vec<ContentBlock> {
        // 1. Pre-process HTML fencing (Ported from content_parser robustly)
        let fenced_content = ensure_html_fenced(content);

        // 2. Core parse
        parse_content(&fenced_content)
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

    // 批量大小 150：参数 150*3=450 < 999，进度条约 8-9次/秒，性能与内存平衡
    const BATCH_SIZE: usize = 150;
    // 动态并发：留 2 核给系统和 IPC，避免全核满载触发温控降频
    let concurrency = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .saturating_sub(2)
        .clamp(2, 6);
    let mut offset = 0;
    let mut processed = 0;

    loop {
        let rows = sqlx::query("SELECT msg_id, content FROM messages LIMIT ? OFFSET ?")
            .bind(BATCH_SIZE as i64)
            .bind(offset as i64)
            .fetch_all(&pool)
            .await
            .map_err(|e| e.to_string())?;

        if rows.is_empty() {
            break;
        }

        let mut tasks = Vec::new();
        for row in rows {
            let msg_id: String = row.get("msg_id");
            let content: String = row.get("content");

            tasks.push(task::spawn(async move {
                let blocks = MessageRenderCompiler::compile(&content);
                let bytes = MessageRenderCompiler::serialize(&blocks).ok();
                (msg_id, bytes)
            }));
        }

        // 并发编译 (CPU 密集型)
        let mut results = futures_util::stream::iter(tasks).buffer_unordered(concurrency);
        let mut batch_data = Vec::new();

        while let Some(res) = results.next().await {
            if let Ok((msg_id, Some(bytes))) = res {
                batch_data.push((msg_id, bytes));
            }
        }

        // 批量 UPDATE：CASE WHEN 单条 SQL，避免 INSERT 分支的 NOT NULL 约束问题
        if !batch_data.is_empty() {
            let mut tx = pool.begin().await.map_err(|e| e.to_string())?;

            let case_parts: Vec<String> =
                batch_data.iter().map(|_| "WHEN ? THEN ?".to_string()).collect();
            let in_parts: Vec<String> = batch_data.iter().map(|_| "?".to_string()).collect();

            let sql = format!(
                "UPDATE messages
                 SET render_content = CASE msg_id {} ELSE render_content END
                 WHERE msg_id IN ({})",
                case_parts.join(" "),
                in_parts.join(", ")
            );

            let mut query = sqlx::query(&sql);
            for (msg_id, bytes) in &batch_data {
                query = query.bind(msg_id).bind(bytes);
            }
            for (msg_id, _) in &batch_data {
                query = query.bind(msg_id);
            }

            query.execute(&mut *tx).await.map_err(|e| e.to_string())?;
            tx.commit().await.map_err(|e| e.to_string())?;

            processed += batch_data.len();
            let _ = app_handle.emit(
                "render_rebuild_progress",
                RebuildProgress {
                    current: processed,
                    total: total as usize,
                },
            );
        }

        offset += BATCH_SIZE;
    }

    let _ = app_handle.emit(
        "render_rebuild_progress",
        RebuildProgress {
            current: total as usize,
            total: total as usize,
        },
    );
    Ok(())
}

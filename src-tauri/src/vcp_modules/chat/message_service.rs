use crate::vcp_modules::chat_manager::{Attachment, ChatMessage};
use crate::vcp_modules::content_parser::ContentBlock;
use crate::vcp_modules::file_manager::get_attachments_root_dir;
use crate::vcp_modules::message_repository::MessageRepository;
use crate::vcp_modules::message_repository::{ContentCompressor, MessageRenderCompiler};
use crate::vcp_modules::settings_manager;
use sqlx::Row;
use std::path::Path;
use tauri::{AppHandle, Manager};
use tokio::fs;

// =================================================================
// vcp_modules/message_service.rs - 消息业务逻辑中心 (含附件对齐)
// =================================================================

/// 批量加载多个 topic 的全量消息 — 一次性 SQL 查询，按 topic_id 分组
/// 避免 push_messages_batch 场景下的 N+1 查询
pub async fn load_multi_topic_messages(
    pool: &sqlx::SqlitePool,
    topic_ids: &[String],
) -> Result<
    std::collections::HashMap<String, Vec<crate::vcp_modules::chat_manager::ChatMessage>>,
    String,
> {
    use sqlx::Row;
    let mut result: std::collections::HashMap<
        String,
        Vec<crate::vcp_modules::chat_manager::ChatMessage>,
    > = topic_ids
        .iter()
        .map(|id| (id.clone(), Vec::new()))
        .collect();

    if topic_ids.is_empty() {
        return Ok(result);
    }

    let placeholders = topic_ids.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
    let query_str = format!(
        "SELECT m.msg_id, m.role, m.name, m.agent_id, m.content, m.timestamp, m.is_group_message, m.group_id, m.finish_reason, r.render_content, m.topic_id, m.content_hash
         FROM messages m
         LEFT JOIN render_cache r ON m.topic_id = r.topic_id AND m.msg_id = r.msg_id
         WHERE m.topic_id IN ({}) AND m.deleted_at IS NULL
         ORDER BY m.topic_id, m.timestamp ASC, m.msg_id ASC",
        placeholders
    );

    let mut q = sqlx::query(&query_str);
    for id in topic_ids {
        q = q.bind(id);
    }
    let rows = q.fetch_all(pool).await.map_err(|e| e.to_string())?;

    for row in rows {
        let msg_id: String = row.get("msg_id");
        let role: String = row.get("role");
        let topic_id: String = row.get("topic_id");
        let timestamp: i64 = row.get("timestamp");
        let render_content: Option<Vec<u8>> = row.get("render_content");
        let blocks = parse_render_bytes(render_content);

        let content_bytes: Vec<u8> = row.get("content");
        let content = ContentCompressor::decompress(&content_bytes).unwrap_or_default();
        let content_hash_raw: String = row.get("content_hash");
        let content_hash = if content_hash_raw.is_empty() { None } else { Some(content_hash_raw) };

        let message = crate::vcp_modules::chat_manager::ChatMessage {
            id: msg_id,
            role,
            name: row.get("name"),
            content,
            timestamp: timestamp as u64,
            is_thinking: Some(false),
            agent_id: row.get("agent_id"),
            group_id: row.get("group_id"),
            topic_id: Some(topic_id.clone()),
            is_group_message: Some(row.get::<i64, _>("is_group_message") != 0),
            finish_reason: row.get("finish_reason"),
            attachments: None, // 批量 push 场景不需要附件回填
            blocks,
            shell: None, // 批量 push 场景不需要外壳预计算
            content_hash,
        };

        result.entry(topic_id).or_default().push(message);
    }

    // 批量加载附件 — 收集所有 (topic_id, msg_id)，一次 JOIN 查询
    let mut all_msg_refs: Vec<(String, String)> = Vec::new();
    for (tid, msgs) in result.iter() {
        for m in msgs {
            all_msg_refs.push((tid.clone(), m.id.clone()));
        }
    }

    if !all_msg_refs.is_empty() {
        let mut att_placeholders = Vec::new();
        att_placeholders.extend(std::iter::repeat_n("(?, ?)", all_msg_refs.len()));
        let att_query = format!(
            "SELECT a.hash, a.mime_type, a.size, a.internal_path, NULL as extracted_text, a.image_frames, a.thumbnail_path, a.created_at,
                    ma.topic_id, ma.msg_id, ma.display_name, ma.src, ma.status
             FROM message_attachments ma
             JOIN attachments a ON ma.hash = a.hash
             WHERE (ma.topic_id, ma.msg_id) IN ({})
             ORDER BY ma.topic_id, ma.msg_id, ma.attachment_order ASC",
            att_placeholders.join(",")
        );
        let mut q = sqlx::query(&att_query);
        for (tid, mid) in &all_msg_refs {
            q = q.bind(tid).bind(mid);
        }
        if let Ok(att_rows) = q.fetch_all(pool).await {
            let mut att_map: std::collections::HashMap<(String, String), Vec<Attachment>> =
                std::collections::HashMap::new();
            for ar in att_rows {
                let tid: String = ar.get("topic_id");
                let mid: String = ar.get("msg_id");
                let hash: String = ar.get("hash");
                let mime_type: String = ar.get("mime_type");
                let internal_path: String = ar.get("internal_path");
                let display_name: String = ar.get("display_name");
                let size_i64: i64 = ar.get("size");
                let created_at_i64: i64 = ar.get("created_at");

                att_map.entry((tid, mid)).or_default().push(Attachment {
                    r#type: mime_type,
                    src: ar.get("src"),
                    name: display_name,
                    size: size_i64 as u64,
                    hash: Some(hash),
                    status: Some(ar.get("status")),
                    internal_path,
                    extracted_text: ar.get("extracted_text"),
                    image_frames: ar
                        .get::<Option<String>, _>("image_frames")
                        .and_then(|s| serde_json::from_str(&s).ok()),
                    thumbnail_path: ar.get("thumbnail_path"),
                    created_at: Some(created_at_i64 as u64),
                });
            }
            // 回填附件到消息
            for (tid, msgs) in result.iter_mut() {
                for msg in msgs.iter_mut() {
                    if let Some(atts) = att_map.remove(&(tid.clone(), msg.id.clone())) {
                        msg.attachments = Some(atts);
                    }
                }
            }
        }
    }

    Ok(result)
}

pub async fn load_chat_history_internal(
    _app_handle: &AppHandle,
    _owner_id: &str,
    _owner_type: &str,
    topic_id: &str,
    limit: Option<usize>,
    offset: Option<usize>,
    include_content: bool,
) -> Result<Vec<ChatMessage>, String> {
    let db_state = _app_handle.state::<crate::vcp_modules::db_manager::DbState>();
    let pool = &db_state.pool;

    let offset = offset.unwrap_or(0);

    let query_str = if limit.is_some() {
        "SELECT m.msg_id, m.role, m.name, m.agent_id, m.content, m.timestamp, m.is_group_message, m.group_id, m.finish_reason, r.render_content, m.content_hash 
         FROM messages m
         LEFT JOIN render_cache r ON m.topic_id = r.topic_id AND m.msg_id = r.msg_id
         WHERE m.topic_id = ? AND m.deleted_at IS NULL 
         ORDER BY m.timestamp DESC, m.rowid DESC 
         LIMIT ? OFFSET ?"
    } else {
        "SELECT m.msg_id, m.role, m.name, m.agent_id, m.content, m.timestamp, m.is_group_message, m.group_id, m.finish_reason, r.render_content, m.content_hash 
         FROM messages m
         LEFT JOIN render_cache r ON m.topic_id = r.topic_id AND m.msg_id = r.msg_id
         WHERE m.topic_id = ? AND m.deleted_at IS NULL 
         ORDER BY m.timestamp DESC, m.rowid DESC"
    };

    let mut q = sqlx::query(query_str).bind(topic_id);
    if let Some(l) = limit {
        q = q.bind(l as i64);
        q = q.bind(offset as i64);
    }
    let rows = q.fetch_all(pool).await.map_err(|e| e.to_string())?;

    // 收集所有 msg_id，用于批量查询附件
    let mut msg_ids = Vec::new();
    for row in &rows {
        use sqlx::Row;
        let msg_id: String = row.get("msg_id");
        msg_ids.push(msg_id);
    }

    // 批量查询所有附件（利用 message_attachments 索引表）
    let mut att_map: std::collections::HashMap<String, Vec<Attachment>> =
        std::collections::HashMap::new();
    if !msg_ids.is_empty() {
        let placeholders = msg_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let att_query = format!(
            "SELECT a.hash, a.mime_type, a.size, a.internal_path, NULL as extracted_text, a.image_frames, a.thumbnail_path, a.created_at,
                    ma.msg_id, ma.display_name, ma.src, ma.status
             FROM message_attachments ma
             JOIN attachments a ON ma.hash = a.hash
             WHERE ma.topic_id = ? AND ma.msg_id IN ({}) 
             ORDER BY ma.msg_id, ma.attachment_order ASC",
            placeholders
        );
        let mut q = sqlx::query(&att_query).bind(topic_id);
        for id in &msg_ids {
            q = q.bind(id);
        }
        let att_rows = q.fetch_all(pool).await.map_err(|e| e.to_string())?;

        for ar in att_rows {
            let msg_id: String = ar.get("msg_id");
            let hash: String = ar.get("hash");
            let mime_type: String = ar.get("mime_type");
            let internal_path: String = ar.get("internal_path");
            let display_name: String = ar.get("display_name");
            let size_i64: i64 = ar.get("size");
            let created_at_i64: i64 = ar.get("created_at");

            att_map.entry(msg_id).or_default().push(Attachment {
                r#type: mime_type,
                src: ar.get("src"),
                name: display_name,
                size: size_i64 as u64,
                hash: Some(hash),
                status: Some(ar.get("status")),
                internal_path,
                extracted_text: ar.get("extracted_text"),
                image_frames: ar
                    .get::<Option<String>, _>("image_frames")
                    .and_then(|s| serde_json::from_str(&s).ok()),
                thumbnail_path: ar.get("thumbnail_path"),
                created_at: Some(created_at_i64 as u64),
            });
        }
    }

    // 预计算外壳属性所需的全局数据
    let agents =
        crate::vcp_modules::agent_service::get_agents(_app_handle.clone(), _app_handle.state())
            .await
            .unwrap_or_default();
    let settings = crate::vcp_modules::settings_manager::read_settings(
        _app_handle.clone(),
        _app_handle.state(),
    )
    .await
    .ok();
    let user_name = settings
        .map(|s| s.user_name)
        .unwrap_or_else(|| "User".to_string());

    let user_avatar_color: Option<String> = sqlx::query_scalar(
        "SELECT dominant_color FROM avatars WHERE owner_type = 'user' AND owner_id = 'user_avatar'",
    )
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();

    let mut history = Vec::new();
    for row in rows {
        use sqlx::Row;
        let msg_id: String = row.get("msg_id");
        let role: String = row.get("role");
        let name: Option<String> = row.get("name");

        let content_bytes: Vec<u8> = row.get("content");
        let render_content: Option<Vec<u8>> = row.get("render_content");

        // 懒渲染策略：render_cache 命中则直接用，未命中则实时编译
        let (blocks, content) = if let Some(ref rb) = render_content {
            let blocks = parse_render_bytes(Some(rb.clone()));
            let content = if include_content {
                ContentCompressor::decompress(&content_bytes).unwrap_or_default()
            } else {
                String::new()
            };
            (blocks, content)
        } else {
            // 未命中：解压 content → 编译 blocks → 异步回写 cache
            let decompressed = ContentCompressor::decompress(&content_bytes).unwrap_or_default();
            if decompressed.is_empty() {
                (None, String::new())
            } else {
                let compiled = MessageRenderCompiler::compile(&decompressed);
                let blocks_json = serde_json::to_value(&compiled).ok();
                
                // 异步回写 render_cache (使用 tokio::spawn，不阻塞消息加载流)
                if let Ok(serialized) = MessageRenderCompiler::serialize(&compiled) {
                    let pool_c = pool.clone();
                    let tid = topic_id.to_string();
                    let mid = msg_id.clone();
                    tokio::spawn(async move {
                        let now = chrono::Utc::now().timestamp_millis();
                        let _ = sqlx::query(
                            "INSERT INTO render_cache (topic_id, msg_id, render_content, updated_at) \
                             VALUES (?, ?, ?, ?) \
                             ON CONFLICT(topic_id, msg_id) DO UPDATE SET \
                             render_content = excluded.render_content, \
                             updated_at = excluded.updated_at"
                        )
                        .bind(&tid)
                        .bind(&mid)
                        .bind(&serialized)
                        .bind(now)
                        .execute(&pool_c)
                        .await;
                    });
                }
                
                let content = if include_content { decompressed } else { String::new() };
                (blocks_json, content)
            }
        };

        let content_hash_raw: String = row.get("content_hash");
        let content_hash = if content_hash_raw.is_empty() { None } else { Some(content_hash_raw) };

        let timestamp: i64 = row.get("timestamp");
        let is_thinking: Option<bool> = Some(false);

        let attachments = att_map.remove(&msg_id);

        let mut message = ChatMessage {
            id: msg_id,
            role,
            name,
            content,
            timestamp: timestamp as u64,
            is_thinking,
            agent_id: row.get("agent_id"),
            group_id: row.get("group_id"),
            topic_id: Some(topic_id.to_string()),
            is_group_message: Some(row.get::<i64, _>("is_group_message") != 0),
            finish_reason: row.get("finish_reason"),
            attachments,
            blocks,
            shell: None,
            content_hash,
        };

        message.shell = Some(crate::vcp_modules::pre_renderer::precompute_shell(
            &message,
            &agents,
            &user_name,
            user_avatar_color.as_deref(),
        ));
        history.push(message);
    }

    history.reverse();
    Ok(history)
}

/// 核心：确保消息中的附件在手机本地物理存在，否则从电脑同步下载
async fn ensure_attachments_locally(
    app: &AppHandle,
    message: &mut ChatMessage,
) -> Result<(), String> {
    let attachments = match &mut message.attachments {
        Some(atts) => atts,
        None => return Ok(()),
    };

    let att_dir = get_attachments_root_dir(app)?;
    if !att_dir.exists() {
        fs::create_dir_all(&att_dir)
            .await
            .map_err(|e| e.to_string())?;
    }

    for att in attachments {
        let hash = match &att.hash {
            Some(h) => h.clone(),
            None => continue,
        };

        // 判定后缀 (对齐 file_manager.rs 逻辑)
        let ext = Path::new(&att.name)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("");

        let local_file_name = if ext.is_empty() {
            hash.clone()
        } else {
            format!("{}.{}", hash, ext)
        };

        let local_path = att_dir.join(&local_file_name);
        let local_path_str = local_path.to_string_lossy().into_owned();

        if !local_path.exists() {
            // 尝试下载
            let settings = settings_manager::read_settings(app.clone(), app.state()).await?;
            if !settings.sync_http_url.is_empty() {
                let client = reqwest::Client::new();
                let url = format!(
                    "{}/api/mobile-sync/download-attachment?hash={}",
                    settings.sync_http_url, hash
                );
                match client
                    .get(&url)
                    .header("x-sync-token", &settings.sync_token)
                    .header("Authorization", format!("Bearer {}", &settings.sync_token))
                    .send()
                    .await
                {
                    Ok(resp) if resp.status().is_success() => {
                        if let Ok(bytes) = resp.bytes().await {
                            let _ = fs::write(&local_path, bytes).await;
                        }
                    }
                    _ => {} // 下载失败则跳过，UI 会显示裂图
                }
            }
        }

        // 核心对齐：
        // 1. src 保持物理路径（用于超栈追踪），如果来自电脑端，它已经包含 file:// 路径
        // 2. internal_path 专门作为手机本地可访问路径，前端可通过 convertFileSrc 转换为 asset://
        if att.src.is_empty() {
            att.src = format!("file://{}", local_path_str);
        }
        att.internal_path = local_path_str;
    }
    Ok(())
}

pub async fn append_single_message(
    app_handle: AppHandle,
    db_pool: &sqlx::Pool<sqlx::Sqlite>,
    _owner_id: &str,
    _owner_type: &str,
    topic_id: String,
    mut message: ChatMessage,
) -> Result<Vec<ContentBlock>, String> {
    ensure_attachments_locally(&app_handle, &mut message).await?;

    let blocks: Vec<ContentBlock> = if let Some(blocks_val) = &message.blocks {
        serde_json::from_value(blocks_val.clone()).map_err(|e| e.to_string())?
    } else {
        MessageRenderCompiler::compile(&message.content)
    };
    let render_bytes = MessageRenderCompiler::serialize(&blocks)?;

    let mut tx = db_pool.begin().await.map_err(|e| e.to_string())?;
    MessageRepository::upsert_message(&mut tx, &message, &topic_id, &render_bytes, false).await?;

    let msg_count: i32 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM messages WHERE topic_id = ? AND deleted_at IS NULL",
    )
    .bind(&topic_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| e.to_string())?
    .unwrap_or(0);

    sqlx::query("UPDATE topics SET updated_at = ?, msg_count = ? WHERE topic_id = ?")
        .bind(message.timestamp as i64)
        .bind(msg_count)
        .bind(&topic_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(blocks)
}

#[tauri::command]
pub async fn fetch_raw_message_content(
    app_handle: tauri::AppHandle,
    message_id: String,
) -> Result<String, String> {
    let db_state = app_handle.state::<crate::vcp_modules::db_manager::DbState>();
    let pool = &db_state.pool;

    let row = sqlx::query("SELECT content FROM messages WHERE msg_id = ?")
        .bind(&message_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| e.to_string())?;

    match row {
        Some(r) => {
            let bytes: Vec<u8> = r.get(0);
            let content = ContentCompressor::decompress(&bytes).map_err(|e| {
                format!(
                    "Failed to decompress content for message {}: {}",
                    message_id, e
                )
            })?;
            Ok(content)
        }
        None => Err(format!("Message {} not found", message_id)),
    }
}

#[tauri::command]
pub async fn re_render_message(
    app_handle: tauri::AppHandle,
    message_id: String,
    topic_id: String,
) -> Result<serde_json::Value, String> {
    let db_state = app_handle.state::<crate::vcp_modules::db_manager::DbState>();
    let pool = &db_state.pool;

    let row = sqlx::query("SELECT content FROM messages WHERE msg_id = ? AND topic_id = ?")
        .bind(&message_id)
        .bind(&topic_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| e.to_string())?;

    match row {
        Some(r) => {
            let bytes: Vec<u8> = r.get("content");
            let decompressed = ContentCompressor::decompress(&bytes).map_err(|e| {
                format!(
                    "Failed to decompress content for message {} in topic {}: {}",
                    message_id, topic_id, e
                )
            })?;

            let compiled = MessageRenderCompiler::compile(&decompressed);
            let serialized = MessageRenderCompiler::serialize(&compiled)?;

            let now = chrono::Utc::now().timestamp_millis();
            sqlx::query(
                "INSERT INTO render_cache (topic_id, msg_id, render_content, updated_at) \
                 VALUES (?, ?, ?, ?) \
                 ON CONFLICT(topic_id, msg_id) DO UPDATE SET \
                 render_content = excluded.render_content, \
                 updated_at = excluded.updated_at",
            )
            .bind(&topic_id)
            .bind(&message_id)
            .bind(&serialized)
            .bind(now)
            .execute(pool)
            .await
            .map_err(|e| e.to_string())?;

            serde_json::to_value(&compiled).map_err(|e| e.to_string())
        }
        None => Err(format!(
            "Message {} with topic {} not found",
            message_id, topic_id
        )),
    }
}

pub async fn patch_single_message(
    app_handle: AppHandle,
    db_pool: &sqlx::Pool<sqlx::Sqlite>,
    _owner_id: &str,
    _owner_type: &str,
    topic_id: String,
    mut message: ChatMessage,
    skip_bubble: bool,
) -> Result<Vec<ContentBlock>, String> {
    ensure_attachments_locally(&app_handle, &mut message).await?;

    // 优先使用传入的 blocks，如果缺失则实时编译
    let blocks: Vec<ContentBlock> = if let Some(blocks_val) = &message.blocks {
        serde_json::from_value(blocks_val.clone()).map_err(|e| e.to_string())?
    } else {
        MessageRenderCompiler::compile(&message.content)
    };
    let render_bytes = MessageRenderCompiler::serialize(&blocks)?;

    let mut tx = db_pool.begin().await.map_err(|e| e.to_string())?;
    MessageRepository::upsert_message(&mut tx, &message, &topic_id, &render_bytes, skip_bubble)
        .await?;

    let now = chrono::Utc::now().timestamp_millis();
    sqlx::query("UPDATE topics SET updated_at = ? WHERE topic_id = ?")
        .bind(now)
        .bind(&topic_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(blocks)
}

pub async fn delete_messages(
    db_pool: &sqlx::Pool<sqlx::Sqlite>,
    topic_id: &str,
    msg_ids: Vec<String>,
) -> Result<(), String> {
    if msg_ids.is_empty() {
        return Ok(());
    }
    let mut tx = db_pool.begin().await.map_err(|e| e.to_string())?;
    let delete_query = format!(
        "UPDATE messages SET deleted_at = ? WHERE topic_id = ? AND msg_id IN ({})",
        msg_ids.iter().map(|_| "?").collect::<Vec<_>>().join(", ")
    );
    let now = chrono::Utc::now().timestamp_millis();
    let mut q = sqlx::query(&delete_query).bind(now).bind(topic_id);
    for id in &msg_ids {
        q = q.bind(id);
    }
    q.execute(&mut *tx).await.map_err(|e| e.to_string())?;

    let msg_count: i32 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM messages WHERE topic_id = ? AND deleted_at IS NULL",
    )
    .bind(topic_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| e.to_string())?
    .unwrap_or(0);

    sqlx::query("UPDATE topics SET msg_count = ?, updated_at = ? WHERE topic_id = ?")
        .bind(msg_count)
        .bind(now)
        .bind(topic_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(())
}

pub async fn truncate_history_after_timestamp(
    _app_handle: AppHandle,
    db_pool: &sqlx::Pool<sqlx::Sqlite>,
    _owner_id: &str,
    _owner_type: &str,
    topic_id: &str,
    timestamp: i64,
) -> Result<(), String> {
    let mut tx = db_pool.begin().await.map_err(|e| e.to_string())?;
    sqlx::query("DELETE FROM message_attachments WHERE msg_id IN (SELECT msg_id FROM messages WHERE topic_id = ? AND timestamp > ?)")
        .bind(topic_id).bind(timestamp).execute(&mut *tx).await.map_err(|e| e.to_string())?;
    sqlx::query("DELETE FROM messages WHERE topic_id = ? AND timestamp > ?")
        .bind(topic_id)
        .bind(timestamp)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    let msg_count: i32 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM messages WHERE topic_id = ? AND deleted_at IS NULL",
    )
    .bind(topic_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| e.to_string())?
    .unwrap_or(0);
    sqlx::query("UPDATE topics SET msg_count = ?, updated_at = ? WHERE topic_id = ?")
        .bind(msg_count)
        .bind(timestamp)
        .bind(topic_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(())
}

/// Helper: Deserializes render_content bytes (JSON + zstd) into JSON blocks for frontend
fn parse_render_bytes(render_content: Option<Vec<u8>>) -> Option<serde_json::Value> {
    render_content.and_then(|bytes| {
        crate::vcp_modules::message_repository::MessageRenderCompiler::deserialize(&bytes)
            .ok()
            .and_then(
                |blocks: Vec<crate::vcp_modules::content_parser::ContentBlock>| {
                    serde_json::to_value(blocks).ok()
                },
            )
    })
}
